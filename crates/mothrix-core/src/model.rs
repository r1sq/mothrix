// SPDX-License-Identifier: MPL-2.0

//! Plain data the UI renders. Nothing in here knows about matrix-sdk, which
//! keeps the UI testable without a homeserver.

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RoomEntry {
    /// The room id; with several accounts, the key made from it (see
    /// `accounts`).
    pub id: String,
    /// Index of the account the room belongs to.
    pub account: usize,
    pub name: String,
    pub topic: String,
    pub is_dm: bool,
    /// A space: a group of rooms rather than a place to talk.
    pub is_space: bool,
    /// The space the room is in, as a section name (`Space` or
    /// `Space/Subspace`); empty for none.
    pub space: String,
    pub is_invite: bool,
    /// What the room notifies about.
    pub notify: Notify,
    /// Pinned to the top of the sidebar (Matrix "favourite" tag, shared
    /// with other clients).
    pub favourite: bool,
    pub low_priority: bool,
    pub unread: u64,
    pub mentions: u64,
    /// Unread only because the user marked it so (here or in another
    /// client): nothing new came, and nothing is to be told.
    #[serde(default)]
    pub marked_unread: bool,
}

/// What a room notifies about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Notify {
    /// As the account's own rules say.
    #[default]
    Default,
    All,
    Mentions,
    Mute,
}

impl Notify {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::All => "all messages",
            Self::Mentions => "mentions only",
            Self::Mute => "muted",
        }
    }

    /// The next choice when cycling through them.
    pub fn next(self) -> Self {
        match self {
            Self::Default | Self::All => Self::Mentions,
            Self::Mentions => Self::Mute,
            Self::Mute => Self::All,
        }
    }
}

/// Someone in a room, as the room info dialog lists them.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RoomMember {
    pub user_id: String,
    pub name: String,
    /// "admin", "moderator" or empty.
    pub role: String,
    /// Invited, not there yet.
    pub invited: bool,
}

/// What the room info dialog shows, and what we may do in the room.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RoomDetails {
    pub room_id: String,
    pub name: String,
    pub topic: String,
    /// The room's public address, like `#lunch:example.org`; may be empty.
    pub alias: String,
    pub encrypted: bool,
    pub members: Vec<RoomMember>,
    pub can_invite: bool,
    pub can_kick: bool,
    pub can_ban: bool,
    pub can_rename: bool,
    pub can_set_topic: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct PollAnswer {
    pub id: String,
    pub text: String,
    /// Display names of the voters. For open polls with hidden results this
    /// only ever contains ourselves.
    pub voters: Vec<String>,
    pub self_voted: bool,
    pub is_winner: bool,
}

/// The event formats polls come in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum PollFormat {
    /// `org.matrix.msc3381.poll.*`: what Element sends and the SDK handles.
    #[default]
    Unstable,
    /// `m.poll.*` events with the standard content (`m.poll`, `m.selections`).
    Standard,
    /// `m.poll.*` event names around the old content layout
    /// (`m.poll.start: {..}`, `m.poll.response: {answers}`), as sent by the
    /// IServ messenger.
    Mixed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Poll {
    pub question: String,
    pub max_selections: u64,
    pub is_ended: bool,
    pub results_visible: bool,
    pub voter_count: u64,
    pub answers: Vec<PollAnswer>,
    /// Which event format the poll uses; votes must be sent in the same.
    pub format: PollFormat,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum ItemKind {
    #[default]
    Text,
    Emote,
    Notice,
    /// Image, file, audio, video or sticker; `body` holds a short label.
    Media,
    /// Membership, profile and room state changes; `body` is the sentence.
    State,
    Poll(Poll),
    Redacted,
    Undecryptable,
    DateDivider,
    ReadMarker,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Reaction {
    pub key: String,
    pub count: usize,
    pub mine: bool,
    /// Names of the others who reacted so, in the order they did (the
    /// user is `mine`).
    pub senders: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Item {
    pub key: String,
    /// Empty while the message is a local echo that has not reached the server.
    pub event_id: String,
    pub kind: ItemKind,
    pub sender_id: String,
    pub sender_name: String,
    pub body: String,
    pub ts_ms: u64,
    pub is_own: bool,
    pub edited: bool,
    /// Our own text message that the server still lets us change.
    pub can_edit: bool,
    /// We may delete it: our own, or anyone's where we moderate.
    pub can_delete: bool,
    /// An image or sticker, for which a [`Preview`] may arrive.
    /// What was written with a picture or file (its caption), apart
    /// from the file's name; empty without one.
    #[serde(default)]
    pub caption: String,
    pub image: bool,
    pub pending: bool,
    pub failed: bool,
    pub reactions: Vec<Reaction>,
    /// (sender name, first line) of the message this one replies to.
    pub reply: Option<(String, String)>,
    /// The id of the message this one answers (empty if none).
    #[serde(default)]
    pub reply_to: String,
    /// Pinned in the room, for everyone.
    pub pinned: bool,
    /// Names of the others who have read up to this message.
    pub read_by: Vec<String>,
    /// The thread this message started: how many replies, and (sender
    /// name, first line) of the newest one.
    pub thread: Option<(u32, Option<(String, String)>)>,
}

/// A room member, for @-mention completion.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Member {
    pub user_id: String,
    pub name: String,
}

/// A small picture for the conversation view, drawn with half-block
/// characters: every terminal cell shows two pixels, one above the other.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preview {
    /// Width in pixels (= terminal cells).
    pub width: usize,
    /// Height in pixels; always even (two per text row).
    pub height: usize,
    /// Row-major RGBA, `width * height` entries.
    pub pixels: Vec<[u8; 4]>,
    /// A sharper PNG for terminals with real graphics support, and the
    /// box of cells it should fill. Empty when not available.
    pub png: Vec<u8>,
    pub png_columns: usize,
    pub png_rows: usize,
}

impl Preview {
    pub fn rows(&self) -> usize {
        self.height / 2
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        self.pixels[y * self.width + x]
    }
}

/// Where a device verification stands, as shown in the verification dialog.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum VerifyState {
    /// Another of our devices asks to verify this one.
    Incoming {
        device: String,
    },
    /// Waiting for the other device to react.
    Waiting,
    /// Both devices show these seven (emoji, name) pairs; the user compares.
    Emojis(Vec<(String, String)>),
    /// We confirmed; waiting for the other device to confirm too.
    Confirming,
    Done,
    Failed(String),
}

/// A problem a LanguageTool server found in the message being written.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct GrammarMatch {
    /// Char range in the checked text.
    pub start: usize,
    pub end: usize,
    /// The text in that range.
    pub text: String,
    pub message: String,
    pub replacements: Vec<String>,
    /// A misspelled word (can be ignored for good) rather than grammar or style.
    pub spelling: bool,
}

/// A sidebar section the user made, holding rooms of their choice.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Section {
    pub name: String,
    /// Room ids.
    #[serde(default)]
    pub rooms: Vec<String>,
}

/// A message found by search.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct SearchHit {
    pub room_id: String,
    pub room_name: String,
    pub event_id: String,
    /// Display name of the sender.
    pub sender: String,
    pub ts_ms: u64,
    pub text: String,
}
