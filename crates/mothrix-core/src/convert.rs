// SPDX-License-Identifier: MPL-2.0

//! Flatten matrix-sdk-ui timeline items into [`Item`]s the UI can draw.

use std::collections::{HashMap, HashSet};

use matrix_sdk::ruma::{
    UserId,
    events::{StateEventContentChange, poll::start::PollKind, room::message::MessageType},
};
use matrix_sdk_ui::timeline::{
    AnyOtherStateEventContentChange, EmbeddedEvent, EventSendState, MembershipChange, MsgLikeKind,
    PollState, TimelineDetails, TimelineItem, TimelineItemContent, VirtualTimelineItem,
};

use crate::{
    model::{Item, ItemKind, Poll, PollAnswer, Reaction},
    stable_polls::StablePoll,
};

/// Known display names by user id (room members and timeline senders).
pub type Names = HashMap<String, String>;

/// The part of `@local:server` people recognise when no display name is known.
pub fn localpart(user_id: &str) -> String {
    user_id
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or(user_id)
        .to_owned()
}

pub fn name_for(user_id: &str, names: &Names) -> String {
    names
        .get(user_id)
        .filter(|name| !name.is_empty())
        .cloned()
        .unwrap_or_else(|| localpart(user_id))
}

/// (sender name, first line) of a message shown inside another: the one
/// replied to, or the newest of a thread.
fn embedded_summary(event: &EmbeddedEvent, names: &Names) -> (String, String) {
    let sender = event.sender.to_string();
    let name = match &event.sender_profile {
        TimelineDetails::Ready(profile) => profile
            .display_name
            .clone()
            .unwrap_or_else(|| name_for(&sender, names)),
        _ => name_for(&sender, names),
    };
    let snippet = match &event.content {
        TimelineItemContent::MsgLike(content) => match &content.kind {
            MsgLikeKind::Message(message) => first_line(message.body()),
            MsgLikeKind::Poll(poll) => first_line(&poll.results().question),
            _ => String::new(),
        },
        _ => String::new(),
    };
    (name, snippet)
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_owned()
}

/// The items with every event once. Right after joining a room from an
/// invitation, the SDK's timeline can hold a message we send twice (the
/// server has it once); the later copy, which is the newer state, stays.
pub fn without_repeats(items: Vec<Item>) -> Vec<Item> {
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<Item> = items
        .into_iter()
        .rev()
        .filter(|item| item.event_id.is_empty() || seen.insert(item.event_id.clone()))
        .collect();
    kept.reverse();
    kept
}

/// User ids of the people `item` shows by name without having their
/// profile at hand: who voted in its poll, who read up to it, and whom
/// the message it replies to (or its thread's newest) is from.
pub fn people_shown(item: &TimelineItem) -> Vec<String> {
    let Some(event) = item.as_event() else {
        return Vec::new();
    };
    let mut people: Vec<String> = event
        .read_receipts()
        .keys()
        .map(ToString::to_string)
        .collect();
    if let Some(poll) = event.content().as_poll() {
        people.extend(poll.results().votes.into_values().flatten());
    }
    if let TimelineItemContent::MsgLike(content) = event.content() {
        if let Some(details) = &content.in_reply_to
            && let TimelineDetails::Ready(replied_to) = &details.event
        {
            people.push(replied_to.sender.to_string());
        }
        if let Some(summary) = &content.thread_summary
            && let TimelineDetails::Ready(latest) = &summary.latest_event
        {
            people.push(latest.sender.to_string());
        }
        if let MsgLikeKind::Message(message) = &content.kind
            && let Some(fallback) = reply_fallback(message.body())
        {
            people.push(fallback.sender);
        }
    }
    people
}

/// The quote some clients put in front of a reply's text (`> <@amy:x> what
/// she wrote`, then an empty line). The SDK takes it off when the event
/// says what it replies to; IServ's messenger sends it where the SDK does
/// not, and with "null" for a message that had no text.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReplyFallback<'a> {
    sender: String,
    /// The quoted text's first line; empty for "null".
    quoted: String,
    /// The reply's own text.
    pub(crate) rest: &'a str,
}

pub(crate) fn reply_fallback(body: &str) -> Option<ReplyFallback<'_>> {
    let first = body.lines().next()?;
    let mention = first
        .strip_prefix("> <")
        .or_else(|| first.strip_prefix("> * <"))?;
    let (sender, quoted) = mention.split_once('>')?;
    if !sender.starts_with('@') {
        return None;
    }
    let mut rest = body;
    while rest.starts_with('>') {
        rest = rest.split_once('\n').map_or("", |(_, after)| after);
    }
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    if rest.trim().is_empty() {
        return None;
    }
    let quoted = quoted.trim();
    Some(ReplyFallback {
        sender: sender.to_owned(),
        quoted: if quoted == "null" {
            String::new()
        } else {
            quoted.to_owned()
        },
        rest,
    })
}

/// Standard-format polls of the open room, by event id.
pub type StablePolls = HashMap<String, StablePoll>;

pub fn convert(
    item: &TimelineItem,
    own_user_id: Option<&UserId>,
    names: &Names,
    stable_polls: &StablePolls,
) -> Option<Item> {
    let key = item.unique_id().0.clone();

    let Some(event) = item.as_event() else {
        return match item.as_virtual()? {
            VirtualTimelineItem::DateDivider(ts) => Some(Item {
                key,
                kind: ItemKind::DateDivider,
                ts_ms: u64::from(ts.get()),
                ..Item::default()
            }),
            VirtualTimelineItem::ReadMarker => Some(Item {
                key,
                kind: ItemKind::ReadMarker,
                ..Item::default()
            }),
            _ => None,
        };
    };

    let sender_id = event.sender().to_string();
    let sender_name = match event.sender_profile() {
        TimelineDetails::Ready(profile) => profile
            .display_name
            .clone()
            .unwrap_or_else(|| name_for(&sender_id, names)),
        _ => name_for(&sender_id, names),
    };

    let mut out = Item {
        key,
        event_id: event
            .event_id()
            .map(ToString::to_string)
            .unwrap_or_default(),
        sender_id: sender_id.clone(),
        sender_name: sender_name.clone(),
        ts_ms: u64::from(event.timestamp().get()),
        is_own: event.is_own(),
        can_edit: event.is_editable(),
        read_by: event
            .read_receipts()
            .keys()
            .filter(|reader| Some(reader.as_ref()) != own_user_id && *reader != event.sender())
            .map(|reader| name_for(reader.as_str(), names))
            .collect(),
        // Moderators get more in `publish_timeline`.
        can_delete: event.is_own(),
        pending: matches!(event.send_state(), Some(EventSendState::NotSentYet { .. })),
        failed: matches!(
            event.send_state(),
            Some(EventSendState::SendingFailed { .. })
        ),
        ..Item::default()
    };

    match event.content() {
        TimelineItemContent::MsgLike(content) => {
            out.reactions = content
                .reactions
                .iter()
                .map(|(key, senders)| Reaction {
                    key: key.clone(),
                    count: senders.len(),
                    mine: own_user_id.is_some_and(|own| senders.contains_key(own)),
                    senders: senders
                        .keys()
                        .filter(|sender| Some(sender.as_ref()) != own_user_id)
                        .map(|sender| name_for(sender.as_str(), names))
                        .collect(),
                })
                .collect();

            if let Some(details) = &content.in_reply_to {
                out.reply_to = details.event_id.to_string();
                if let TimelineDetails::Ready(replied_to) = &details.event {
                    out.reply = Some(embedded_summary(replied_to, names));
                }
            }
            if let Some(summary) = &content.thread_summary {
                let latest = match &summary.latest_event {
                    TimelineDetails::Ready(latest) => Some(embedded_summary(latest, names)),
                    _ => None,
                };
                out.thread = Some((summary.num_replies, latest));
            }

            match &content.kind {
                MsgLikeKind::Message(message) => {
                    out.edited = message.is_edited();
                    out.body = message.body().to_owned();
                    if let Some(fallback) = reply_fallback(message.body()) {
                        out.body = fallback.rest.to_owned();
                        if out.reply.is_none() {
                            out.reply = Some((name_for(&fallback.sender, names), fallback.quoted));
                        }
                    }
                    // A file's own words, apart from its name (Matrix sends
                    // a caption as the body next to `filename`).
                    out.caption = match message.msgtype() {
                        MessageType::Image(content) => content.caption(),
                        MessageType::Video(content) => content.caption(),
                        MessageType::Audio(content) => content.caption(),
                        MessageType::File(content) => content.caption(),
                        _ => None,
                    }
                    .map(str::to_owned)
                    .unwrap_or_default();
                    out.kind = match message.msgtype() {
                        MessageType::Emote(_) => ItemKind::Emote,
                        MessageType::Notice(_) => ItemKind::Notice,
                        MessageType::Image(_) => {
                            out.image = true;
                            media(&mut out.body, "image")
                        }
                        MessageType::Video(_) => media(&mut out.body, "video"),
                        MessageType::Audio(audio) if audio.voice.is_some() => {
                            // A voice message: its length, not its file name.
                            let length = audio
                                .info
                                .as_ref()
                                .and_then(|info| info.duration)
                                .or(audio.audio.as_ref().map(|details| details.duration));
                            out.body = length.map(clock).unwrap_or_default();
                            media(&mut out.body, "voice")
                        }
                        MessageType::Audio(_) => media(&mut out.body, "audio"),
                        MessageType::File(_) => media(&mut out.body, "file"),
                        MessageType::Location(_) => media(&mut out.body, "location"),
                        _ => ItemKind::Text,
                    };
                }
                MsgLikeKind::Sticker(sticker) => {
                    out.body = sticker.content().body.clone();
                    out.image = true;
                    out.kind = media(&mut out.body, "sticker");
                }
                MsgLikeKind::Poll(poll) => {
                    let poll = summarize_poll(
                        poll,
                        stable_polls.get(&out.event_id),
                        own_user_id.map(UserId::as_str),
                        names,
                    );
                    out.body = poll.question.clone();
                    out.kind = ItemKind::Poll(poll);
                }
                MsgLikeKind::Redacted => out.kind = ItemKind::Redacted,
                MsgLikeKind::UnableToDecrypt(_) => out.kind = ItemKind::Undecryptable,
                MsgLikeKind::Other(other) => match stable_polls.get(&out.event_id) {
                    Some(stable) => {
                        let poll = stable_poll_view(stable, own_user_id, names);
                        out.body = poll.question.clone();
                        out.kind = ItemKind::Poll(poll);
                    }
                    None => {
                        out.kind = ItemKind::Notice;
                        out.body = format!("[unsupported event: {}]", other.event_type());
                    }
                },
                _ => {
                    out.kind = ItemKind::Notice;
                    out.body = "[unsupported event]".to_owned();
                }
            }
        }
        TimelineItemContent::MembershipChange(change) => {
            let target = change
                .display_name()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| name_for(change.user_id().as_str(), names));
            out.kind = ItemKind::State;
            out.body = match change.change() {
                Some(MembershipChange::Joined | MembershipChange::InvitationAccepted) => {
                    format!("{target} joined")
                }
                Some(MembershipChange::Left) => format!("{target} left"),
                Some(MembershipChange::Invited) => format!("{sender_name} invited {target}"),
                Some(MembershipChange::Kicked) => format!("{sender_name} removed {target}"),
                Some(MembershipChange::Banned | MembershipChange::KickedAndBanned) => {
                    format!("{sender_name} banned {target}")
                }
                Some(MembershipChange::Unbanned) => format!("{sender_name} unbanned {target}"),
                Some(MembershipChange::InvitationRejected) => {
                    format!("{target} declined the invitation")
                }
                Some(MembershipChange::InvitationRevoked) => {
                    format!("{sender_name} withdrew {target}'s invitation")
                }
                Some(MembershipChange::Knocked) => format!("{target} asked to join"),
                _ => format!("{target}'s membership changed"),
            };
        }
        TimelineItemContent::ProfileChange(change) => {
            out.kind = ItemKind::State;
            out.body = match change.displayname_change() {
                Some(names_change) => match (&names_change.old, &names_change.new) {
                    (Some(old), Some(new)) => format!("{old} is now {new}"),
                    (None, Some(new)) => format!("{sender_name} set their name to {new}"),
                    _ => format!("{sender_name} removed their display name"),
                },
                None => format!("{sender_name} changed their avatar"),
            };
        }
        TimelineItemContent::OtherState(state) => {
            out.kind = ItemKind::State;
            out.body =
                match state.content() {
                    AnyOtherStateEventContentChange::RoomName(
                        StateEventContentChange::Original { content, .. },
                    ) => format!("{sender_name} named the room {}", content.name),
                    AnyOtherStateEventContentChange::RoomTopic(
                        StateEventContentChange::Original { content, .. },
                    ) => format!(
                        "{sender_name} set the topic: {}",
                        first_line(&content.topic)
                    ),
                    AnyOtherStateEventContentChange::RoomCreate(_) => {
                        format!("{sender_name} created the room")
                    }
                    AnyOtherStateEventContentChange::RoomEncryption(_) => {
                        format!("{sender_name} turned on encryption")
                    }
                    other => format!("{sender_name} changed {}", other.event_type()),
                };
        }
        TimelineItemContent::CallInvite | TimelineItemContent::RtcNotification { .. } => {
            out.kind = ItemKind::State;
            out.body = format!("{sender_name} started a call");
        }
        // Events the SDK could not parse are normally left out, except
        // polls whose content does not follow the standard (see
        // `stable_polls`), which we read ourselves.
        TimelineItemContent::FailedToParseMessageLike { .. } => {
            let poll = stable_poll_view(stable_polls.get(&out.event_id)?, own_user_id, names);
            out.body = poll.question.clone();
            out.kind = ItemKind::Poll(poll);
        }
        _ => return None,
    }

    Some(out)
}

fn stable_poll_view(stable: &StablePoll, own_user_id: Option<&UserId>, names: &Names) -> Poll {
    let mut poll = build_poll(
        &stable.question,
        stable.disclosed,
        stable.max_selections,
        stable.is_ended(),
        &stable.answers,
        &stable.votes(),
        own_user_id.map(UserId::as_str),
        names,
    );
    poll.format = stable.format;
    poll
}

/// A length as minutes and seconds: `0:07`, `12:30`.
fn clock(length: std::time::Duration) -> String {
    let seconds = length.as_secs_f64().round() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn media(body: &mut String, label: &str) -> ItemKind {
    *body = if body.trim().is_empty() {
        format!("[{label}]")
    } else {
        format!("[{label}] {body}")
    };
    ItemKind::Media
}

/// The poll as our own tally sees it, for a poll the SDK tallies too
/// (the older format). Other clients may answer such a poll in another
/// format, which the SDK ignores; our tally reads every format.
pub fn own_tally_for(sender: &str, poll: &PollState) -> StablePoll {
    let results = poll.results();
    StablePoll {
        sender: sender.to_owned(),
        format: Default::default(),
        question: results.question.clone(),
        disclosed: matches!(results.kind, PollKind::Disclosed),
        max_selections: results.max_selections,
        answers: results
            .answers
            .iter()
            .map(|answer| (answer.id.clone(), answer.text.clone()))
            .collect(),
        responses: Vec::new(),
        end_ts: None,
    }
}

/// A number that changes whenever the SDK's view of a poll changes.
pub fn poll_mark(poll: &PollState) -> usize {
    let results = poll.results();
    let votes: usize = results.votes.values().map(|voters| voters.len()).sum();
    votes * 2 + usize::from(results.end_time.is_some())
}

/// Combine the SDK's votes with our own tally: whoever our tally has an
/// answer from is taken from there (it knows every format and which answer
/// is the latest); everyone else stays as the SDK has them.
pub fn merge_votes(
    sdk: &HashMap<String, Vec<String>>,
    own: &StablePoll,
) -> HashMap<String, Vec<String>> {
    let answered: HashSet<&str> = own.voter_ids().collect();
    let mut votes: HashMap<String, Vec<String>> = sdk
        .iter()
        .map(|(id, voters)| {
            let kept = voters
                .iter()
                .filter(|voter| !answered.contains(voter.as_str()))
                .cloned()
                .collect();
            (id.clone(), kept)
        })
        .collect();
    for (id, voters) in own.votes() {
        votes.entry(id).or_default().extend(voters);
    }
    votes
}

fn summarize_poll(
    poll: &PollState,
    own: Option<&StablePoll>,
    own_user_id: Option<&str>,
    names: &Names,
) -> Poll {
    let results = poll.results();
    let answers: Vec<(String, String)> = results
        .answers
        .iter()
        .map(|answer| (answer.id.clone(), answer.text.clone()))
        .collect();
    let sdk_votes: HashMap<String, Vec<String>> = results
        .votes
        .iter()
        .map(|(id, voters)| (id.clone(), voters.to_vec()))
        .collect();
    let votes = match own {
        Some(own) => merge_votes(&sdk_votes, own),
        None => sdk_votes,
    };
    build_poll(
        &results.question,
        matches!(results.kind, PollKind::Disclosed),
        results.max_selections,
        // Ended by its author: the own tally checks who sent the end,
        // the SDK's takes anyone's. Without an own tally (not loaded yet)
        // the SDK's word has to do.
        own.map_or(results.end_time.is_some(), StablePoll::is_ended),
        &answers,
        &votes,
        own_user_id,
        names,
    )
}

/// Pure aggregation step, kept free of SDK types so it can be unit tested.
/// `votes` maps answer id to voter user ids and is expected to hold valid
/// votes only (the SDK applies the MSC3381 rules).
#[allow(clippy::too_many_arguments)]
pub fn build_poll(
    question: &str,
    is_disclosed: bool,
    max_selections: u64,
    is_ended: bool,
    answers: &[(String, String)],
    votes: &HashMap<String, Vec<String>>,
    own_user_id: Option<&str>,
    names: &Names,
) -> Poll {
    let results_visible = is_disclosed || is_ended;
    let mut distinct: HashSet<&str> = HashSet::new();
    let mut top = 0usize;
    for (id, _) in answers {
        if let Some(voters) = votes.get(id) {
            top = top.max(voters.len());
            distinct.extend(voters.iter().map(String::as_str));
        }
    }

    let answers = answers
        .iter()
        .map(|(id, text)| {
            let all: &[String] = votes.get(id).map(Vec::as_slice).unwrap_or(&[]);
            // Hidden results: keep nobody but ourselves, so the UI cannot
            // leak who voted for what before the poll ends.
            let mut voters: Vec<String> = all
                .iter()
                .filter(|voter| results_visible || Some(voter.as_str()) == own_user_id)
                .map(|voter| name_for(voter, names))
                .collect();
            voters.sort_by_key(|name| name.to_lowercase());
            PollAnswer {
                id: id.clone(),
                text: text.clone(),
                voters,
                self_voted: own_user_id.is_some_and(|own| all.iter().any(|voter| voter == own)),
                is_winner: is_ended && top > 0 && all.len() == top,
            }
        })
        .collect();

    Poll {
        question: question.to_owned(),
        max_selections: max_selections.max(1),
        is_ended,
        results_visible,
        voter_count: distinct.len() as u64,
        answers,
        format: Default::default(),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_voice_message_is_shown_by_its_length() {
        use std::time::Duration;
        assert_eq!(super::clock(Duration::from_millis(3414)), "0:03");
        assert_eq!(super::clock(Duration::from_millis(750_000)), "12:30");
    }

    use super::*;

    #[test]
    fn a_quote_in_front_of_a_reply_is_taken_off_and_null_is_nothing() {
        let body = "> <@6f1c-77aa:iserv.example> null\n\nSounds good";
        assert_eq!(
            reply_fallback(body),
            Some(ReplyFallback {
                sender: "@6f1c-77aa:iserv.example".into(),
                quoted: String::new(),
                rest: "Sounds good",
            })
        );
        let body = "> <@amy:x> Lunch at noon?\n> second line\n\nYes\nsure";
        let fallback = reply_fallback(body).unwrap();
        assert_eq!(fallback.quoted, "Lunch at noon?");
        assert_eq!(fallback.rest, "Yes\nsure");
        // A quote of one's own, or nothing after it, stays as written.
        assert_eq!(reply_fallback("> just a quote\n\ntext"), None);
        assert_eq!(reply_fallback("> <@amy:x> hi"), None);
        assert_eq!(reply_fallback("hello"), None);
    }

    #[test]
    fn votes_in_another_format_are_merged_into_the_sdk_tally() {
        // Amy and Ben answered in the poll's own format (the SDK saw them);
        // Cleo and, later, Ben answered in another one (only we saw those).
        let sdk: HashMap<String, Vec<String>> = HashMap::from([
            (
                "a".to_owned(),
                vec!["@amy:x".to_owned(), "@ben:x".to_owned()],
            ),
            ("b".to_owned(), vec![]),
        ]);
        let mut own = StablePoll {
            sender: "@amy:x".into(),
            max_selections: 1,
            answers: vec![("a".into(), "Pizza".into()), ("b".into(), "Sushi".into())],
            ..StablePoll::default()
        };
        let iserv = |answer: &str| serde_json::json!({"m.poll.response": {"answers": [answer]}});
        own.apply("m.poll.response", "@cleo:x", 10, &iserv("b"));
        own.apply("m.poll.response", "@ben:x", 20, &iserv("b"));

        let mut merged = merge_votes(&sdk, &own);
        merged.values_mut().for_each(|voters| voters.sort());
        assert_eq!(merged["a"], ["@amy:x"]);
        assert_eq!(merged["b"], ["@ben:x", "@cleo:x"]);
    }

    fn answers() -> Vec<(String, String)> {
        vec![("a".into(), "Pizza".into()), ("b".into(), "Sushi".into())]
    }

    fn votes(entries: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(id, voters)| {
                (
                    (*id).to_owned(),
                    voters.iter().map(|v| (*v).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn open_disclosed_poll_names_every_voter() {
        let names: Names = [("@amy:x".to_owned(), "Amy Winter".to_owned())].into();
        let poll = build_poll(
            "Lunch?",
            true,
            1,
            false,
            &answers(),
            &votes(&[("a", &["@zoe:x", "@amy:x"]), ("b", &["@me:x"])]),
            Some("@me:x"),
            &names,
        );
        assert_eq!(poll.voter_count, 3);
        assert_eq!(poll.answers[0].voters, ["Amy Winter", "zoe"]);
        assert!(poll.answers[1].self_voted);
        assert!(poll.answers.iter().all(|answer| !answer.is_winner));
    }

    #[test]
    fn hidden_results_only_show_our_own_vote_until_the_end() {
        let all = votes(&[("a", &["@amy:x", "@me:x"]), ("b", &["@zoe:x"])]);
        let open = build_poll(
            "Q",
            false,
            1,
            false,
            &answers(),
            &all,
            Some("@me:x"),
            &Names::new(),
        );
        assert!(!open.results_visible);
        assert_eq!(open.answers[0].voters, ["me"]);
        assert!(open.answers[1].voters.is_empty());
        assert_eq!(open.voter_count, 3);

        let ended = build_poll(
            "Q",
            false,
            1,
            true,
            &answers(),
            &all,
            Some("@me:x"),
            &Names::new(),
        );
        assert!(ended.results_visible);
        assert_eq!(ended.answers[0].voters, ["amy", "me"]);
        assert!(ended.answers[0].is_winner);
        assert!(!ended.answers[1].is_winner);
    }

    #[test]
    fn localpart_strips_sigil_and_server() {
        assert_eq!(localpart("@amy:example.org"), "amy");
        assert_eq!(localpart("weird"), "weird");
    }

    #[test]
    fn an_event_listed_twice_is_shown_once() {
        let item = |key: &str, event_id: &str, body: &str| Item {
            key: key.into(),
            event_id: event_id.into(),
            body: body.into(),
            ..Item::default()
        };
        let items = vec![
            item("a", "$1", "first"),
            item("b", "", "sending"),
            item("c", "$2", "old"),
            item("d", "", "sending too"),
            item("e", "$2", "new"),
        ];
        let kept: Vec<String> = without_repeats(items)
            .into_iter()
            .map(|item| item.key)
            .collect();
        assert_eq!(kept, ["a", "b", "d", "e"]);
    }
}
