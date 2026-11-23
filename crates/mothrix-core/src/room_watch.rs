// SPDX-License-Identifier: MPL-2.0

//! What mothrix decides about the room on screen each time its timeline
//! changes: which read receipts to send, which polls to (re)read the
//! answers of, which pictures to fetch. Plain data in, plain data out;
//! `matrix::timeline_loop` reads the SDK's items into a [`Seen`] and
//! carries out the [`ToDo`].

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::{convert::StablePolls, stable_polls::StablePoll};

/// Pictures larger than this are not fetched for the conversation.
pub const MAX_PREVIEW_BYTES: u64 = 15 * 1024 * 1024;

/// What one snapshot of the timeline holds that matters here.
#[derive(Default)]
pub struct Seen {
    /// The newest event of the conversation shown.
    pub newest: Option<String>,
    /// (time, event id) of every message, and of the newest reply of each
    /// thread that starts here.
    pub with_threads: Vec<(u64, String)>,
    /// Polls the SDK tallies itself: (event id, how the SDK shows it now
    /// (`convert::poll_mark`), our own tally to start from).
    pub sdk_polls: Vec<(String, usize, StablePoll)>,
    /// Polls in the standard formats not tracked yet (see
    /// [`RoomWatch::tracks`]), parsed from their start event.
    pub new_standard_polls: Vec<(String, StablePoll)>,
    /// Picture messages: (event id, size in bytes when known).
    pub pictures: Vec<(String, Option<u64>)>,
}

/// What to do about one snapshot.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ToDo {
    /// Send a read receipt for the conversation shown.
    pub mark_read: bool,
    /// Send an unthreaded receipt ("read up to here, threads included")
    /// for this event.
    pub read_with_threads: Option<String>,
    /// Take away a "marked as unread" flag, if the room has one.
    pub clear_unread_flag: bool,
    /// Read the answers of these polls (again).
    pub load_polls: Vec<String>,
    /// Fetch these pictures.
    pub fetch_pictures: Vec<String>,
}

/// The state kept between snapshots of one open room.
pub struct RoomWatch {
    /// The room's own conversation (not a thread or the pinned view).
    live: bool,
    last_read: Option<String>,
    read_with_threads: Option<String>,
    /// Polls with the answers read so far, in every format.
    polls: StablePolls,
    /// How the SDK last showed each poll it tallies itself.
    sdk_marks: HashMap<String, usize>,
    pictures_asked: HashSet<String>,
}

impl RoomWatch {
    pub fn new(live: bool) -> Self {
        Self {
            live,
            last_read: None,
            read_with_threads: None,
            polls: StablePolls::new(),
            sdk_marks: HashMap::new(),
            pictures_asked: HashSet::new(),
        }
    }

    /// Whether this poll is followed already (no need to parse its start).
    pub fn tracks(&self, poll: &str) -> bool {
        self.polls.contains_key(poll)
    }

    pub fn polls(&self) -> &StablePolls {
        &self.polls
    }

    /// Take in a snapshot of the timeline.
    pub fn see(&mut self, seen: Seen) -> ToDo {
        let mut todo = ToDo::default();

        // Polls the SDK tallies: other clients may answer them in another
        // format, which the SDK ignores. Read all answers ourselves, again
        // whenever the SDK's view changes.
        for (id, mark, own) in seen.sdk_polls {
            if self.sdk_marks.insert(id.clone(), mark) == Some(mark) {
                continue;
            }
            self.polls.entry(id.clone()).or_insert(own);
            todo.load_polls.push(id);
        }
        for (id, poll) in seen.new_standard_polls {
            if self.polls.contains_key(&id) {
                continue;
            }
            self.polls.insert(id.clone(), poll);
            todo.load_polls.push(id);
        }

        for (id, size) in seen.pictures {
            let too_big = size.is_some_and(|size| size > MAX_PREVIEW_BYTES);
            if !too_big && self.pictures_asked.insert(id.clone()) {
                todo.fetch_pictures.push(id);
            }
        }

        // The room is on screen, so what arrived counts as read.
        if let Some(newest) = seen.newest
            && self.last_read.as_ref() != Some(&newest)
        {
            self.last_read = Some(newest);
            todo.mark_read = true;
        }
        // That receipt is for the conversation shown. Replies inside
        // threads count as unread separately on some servers, and another
        // client may have marked the room as unread by hand; neither would
        // ever clear. So also say "read up to here, threads included" for
        // the newest message known, wherever it is.
        if self.live {
            let mut newest: Option<&(u64, String)> = None;
            for candidate in &seen.with_threads {
                if newest.is_none_or(|known| candidate.0 >= known.0) {
                    newest = Some(candidate);
                }
            }
            if let Some((_, id)) = newest
                && self.read_with_threads.as_ref() != Some(id)
            {
                self.read_with_threads = Some(id.clone());
                todo.read_with_threads = Some(id.clone());
            }
            todo.clear_unread_flag = true;
        }
        todo
    }

    /// The answers of a poll were read: count them anew.
    pub fn poll_loaded(&mut self, poll: &str, events: &[Value]) {
        if let Some(poll) = self.polls.get_mut(poll) {
            poll.reset();
            for event in events {
                poll.apply(
                    event["type"].as_str().unwrap_or_default(),
                    event["sender"].as_str().unwrap_or_default(),
                    event["origin_server_ts"].as_u64().unwrap_or_default(),
                    &event["content"],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(newest: &str, with_threads: &[(u64, &str)]) -> Seen {
        Seen {
            newest: Some(newest.into()),
            with_threads: with_threads
                .iter()
                .map(|(at, id)| (*at, (*id).to_owned()))
                .collect(),
            ..Seen::default()
        }
    }

    #[test]
    fn receipts_go_out_once_per_newer_message() {
        let mut watch = RoomWatch::new(true);
        let todo = watch.see(seen("$2", &[(1, "$1"), (2, "$2")]));
        assert!(todo.mark_read);
        assert_eq!(todo.read_with_threads.as_deref(), Some("$2"));
        assert!(todo.clear_unread_flag);
        // The same snapshot again: nothing new to say.
        let todo = watch.see(seen("$2", &[(1, "$1"), (2, "$2")]));
        assert!(!todo.mark_read && todo.read_with_threads.is_none());
    }

    #[test]
    fn a_newer_thread_reply_gets_the_threads_included_receipt() {
        let mut watch = RoomWatch::new(true);
        watch.see(seen("$2", &[(1, "$1"), (2, "$2")]));
        // A reply in the thread of $1 arrived; the conversation itself
        // did not change.
        let todo = watch.see(seen("$2", &[(1, "$1"), (2, "$2"), (3, "$reply")]));
        assert!(!todo.mark_read);
        assert_eq!(todo.read_with_threads.as_deref(), Some("$reply"));
    }

    #[test]
    fn a_thread_view_leaves_the_room_wide_receipt_alone() {
        let mut watch = RoomWatch::new(false);
        let todo = watch.see(seen("$r2", &[(5, "$r2")]));
        assert!(todo.mark_read);
        assert_eq!(todo.read_with_threads, None);
        assert!(!todo.clear_unread_flag);
    }

    #[test]
    fn pictures_are_fetched_once_and_not_when_too_big() {
        let mut watch = RoomWatch::new(true);
        let pictures = || Seen {
            pictures: vec![
                ("$small".into(), Some(1000)),
                ("$unknown".into(), None),
                ("$huge".into(), Some(MAX_PREVIEW_BYTES + 1)),
            ],
            ..Seen::default()
        };
        assert_eq!(watch.see(pictures()).fetch_pictures, ["$small", "$unknown"]);
        assert!(watch.see(pictures()).fetch_pictures.is_empty());
    }

    #[test]
    fn polls_are_read_when_new_and_when_the_sdk_view_changes() {
        let mut watch = RoomWatch::new(true);
        let sdk = |mark| Seen {
            sdk_polls: vec![("$p".into(), mark, StablePoll::default())],
            ..Seen::default()
        };
        assert_eq!(watch.see(sdk(1)).load_polls, ["$p"]);
        assert!(watch.see(sdk(1)).load_polls.is_empty());
        assert_eq!(watch.see(sdk(2)).load_polls, ["$p"]);
        assert!(watch.tracks("$p"));

        let standard = Seen {
            new_standard_polls: vec![("$s".into(), StablePoll::default())],
            ..Seen::default()
        };
        assert_eq!(watch.see(standard).load_polls, ["$s"]);
        assert!(watch.tracks("$s"));
    }
}
