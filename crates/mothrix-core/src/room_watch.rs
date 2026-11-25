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

/// What of a room the conversation followed shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// The room's own conversation.
    Room,
    /// One thread of it.
    Thread,
    /// Its pinned messages: old ones, picked out. Looking at them reads
    /// nothing.
    Pinned,
}

/// What one snapshot of the timeline holds that matters here.
#[derive(Default)]
pub struct Seen {
    /// The newest event of the conversation shown.
    pub newest: Option<String>,
    /// The time each message of the conversation says it was sent at.
    pub times: Vec<u64>,
    /// (time, event id) of the newest reply of each thread that starts
    /// here.
    pub replies: Vec<(u64, String)>,
    /// The time now, as the times above count it (milliseconds).
    pub now: u64,
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
    shown: Shown,
    last_read: Option<String>,
    read_with_threads: Option<String>,
    /// Polls with the answers read so far, in every format: what is
    /// shown of them.
    polls: StablePolls,
    /// Polls the SDK tallies too, whose answers we have not read yet.
    /// Until then the SDK's word is all there is, also on whether the
    /// poll has ended: an empty tally of ours would say it has not.
    unread: StablePolls,
    /// Polls being read: one reading per poll at a time. `true`: it
    /// changed since that reading began, and is read again after it.
    reading: HashMap<String, bool>,
    /// How the SDK last showed each poll it tallies itself.
    sdk_marks: HashMap<String, usize>,
    pictures_asked: HashSet<String>,
}

impl RoomWatch {
    pub fn new(shown: Shown) -> Self {
        Self {
            shown,
            last_read: None,
            read_with_threads: None,
            polls: StablePolls::new(),
            unread: StablePolls::new(),
            reading: HashMap::new(),
            sdk_marks: HashMap::new(),
            pictures_asked: HashSet::new(),
        }
    }

    /// Whether this poll is followed already (no need to parse its start).
    pub fn tracks(&self, poll: &str) -> bool {
        self.polls.contains_key(poll) || self.unread.contains_key(poll)
    }

    /// Our own tallies, to show: of the standard-format polls, and of
    /// those the SDK tallies too once their answers were read.
    pub fn polls(&self) -> &StablePolls {
        &self.polls
    }

    /// The poll's answers are to be read (again). Returns whether to
    /// start a reading now; while one is under way the next waits for it
    /// ([`RoomWatch::poll_loaded`]).
    fn read_poll(&mut self, poll: &str) -> bool {
        match self.reading.get_mut(poll) {
            Some(again) => {
                *again = true;
                false
            }
            None => {
                self.reading.insert(poll.to_owned(), false);
                true
            }
        }
    }

    /// A poll got a new answer or was ended. Returns whether to start
    /// reading its answers now.
    pub fn poll_changed(&mut self, poll: &str) -> bool {
        self.tracks(poll) && self.read_poll(poll)
    }

    /// Take in a snapshot of the timeline.
    pub fn see(&mut self, seen: Seen) -> ToDo {
        let mut todo = ToDo::default();
        let with_threads = newest_with_threads(&seen);

        // Polls the SDK tallies: other clients may answer them in another
        // format, which the SDK ignores. Read all answers ourselves, again
        // whenever the SDK's view changes.
        for (id, mark, own) in seen.sdk_polls {
            if self.sdk_marks.insert(id.clone(), mark) == Some(mark) {
                continue;
            }
            if !self.polls.contains_key(&id) {
                self.unread.entry(id.clone()).or_insert(own);
            }
            if self.read_poll(&id) {
                todo.load_polls.push(id);
            }
        }
        for (id, poll) in seen.new_standard_polls {
            if self.tracks(&id) {
                continue;
            }
            self.polls.insert(id.clone(), poll);
            if self.read_poll(&id) {
                todo.load_polls.push(id);
            }
        }

        for (id, size) in seen.pictures {
            let too_big = size.is_some_and(|size| size > MAX_PREVIEW_BYTES);
            if !too_big && self.pictures_asked.insert(id.clone()) {
                todo.fetch_pictures.push(id);
            }
        }

        // The room is on screen, so what arrived counts as read. Not
        // among the pinned messages: the newest of them is an old message,
        // and a receipt for it would be one for the room.
        if self.shown != Shown::Pinned
            && let Some(newest) = &seen.newest
            && self.last_read.as_ref() != Some(newest)
        {
            self.last_read = Some(newest.clone());
            todo.mark_read = true;
        }
        // That receipt is for the conversation shown. Replies inside
        // threads count as unread separately on some servers, and another
        // client may have marked the room as unread by hand; neither would
        // ever clear. So also say "read up to here, threads included" for
        // the newest message known, wherever it is.
        if self.shown == Shown::Room {
            if let Some(id) = with_threads
                && self.read_with_threads.as_ref() != Some(&id)
            {
                self.read_with_threads = Some(id.clone());
                todo.read_with_threads = Some(id);
            }
            todo.clear_unread_flag = true;
        }
        todo
    }

    /// The receipt for the conversation shown ([`ToDo::mark_read`]) did
    /// not go out: the next snapshot asks for it again.
    pub fn read_failed(&mut self) {
        self.last_read = None;
    }

    /// The receipt with threads ([`ToDo::read_with_threads`]) did not go
    /// out: the next snapshot asks for it again.
    pub fn read_with_threads_failed(&mut self) {
        self.read_with_threads = None;
    }

    /// The answers of a poll were read, all of them: count them anew.
    /// Returns whether to read them once more now, because the poll
    /// changed while they were read.
    pub fn poll_loaded(&mut self, poll: &str, events: &[Value]) -> bool {
        if let Some(tally) = self.unread.remove(poll) {
            self.polls.insert(poll.to_owned(), tally);
        }
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
        match self.reading.remove(poll) {
            Some(true) => self.read_poll(poll),
            _ => false,
        }
    }
}

/// How far a server's clock may be ahead of this computer's for a time
/// it gives to be believed, in milliseconds.
const CLOCKS_APART: u64 = 5 * 60 * 1000;

/// The event the threads-included receipt is for: the newest of the
/// conversation, or the reply in a thread that is newer still. The
/// conversation's newest is the last in it, whatever time it claims; a
/// reply is compared by the time it says it was sent at, and one that
/// says it comes from the future is not believed. Going by the largest
/// time alone, one message dated next year would be "the newest" for a
/// year, and no receipt would follow it.
///
/// "The future" begins some minutes from now ([`CLOCKS_APART`]): the
/// times are the senders' servers', and with this computer's clock a
/// little behind, a reply that just came in would be from the future
/// and get no receipt.
fn newest_with_threads(seen: &Seen) -> Option<String> {
    let believed = |at: &u64| *at <= seen.now.saturating_add(CLOCKS_APART);
    let newest_at = seen.times.iter().copied().filter(believed).max();
    let reply = seen
        .replies
        .iter()
        .filter(|(at, _)| believed(at))
        .max_by_key(|(at, _)| *at);
    match (&seen.newest, reply) {
        (Some(_), Some((at, id))) if newest_at.is_none_or(|newest| *at > newest) => {
            Some(id.clone())
        }
        (Some(newest), _) => Some(newest.clone()),
        (None, reply) => reply.map(|(_, id)| id.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A conversation of these (time, id) messages, with these newest
    /// replies in its threads, looked at when it is 100 o'clock.
    fn seen(messages: &[(u64, &str)], replies: &[(u64, &str)]) -> Seen {
        Seen {
            newest: messages.last().map(|(_, id)| (*id).to_owned()),
            times: messages.iter().map(|(at, _)| *at).collect(),
            replies: replies
                .iter()
                .map(|(at, id)| (*at, (*id).to_owned()))
                .collect(),
            now: 100,
            ..Seen::default()
        }
    }

    #[test]
    fn a_receipt_that_did_not_go_out_is_sent_again() {
        let mut watch = RoomWatch::new(Shown::Room);
        let snapshot = || seen(&[(1, "$1"), (2, "$2")], &[]);
        assert!(watch.see(snapshot()).mark_read);
        watch.read_failed();
        let todo = watch.see(snapshot());
        assert!(todo.mark_read && todo.read_with_threads.is_none());
        watch.read_with_threads_failed();
        let todo = watch.see(snapshot());
        assert!(!todo.mark_read);
        assert_eq!(todo.read_with_threads.as_deref(), Some("$2"));
    }

    #[test]
    fn receipts_go_out_once_per_newer_message() {
        let mut watch = RoomWatch::new(Shown::Room);
        let todo = watch.see(seen(&[(1, "$1"), (2, "$2")], &[]));
        assert!(todo.mark_read);
        assert_eq!(todo.read_with_threads.as_deref(), Some("$2"));
        assert!(todo.clear_unread_flag);
        // The same snapshot again: nothing new to say.
        let todo = watch.see(seen(&[(1, "$1"), (2, "$2")], &[]));
        assert!(!todo.mark_read && todo.read_with_threads.is_none());
    }

    #[test]
    fn a_newer_thread_reply_gets_the_threads_included_receipt() {
        let mut watch = RoomWatch::new(Shown::Room);
        watch.see(seen(&[(1, "$1"), (2, "$2")], &[]));
        // A reply in the thread of $1 arrived; the conversation itself
        // did not change.
        let todo = watch.see(seen(&[(1, "$1"), (2, "$2")], &[(3, "$reply")]));
        assert!(!todo.mark_read);
        assert_eq!(todo.read_with_threads.as_deref(), Some("$reply"));
        // An older reply is behind the conversation's newest.
        let mut watch = RoomWatch::new(Shown::Room);
        let todo = watch.see(seen(&[(1, "$1"), (5, "$2")], &[(3, "$reply")]));
        assert_eq!(todo.read_with_threads.as_deref(), Some("$2"));
    }

    #[test]
    fn a_message_dated_in_the_future_does_not_stop_the_receipts() {
        // It is 100; "$late" says it was sent hours later.
        let (late, later) = (100 + 20 * CLOCKS_APART, 100 + 40 * CLOCKS_APART);
        let mut watch = RoomWatch::new(Shown::Room);
        let todo = watch.see(seen(&[(1, "$1"), (late, "$late")], &[]));
        assert_eq!(todo.read_with_threads.as_deref(), Some("$late"));
        // What comes after it is the newest, though its time is smaller.
        let after = [(1, "$1"), (late, "$late"), (50, "$3")];
        let todo = watch.see(seen(&after, &[]));
        assert_eq!(todo.read_with_threads.as_deref(), Some("$3"));
        // Nor does a reply from the future stay the newest for ever.
        let todo = watch.see(seen(&after, &[(later, "$reply")]));
        assert_eq!(todo.read_with_threads, None);
        let more = [(1, "$1"), (late, "$late"), (50, "$3"), (60, "$4")];
        let todo = watch.see(seen(&more, &[(later, "$reply"), (55, "$other")]));
        assert_eq!(todo.read_with_threads.as_deref(), Some("$4"));
    }

    #[test]
    fn a_reply_a_little_ahead_of_our_clock_still_gets_its_receipt() {
        // Our clock is half a minute behind the server's: the reply that
        // just came in says it was sent at 30 100, and it is 100 here.
        let mut watch = RoomWatch::new(Shown::Room);
        watch.see(seen(&[(1, "$1"), (2, "$2")], &[]));
        let todo = watch.see(seen(&[(1, "$1"), (2, "$2")], &[(30_100, "$reply")]));
        assert_eq!(todo.read_with_threads.as_deref(), Some("$reply"));
    }

    #[test]
    fn a_thread_view_leaves_the_room_wide_receipt_alone() {
        let mut watch = RoomWatch::new(Shown::Thread);
        let todo = watch.see(seen(&[(5, "$r2")], &[]));
        assert!(todo.mark_read);
        assert_eq!(todo.read_with_threads, None);
        assert!(!todo.clear_unread_flag);
    }

    #[test]
    fn looking_at_the_pinned_messages_reads_nothing() {
        let mut watch = RoomWatch::new(Shown::Pinned);
        let todo = watch.see(seen(&[(1, "$old"), (2, "$pinned")], &[]));
        assert_eq!(todo, ToDo::default());
    }

    #[test]
    fn pictures_are_fetched_once_and_not_when_too_big() {
        let mut watch = RoomWatch::new(Shown::Room);
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
        let mut watch = RoomWatch::new(Shown::Room);
        let sdk = |mark| Seen {
            sdk_polls: vec![("$p".into(), mark, StablePoll::default())],
            ..Seen::default()
        };
        assert_eq!(watch.see(sdk(1)).load_polls, ["$p"]);
        assert!(watch.see(sdk(1)).load_polls.is_empty());
        assert!(!watch.poll_loaded("$p", &[]));
        assert_eq!(watch.see(sdk(2)).load_polls, ["$p"]);
        assert!(watch.tracks("$p"));

        let standard = Seen {
            new_standard_polls: vec![("$s".into(), StablePoll::default())],
            ..Seen::default()
        };
        assert_eq!(watch.see(standard).load_polls, ["$s"]);
        assert!(watch.tracks("$s"));
    }

    #[test]
    fn the_sdk_s_word_on_a_poll_counts_until_our_own_reading_went_through() {
        let mut watch = RoomWatch::new(Shown::Room);
        let sdk = |mark| Seen {
            sdk_polls: vec![("$p".into(), mark, StablePoll::default())],
            ..Seen::default()
        };
        assert_eq!(watch.see(sdk(1)).load_polls, ["$p"]);
        // No reading yet (no network, or the server said to wait): there
        // is no tally of ours to show, which would say "not ended" over
        // the SDK's "ended".
        assert!(watch.tracks("$p"));
        assert!(!watch.polls().contains_key("$p"));
        watch.see(sdk(2));
        assert!(!watch.polls().contains_key("$p"));
        // Once read, ours is shown.
        watch.poll_loaded("$p", &[]);
        assert!(watch.polls().contains_key("$p"));
        // A standard-format poll has no other tally: shown from the start.
        let standard = Seen {
            new_standard_polls: vec![("$s".into(), StablePoll::default())],
            ..Seen::default()
        };
        watch.see(standard);
        assert!(watch.polls().contains_key("$s"));
    }

    #[test]
    fn one_reading_per_poll_at_a_time_and_again_after_it_when_the_poll_changed() {
        let mut watch = RoomWatch::new(Shown::Room);
        let sdk = |mark| Seen {
            sdk_polls: vec![("$p".into(), mark, StablePoll::default())],
            ..Seen::default()
        };
        assert_eq!(watch.see(sdk(1)).load_polls, ["$p"]);
        // Three votes arrive while the answers are being read: no reading
        // each, but one more when this one is done.
        assert!(!watch.poll_changed("$p"));
        assert!(!watch.poll_changed("$p"));
        assert!(watch.see(sdk(2)).load_polls.is_empty());
        assert!(watch.poll_loaded("$p", &[]));
        // That one found the poll at rest.
        assert!(!watch.poll_loaded("$p", &[]));
        assert!(watch.poll_changed("$p"));
        // A poll that is not followed is not read.
        assert!(!watch.poll_changed("$unknown"));
    }
}
