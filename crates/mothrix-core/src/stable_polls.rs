// SPDX-License-Identifier: MPL-2.0

//! Polls in the standardised `m.poll.*` format (Matrix 1.7).
//!
//! matrix-sdk's timeline only understands the older "unstable"
//! `org.matrix.msc3381.poll.*` events: it aggregates those itself and drops
//! the standard ones. Some servers' own clients send the standard format,
//! so mothrix lets those poll starts through, fetches their responses via the
//! relations API and tallies the votes here.
//!
//! Everything works on raw JSON and accepts a few spellings per field, since
//! this is exactly the place where clients differ.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::model::PollFormat;

/// Event types treated as the start of a standard-format poll.
pub const START_TYPES: [&str; 2] = ["m.poll.start", "org.matrix.msc3381.v2.poll.start"];
const RESPONSE_TYPES: [&str; 3] = [
    "m.poll.response",
    "org.matrix.msc3381.v2.poll.response",
    "org.matrix.msc3381.poll.response",
];
const END_TYPES: [&str; 3] = [
    "m.poll.end",
    "org.matrix.msc3381.v2.poll.end",
    "org.matrix.msc3381.poll.end",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub sender: String,
    pub ts: u64,
    pub selections: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StablePoll {
    pub sender: String,
    /// `Standard` or `Mixed`, decided by how the start event is laid out.
    pub format: PollFormat,
    pub question: String,
    pub disclosed: bool,
    pub max_selections: u64,
    /// (answer id, text) in display order.
    pub answers: Vec<(String, String)>,
    pub responses: Vec<Response>,
    /// When the poll's author ended it.
    pub end_ts: Option<u64>,
}

fn first<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| value.get(key))
}

/// Text of an extensible-events text block in any of its shapes:
/// `[{"body": "..."}]`, `{"m.text": [...]}`, or a bare string.
fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            // Prefer the plain-text representation over HTML.
            let plain = parts.iter().find(|part| {
                part.get("mimetype")
                    .and_then(Value::as_str)
                    .is_none_or(|mime| mime == "text/plain")
            });
            plain
                .or(parts.first())
                .and_then(|part| part.get("body"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        }
        Value::Object(_) => {
            first(value, &["m.text", "org.matrix.msc1767.text", "body"]).and_then(text_of)
        }
        _ => None,
    }
}

/// Parse the content of a poll start event. `None` if it is not a poll.
pub fn parse_start(sender: &str, content: &Value) -> Option<StablePoll> {
    // The standard puts the poll under `m.poll`. IServ keeps the old
    // layout, with the poll under a key named like the event type.
    let (poll, format) = match first(content, &["m.poll", "org.matrix.msc3381.v2.poll"]) {
        Some(poll) => (poll, PollFormat::Standard),
        None => (
            first(content, &["m.poll.start", "org.matrix.msc3381.poll.start"])?,
            PollFormat::Mixed,
        ),
    };
    let question = poll.get("question").and_then(text_of)?;
    let answers: Vec<(String, String)> = poll
        .get("answers")?
        .as_array()?
        .iter()
        .filter_map(|answer| {
            let id = first(answer, &["m.id", "id"])?.as_str()?.to_owned();
            Some((id, text_of(answer)?))
        })
        .collect();
    if answers.is_empty() {
        return None;
    }
    // Undisclosed is the default when the field is missing.
    let disclosed = poll
        .get("kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.ends_with("disclosed") && !kind.ends_with("undisclosed"));
    let max_selections = poll
        .get("max_selections")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1);
    Some(StablePoll {
        sender: sender.to_owned(),
        format,
        question,
        disclosed,
        max_selections,
        answers,
        responses: Vec::new(),
        end_ts: None,
    })
}

impl StablePoll {
    /// Take in one event related to this poll (a response or an end event);
    /// anything else is ignored.
    pub fn apply(&mut self, event_type: &str, sender: &str, ts: u64, content: &Value) {
        if RESPONSE_TYPES.contains(&event_type) {
            // Standard: `m.selections: [..]`. Old layout: `<type>: {answers: [..]}`.
            let ids = first(
                content,
                &["m.selections", "org.matrix.msc3381.v2.selections"],
            )
            .or_else(|| {
                first(
                    content,
                    &["m.poll.response", "org.matrix.msc3381.poll.response"],
                )
                .and_then(|block| block.get("answers"))
            });
            let selections = ids
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let response = Response {
                sender: sender.to_owned(),
                ts,
                selections,
            };
            if !self.responses.contains(&response) {
                self.responses.push(response);
            }
        } else if END_TYPES.contains(&event_type) && sender == self.sender {
            // Only the author may close the poll; the earliest end counts.
            self.end_ts = Some(self.end_ts.map_or(ts, |current| current.min(ts)));
        }
    }

    /// Forget all responses (before re-reading them from the server).
    pub fn reset(&mut self) {
        self.responses.clear();
        self.end_ts = None;
    }

    pub fn is_ended(&self) -> bool {
        self.end_ts.is_some()
    }

    /// Everyone who responded, for display-name lookups.
    pub fn voter_ids(&self) -> impl Iterator<Item = &str> {
        self.responses
            .iter()
            .map(|response| response.sender.as_str())
    }

    /// Valid votes per answer id. Each user's latest response counts;
    /// responses after the end are ignored; unknown answers are dropped and
    /// a response is cut to the allowed number of selections.
    pub fn votes(&self) -> HashMap<String, Vec<String>> {
        let mut latest: HashMap<&str, &Response> = HashMap::new();
        for response in &self.responses {
            if self.end_ts.is_some_and(|end| response.ts > end) {
                continue;
            }
            let newer = latest
                .get(response.sender.as_str())
                .is_none_or(|known| response.ts >= known.ts);
            if newer {
                latest.insert(&response.sender, response);
            }
        }

        let mut votes: HashMap<String, Vec<String>> = HashMap::new();
        for response in latest.into_values() {
            let mut chosen: Vec<&String> = Vec::new();
            for id in &response.selections {
                let known = self.answers.iter().any(|(answer_id, _)| answer_id == id);
                if known && !chosen.contains(&id) {
                    chosen.push(id);
                }
            }
            for id in chosen.into_iter().take(self.max_selections as usize) {
                votes
                    .entry(id.clone())
                    .or_default()
                    .push(response.sender.clone());
            }
        }
        votes
    }
}

/// Whether an event of this type changes the result of a poll mothrix
/// tallies itself (a response or an end).
pub fn is_poll_update(event_type: &str) -> bool {
    RESPONSE_TYPES.contains(&event_type) || END_TYPES.contains(&event_type)
}

/// Event type and content for a vote on a poll of the given format.
pub fn response_event(
    format: PollFormat,
    poll_id: &str,
    answers: &[String],
) -> (&'static str, Value) {
    let relates_to = json!({"rel_type": "m.reference", "event_id": poll_id});
    let content = match format {
        PollFormat::Mixed => json!({
            "m.relates_to": relates_to,
            "m.poll.response": {"answers": answers},
        }),
        _ => json!({"m.relates_to": relates_to, "m.selections": answers}),
    };
    ("m.poll.response", content)
}

/// Event type and content that end a poll of the given format.
pub fn end_event(format: PollFormat, poll_id: &str, text: &str) -> (&'static str, Value) {
    let relates_to = json!({"rel_type": "m.reference", "event_id": poll_id});
    let content = match format {
        PollFormat::Mixed => json!({
            "m.relates_to": relates_to,
            "m.poll.end": {},
            "m.text": [{"body": text}],
            "org.matrix.msc1767.text": text,
            "body": text,
        }),
        _ => json!({"m.relates_to": relates_to, "m.text": [{"body": text}]}),
    };
    ("m.poll.end", content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> StablePoll {
        parse_start(
            "@amy:x",
            &json!({
                "m.text": [{"body": "Lunch?\n1. Pizza\n2. Sushi"}],
                "m.poll": {
                    "kind": "m.disclosed",
                    "max_selections": 1,
                    "question": {"m.text": [{"body": "Lunch?"}]},
                    "answers": [
                        {"m.id": "a", "m.text": [{"body": "Pizza"}]},
                        {"m.id": "b", "m.text": [{"mimetype": "text/html", "body": "<b>Sushi</b>"}, {"body": "Sushi"}]},
                    ],
                },
            }),
        )
        .expect("a poll")
    }

    fn respond(poll: &mut StablePoll, sender: &str, ts: u64, ids: &[&str]) {
        poll.apply("m.poll.response", sender, ts, &json!({"m.selections": ids}));
    }

    #[test]
    fn standard_poll_start_is_parsed() {
        let poll = start();
        assert_eq!(poll.format, PollFormat::Standard);
        assert_eq!(poll.question, "Lunch?");
        assert!(poll.disclosed);
        assert_eq!(poll.max_selections, 1);
        assert_eq!(
            poll.answers,
            [
                ("a".to_owned(), "Pizza".to_owned()),
                ("b".to_owned(), "Sushi".to_owned())
            ]
        );
    }

    #[test]
    fn other_spellings_are_accepted_and_non_polls_rejected() {
        let poll = parse_start(
            "@amy:x",
            &json!({"org.matrix.msc3381.v2.poll": {
                "question": {"org.matrix.msc1767.text": "Q"},
                "answers": [{"id": "1", "org.matrix.msc1767.text": "One"}],
            }}),
        )
        .expect("a poll");
        assert_eq!(poll.question, "Q");
        assert!(!poll.disclosed, "missing kind means results stay hidden");
        assert_eq!(poll.answers[0], ("1".to_owned(), "One".to_owned()));
        assert!(parse_start("@amy:x", &json!({"body": "hi", "msgtype": "m.text"})).is_none());
    }

    /// The layout the IServ messenger sends: standard event names around
    /// the old content structure.
    fn iserv_start() -> StablePoll {
        parse_start(
            "@amy:x",
            &json!({
                "body": "Lunch?",
                "m.text": [{"body": "Lunch?"}],
                "m.poll.start": {
                    "kind": "m.poll.disclosed",
                    "max_selections": 1,
                    "question": {
                        "body": "Lunch?",
                        "m.text": [{"body": "Lunch?"}],
                        "org.matrix.msc1767.text": "Lunch?",
                    },
                    "answers": [
                        {"id": "a", "m.text": [{"body": "Pizza"}], "org.matrix.msc1767.text": "Pizza"},
                        {"id": "b", "m.text": [{"body": "Sushi"}], "org.matrix.msc1767.text": "Sushi"},
                    ],
                },
            }),
        )
        .expect("a poll")
    }

    #[test]
    fn iserv_layout_is_parsed_and_tallied() {
        let mut poll = iserv_start();
        assert_eq!(poll.format, PollFormat::Mixed);
        assert_eq!(poll.question, "Lunch?");
        assert!(poll.disclosed);
        assert_eq!(
            poll.answers,
            [
                ("a".to_owned(), "Pizza".to_owned()),
                ("b".to_owned(), "Sushi".to_owned())
            ]
        );

        poll.apply(
            "m.poll.response",
            "@ben:x",
            10,
            &json!({
                "m.poll.response": {"answers": ["b"]},
                "m.relates_to": {"rel_type": "m.reference", "event_id": "$p"},
            }),
        );
        assert_eq!(poll.votes()["b"], ["@ben:x"]);
    }

    #[test]
    fn votes_and_end_events_follow_the_poll_format() {
        let answers = vec!["b".to_owned()];
        let (kind, content) = response_event(PollFormat::Mixed, "$p", &answers);
        assert_eq!(kind, "m.poll.response");
        assert_eq!(content["m.poll.response"]["answers"], json!(["b"]));
        assert_eq!(content["m.relates_to"]["event_id"], "$p");
        assert!(content.get("m.selections").is_none());
        // A vote we send must be readable by our own parser.
        let mut poll = iserv_start();
        poll.apply(kind, "@me:x", 5, &content);
        assert_eq!(poll.votes()["b"], ["@me:x"]);

        let (_, content) = response_event(PollFormat::Standard, "$p", &answers);
        assert_eq!(content["m.selections"], json!(["b"]));

        let (kind, content) = end_event(PollFormat::Mixed, "$p", "Closed");
        assert_eq!(kind, "m.poll.end");
        assert!(content["m.poll.end"].is_object());
        let mut poll = iserv_start();
        poll.apply(kind, "@amy:x", 9, &content);
        assert!(poll.is_ended());
    }

    #[test]
    fn the_latest_response_per_user_counts() {
        let mut poll = start();
        respond(&mut poll, "@ben:x", 10, &["a"]);
        respond(&mut poll, "@cleo:x", 11, &["a"]);
        respond(&mut poll, "@ben:x", 20, &["b"]);
        // The same event seen twice (sync and relations) counts once.
        respond(&mut poll, "@ben:x", 20, &["b"]);
        let votes = poll.votes();
        assert_eq!(votes["a"], ["@cleo:x"]);
        assert_eq!(votes["b"], ["@ben:x"]);
        assert_eq!(poll.responses.len(), 3);
    }

    #[test]
    fn invalid_selections_are_dropped_and_an_empty_response_retracts() {
        let mut poll = start();
        respond(&mut poll, "@ben:x", 10, &["nope", "b", "a"]);
        respond(&mut poll, "@cleo:x", 10, &["a"]);
        respond(&mut poll, "@cleo:x", 12, &[]);
        let votes = poll.votes();
        // Single choice: only the first valid selection counts.
        assert_eq!(votes["b"], ["@ben:x"]);
        assert!(!votes.contains_key("a"));
    }

    #[test]
    fn only_the_author_can_end_and_late_votes_do_not_count() {
        let mut poll = start();
        respond(&mut poll, "@ben:x", 10, &["a"]);
        poll.apply("m.poll.end", "@mallory:x", 15, &json!({}));
        assert!(!poll.is_ended());
        poll.apply("m.poll.end", "@amy:x", 20, &json!({}));
        assert!(poll.is_ended());
        respond(&mut poll, "@ben:x", 25, &["b"]);
        respond(&mut poll, "@cleo:x", 30, &["b"]);
        let votes = poll.votes();
        assert_eq!(votes["a"], ["@ben:x"]);
        assert!(!votes.contains_key("b"));
    }
}
