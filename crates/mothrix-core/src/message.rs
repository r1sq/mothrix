// SPDX-License-Identifier: MPL-2.0

//! The rules about one message, alike in both front ends: what can be
//! done to it, what "copy" takes, which poll answers a vote sends, what
//! a new poll needs, which mentions go out with a text.

use crate::model::{Item, ItemKind, Member, Poll};

/// MSC3381 allows at most 20 answers.
pub const MAX_POLL_ANSWERS: usize = 20;

/// What can be done to a message ([`offered`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Offered {
    pub reply: bool,
    pub react: bool,
    pub edit: bool,
    pub delete: bool,
    /// It is a picture or file to open.
    pub open_media: bool,
    /// Its text has a link to open.
    pub open_link: bool,
    pub copy: bool,
    /// Its text has a code block to copy alone.
    pub copy_code: bool,
    pub pin: bool,
    pub forward: bool,
    /// Its thread can be opened (not from inside a thread).
    pub thread: bool,
    /// It is a poll of one's own that still runs.
    pub end_poll: bool,
}

/// What can be done to `item`; `in_thread`: a thread (or the pinned
/// messages) is shown, not the room's conversation.
pub fn offered(item: &Item, in_thread: bool) -> Offered {
    let redacted = matches!(item.kind, ItemKind::Redacted);
    let message = !matches!(
        item.kind,
        ItemKind::State | ItemKind::DateDivider | ItemKind::ReadMarker
    );
    let readable = message && !matches!(item.kind, ItemKind::Redacted | ItemKind::Undecryptable);
    let alive = message && !redacted;
    let media = matches!(item.kind, ItemKind::Media);
    Offered {
        reply: alive,
        react: alive,
        edit: item.can_edit && matches!(item.kind, ItemKind::Text | ItemKind::Emote),
        delete: item.can_delete && alive,
        open_media: media,
        open_link: !media && readable && !crate::inline::parse_links(&item.body).1.is_empty(),
        copy: readable,
        copy_code: matches!(item.kind, ItemKind::Text | ItemKind::Notice)
            && !crate::codeblock::code_blocks(&item.body).is_empty(),
        pin: alive,
        forward: matches!(
            item.kind,
            ItemKind::Text | ItemKind::Notice | ItemKind::Emote | ItemKind::Media
        ),
        thread: alive && !in_thread,
        end_poll: matches!(&item.kind, ItemKind::Poll(poll) if item.is_own && !poll.is_ended),
    }
}

/// What "copy" puts on the clipboard for a message: its text, and for a
/// poll the question with its answers.
pub fn copy_text(item: &Item) -> String {
    let text = match &item.kind {
        ItemKind::Poll(poll) => {
            let mut text = poll.question.clone();
            for answer in &poll.answers {
                text.push_str("\n- ");
                text.push_str(&answer.text);
            }
            text
        }
        _ => item.body.clone(),
    };
    copyable(&text)
}

/// Characters that are obeyed rather than shown: control characters (C0,
/// C1, DEL) and the bidi embeddings, overrides and isolates, which let a
/// text show in another order than it has.
pub fn obeyed(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Other people's text as it goes to the clipboard: what is pasted into
/// a terminal or an editor next is typed there, so an escape sequence or
/// a text that reads otherwise than it is stays behind. Lines and tabs
/// are kept.
pub fn copyable(text: &str) -> String {
    text.chars()
        .filter(|c| matches!(c, '\n' | '\t') || !obeyed(*c))
        .collect()
}

/// Text for a plain terminal: what the command-line modes print of a
/// server's words (names, event types, error messages), where no screen
/// is drawn that would take care. An escape sequence there could write
/// the clipboard or answer as if typed. A control character shows as
/// "\u{fffd}", the bidi ones go; lines and tabs are kept.
pub fn for_terminal(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' | '\t' => Some(c),
            c if c.is_control() => Some('\u{fffd}'),
            c if obeyed(c) => None,
            c => Some(c),
        })
        .collect()
}

/// Whether reacting with `key` to the message `event_id` of `items` is an
/// emoji taken, to count among the user's favourites: a reaction given,
/// not one taken back, and an emoji. Others' reactions can be any text,
/// and joining one of those is no emoji taken.
pub fn takes_emoji(items: &[Item], event_id: &str, key: &str) -> bool {
    crate::emoji::is_emoji(key)
        && !items.iter().any(|item| {
            item.event_id == event_id
                && item
                    .reactions
                    .iter()
                    .any(|reaction| reaction.key == key && reaction.mine)
        })
}

/// The poll of the message `event_id` among `items`.
pub fn poll_of<'a>(items: &'a [Item], event_id: &str) -> Option<&'a Poll> {
    items.iter().find_map(|item| match &item.kind {
        ItemKind::Poll(poll) if item.event_id == event_id => Some(poll),
        _ => None,
    })
}

/// The selection to send after picking `answer_id`, or `None` when the pick
/// changes nothing (poll ended, same single choice, or the limit is reached).
pub fn vote(poll: &Poll, answer_id: &str) -> Option<Vec<String>> {
    if poll.is_ended {
        return None;
    }
    let mut selection: Vec<String> = poll
        .answers
        .iter()
        .filter(|answer| answer.self_voted)
        .map(|answer| answer.id.clone())
        .collect();
    let position = selection.iter().position(|id| id == answer_id);

    if poll.max_selections <= 1 {
        if position.is_some() {
            return None;
        }
        return Some(vec![answer_id.to_owned()]);
    }
    match position {
        Some(index) => {
            selection.remove(index);
        }
        None if selection.len() as u64 >= poll.max_selections => return None,
        None => selection.push(answer_id.to_owned()),
    }
    Some(selection)
}

/// The question and answers of a new poll, tidied; or what is wrong
/// with them.
pub fn new_poll(question: &str, answers: &[String]) -> Result<(String, Vec<String>), String> {
    let question = question.trim();
    let answers: Vec<String> = answers
        .iter()
        .map(|answer| answer.trim().to_owned())
        .filter(|answer| !answer.is_empty())
        .collect();
    if question.is_empty() || answers.len() < 2 {
        return Err("A poll needs a question and at least two answers.".to_owned());
    }
    if answers.len() > MAX_POLL_ANSWERS {
        return Err(format!(
            "A poll can have at most {MAX_POLL_ANSWERS} answers."
        ));
    }
    Ok((question.to_owned(), answers))
}

/// The people completed with @ who go out with `text` as mentioned: those
/// whose name survived the editing, each once.
pub fn mentions_kept(text: &str, mentions: Vec<Member>) -> Vec<Member> {
    let mut kept: Vec<Member> = Vec::new();
    for member in mentions {
        // The name as a word: "@Ann" is not in "@Anna".
        let tag = format!("@{}", member.name);
        let stands = text.match_indices(&tag).any(|(at, _)| {
            !text[at + tag.len()..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric)
        });
        if stands && !kept.iter().any(|known| known.user_id == member.user_id) {
            kept.push(member);
        }
    }
    kept
}

/// Whether what is in the message box is a command, not a message being
/// written. Spaces in front do not make " /recover KEY" a message: it is
/// not told to the room as typing, not sent to a grammar server, and its
/// words are not for the spell checker either.
pub fn is_command(text: &str) -> bool {
    text.trim_start().starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PollAnswer;

    #[test]
    fn only_an_emoji_given_counts_as_one_taken() {
        let mut reacted = item(ItemKind::Text, "hi");
        reacted.event_id = "$1".into();
        reacted.reactions = vec![
            crate::model::Reaction {
                key: "👍".into(),
                count: 2,
                mine: true,
                ..Default::default()
            },
            crate::model::Reaction {
                key: "🦋".into(),
                count: 1,
                ..Default::default()
            },
        ];
        let items = [reacted];
        // Given: one nobody gave yet, and one of someone else's.
        assert!(takes_emoji(&items, "$1", "🎉"));
        assert!(takes_emoji(&items, "$1", "🦋"));
        // Taken back.
        assert!(!takes_emoji(&items, "$1", "👍"));
        // Our own on another message is not this one's.
        assert!(takes_emoji(&items, "$2", "👍"));
        // A reaction of any text is no emoji.
        assert!(!takes_emoji(&items, "$1", "+1 from me"));
    }

    #[test]
    fn copy_leaves_out_what_a_terminal_or_editor_would_obey() {
        let body = "run\tthis:\n\u{1b}]52;c;ZXZpbA==\u{7}ls \u{202e}gnp.exe\u{2069}\r";
        assert_eq!(
            copy_text(&item(ItemKind::Text, body)),
            "run\tthis:\n]52;c;ZXZpbA==ls gnp.exe"
        );
        assert_eq!(copyable("plain ä 漢 👍"), "plain ä 漢 👍");
    }

    #[test]
    fn a_plain_terminal_gets_no_control_characters() {
        assert_eq!(
            for_terminal("M_FORBIDDEN: \u{1b}]52;c;eA==\u{7}no\u{9b}\u{202e}\n\tnext"),
            "M_FORBIDDEN: \u{fffd}]52;c;eA==\u{fffd}no\u{fffd}\n\tnext"
        );
        assert_eq!(for_terminal("Zoë's room"), "Zoë's room");
    }

    fn item(kind: ItemKind, body: &str) -> Item {
        Item {
            key: "$1".into(),
            event_id: "$1".into(),
            body: body.into(),
            kind,
            ..Item::default()
        }
    }

    fn poll(max_selections: u64, voted: &[&str]) -> Poll {
        Poll {
            max_selections,
            answers: ["a", "b", "c"]
                .iter()
                .map(|id| PollAnswer {
                    id: (*id).to_owned(),
                    text: id.to_uppercase(),
                    self_voted: voted.contains(id),
                    ..PollAnswer::default()
                })
                .collect(),
            ..Poll::default()
        }
    }

    #[test]
    fn each_kind_of_message_offers_what_works_on_it() {
        let text = offered(&item(ItemKind::Text, "hi"), false);
        assert!(text.reply && text.react && text.copy && text.pin && text.forward && text.thread);
        assert!(!text.edit && !text.delete && !text.open_link && !text.copy_code);

        let mut own = item(ItemKind::Text, "see https://x.example\n```\ncode\n```");
        own.can_edit = true;
        own.can_delete = true;
        let own = offered(&own, false);
        assert!(own.edit && own.delete && own.open_link && own.copy_code);

        // A file is opened and forwarded; what was written with it is
        // not edited as a message's text.
        let mut media = item(ItemKind::Media, "a.png");
        media.can_edit = true;
        let media = offered(&media, false);
        assert!(media.open_media && media.forward && !media.open_link && !media.edit);

        assert_eq!(
            offered(&item(ItemKind::Redacted, ""), false),
            Offered::default()
        );
        assert_eq!(
            offered(&item(ItemKind::State, "Amy joined"), false),
            Offered::default()
        );
        let lost = offered(&item(ItemKind::Undecryptable, ""), false);
        assert!(lost.reply && !lost.copy && !lost.forward);

        // Polls cannot be forwarded; one's own can be ended while it runs.
        let mut question = item(ItemKind::Poll(poll(1, &[])), "Lunch?");
        assert!(!offered(&question, false).forward && !offered(&question, false).end_poll);
        question.is_own = true;
        assert!(offered(&question, false).end_poll);
        question.kind = ItemKind::Poll(Poll {
            is_ended: true,
            ..poll(1, &[])
        });
        assert!(!offered(&question, false).end_poll);

        // Inside a thread there is no thread to open.
        assert!(!offered(&item(ItemKind::Text, "hi"), true).thread);
    }

    #[test]
    fn a_vote_toggles_within_what_the_poll_allows() {
        assert_eq!(vote(&poll(1, &["a"]), "b"), Some(vec!["b".to_owned()]));
        assert_eq!(vote(&poll(1, &["a"]), "a"), None);
        assert_eq!(
            vote(&poll(2, &["a"]), "b"),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(vote(&poll(2, &["a", "b"]), "c"), None);
        assert_eq!(vote(&poll(2, &["a", "b"]), "a"), Some(vec!["b".to_owned()]));
        let ended = Poll {
            is_ended: true,
            ..poll(1, &[])
        };
        assert_eq!(vote(&ended, "a"), None);
    }

    #[test]
    fn a_poll_is_copied_with_its_answers_and_found_by_its_message() {
        let mut question = item(ItemKind::Poll(poll(1, &[])), "");
        if let ItemKind::Poll(poll) = &mut question.kind {
            poll.question = "Lunch?".into();
        }
        assert_eq!(copy_text(&question), "Lunch?\n- A\n- B\n- C");
        assert_eq!(copy_text(&item(ItemKind::Text, "hi")), "hi");
        let items = [item(ItemKind::Text, "hi"), question];
        assert!(poll_of(&items[1..], "$1").is_some());
        assert!(poll_of(&items[..1], "$1").is_none());
        assert!(poll_of(&items, "$2").is_none());
    }

    #[test]
    fn a_new_poll_needs_a_question_and_two_to_twenty_answers() {
        let answers = |count: usize| -> Vec<String> {
            (0..count)
                .map(|index| format!(" answer {index} "))
                .collect()
        };
        assert_eq!(
            new_poll(" Where? ", &[" Here".into(), " ".into(), "There ".into()]),
            Ok(("Where?".to_owned(), vec!["Here".into(), "There".into()]))
        );
        assert!(new_poll("Where?", &answers(1)).is_err());
        assert!(new_poll("  ", &answers(2)).is_err());
        assert!(new_poll("Where?", &answers(20)).is_ok());
        assert!(new_poll("Where?", &answers(21)).is_err());
    }

    #[test]
    fn only_mentions_still_in_the_text_go_out_once_each() {
        let member = |name: &str| Member {
            user_id: format!("@{}:x", name.to_lowercase()),
            name: name.into(),
        };
        let kept = mentions_kept(
            "hello @Amy and @Amy",
            vec![member("Amy"), member("Ben"), member("Amy")],
        );
        assert_eq!(kept, [member("Amy")]);
    }
}
