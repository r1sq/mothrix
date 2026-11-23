// SPDX-License-Identifier: MPL-2.0

//! What the user can set: the preferences the settings dialog changes and
//! the config file stores (as fields at its top level).

use serde::{Deserialize, Serialize};

use crate::languagetool;

/// Order of rooms inside each sidebar section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    /// Most recent activity first (the order the server reports).
    #[default]
    Recent,
    Name,
    /// Rooms with mentions, then unread rooms, then the rest by activity.
    Unread,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::Name => "name",
            Self::Unread => "unread",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Recent => Self::Name,
            Self::Name => Self::Unread,
            Self::Unread => Self::Recent,
        }
    }

    pub fn from_label(label: &str) -> Self {
        match label.trim() {
            "name" => Self::Name,
            "unread" => Self::Unread,
            _ => Self::Recent,
        }
    }
}

/// The preferences the settings dialog changes (besides the theme), as
/// the UI holds them and the config file stores them. Their defaults are
/// the ones here, for a new file and a fresh UI alike.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// Sidebar order: "recent", "name" or "unread".
    #[serde(default, with = "sort_label")]
    pub sort: SortMode,
    /// Desktop notifications for new messages (default on).
    #[serde(default = "default_true")]
    pub notifications: bool,
    /// How pictures are drawn: "auto", "kitty", "sixel", "blocks" or "off".
    #[serde(default = "default_images")]
    pub images: String,
    /// Underline misspelled words in the message box (default on).
    #[serde(default = "default_true")]
    pub spellcheck: bool,
    /// Show joins, leaves and other room changes (default on).
    #[serde(default = "default_true")]
    pub room_events: bool,
    /// Rooms and people in one sidebar section (default: two).
    #[serde(default)]
    pub mixed_list: bool,
    /// Rooms of a space are listed under the space's name (default on).
    #[serde(default = "default_true")]
    pub spaces: bool,
    /// Tell rooms when you are writing (default on).
    #[serde(default = "default_true")]
    pub typing_notices: bool,
    /// Show who has read up to which message (default on).
    #[serde(default = "default_true")]
    pub read_receipts: bool,
    /// Ask the homeserver what is behind links in messages (default off:
    /// the server learns the addresses, also from encrypted rooms).
    #[serde(default)]
    pub link_previews: bool,
    /// Address of a LanguageTool server for grammar checking, like
    /// "https://lt.example.org". Empty (the default): not used. What
    /// you write is sent to that server.
    #[serde(default)]
    pub languagetool_url: String,
    /// Languages LanguageTool checks in, like ["de-DE", "en-US"]: the
    /// text is checked in each and every sentence keeps the one that fits.
    /// Empty (the default): the server detects one language per message.
    #[serde(default)]
    pub languagetool_languages: Vec<String>,
    /// Account of the paid LanguageTool service (server
    /// "https://api.languagetoolplus.com"): e-mail and API key.
    #[serde(default)]
    pub languagetool_username: String,
    #[serde(default)]
    pub languagetool_api_key: String,
    /// An HTTP proxy for every connection, like "http://proxy:3128", for
    /// networks without a direct way out. Empty (the default): the
    /// `HTTPS_PROXY` or `ALL_PROXY` environment variable if set, else
    /// direct. Used from the next start (and by new sign-ins).
    #[serde(default)]
    pub proxy: String,
    /// Fold messages longer than this many lines; 0 shows all in full.
    #[serde(default = "default_long_messages")]
    pub long_messages: usize,
    /// Look for a new release on GitHub after the start and then daily
    /// (default on; only release builds and packaged ones look, see
    /// `update.rs`). GitHub then sees the address mothrix runs from.
    #[serde(default = "default_true")]
    pub updates: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        toml::from_str("").expect("every preference has a default")
    }
}

impl Prefs {
    /// The same with what was typed by hand tidied up: spaces around the
    /// texts, an empty picture mode, languages spelled like "de".
    pub fn tidied(&self, dictionaries: &[String]) -> Self {
        let images = self.images.trim();
        Self {
            images: if images.is_empty() {
                default_images()
            } else {
                images.to_owned()
            },
            languagetool_url: self.languagetool_url.trim().to_owned(),
            proxy: self.proxy.trim().to_owned(),
            languagetool_username: self.languagetool_username.trim().to_owned(),
            languagetool_api_key: self.languagetool_api_key.trim().to_owned(),
            languagetool_languages: languagetool::parse_languages(
                &self.languagetool_languages.join(","),
                &languagetool::preferred_variants(dictionaries),
            ),
            ..self.clone()
        }
    }
}

/// The sort order as its label in the file.
mod sort_label {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::SortMode;

    pub fn serialize<S: Serializer>(sort: &SortMode, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(sort.label())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SortMode, D::Error> {
        Ok(SortMode::from_label(&String::deserialize(deserializer)?))
    }
}

fn default_long_messages() -> usize {
    15
}

fn default_images() -> String {
    "auto".to_owned()
}

fn default_true() -> bool {
    true
}
