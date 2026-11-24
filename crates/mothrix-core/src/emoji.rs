// SPDX-License-Identifier: MPL-2.0

//! Emoji: finding them by their short name (`:tada:` style) for completion
//! in the message box, all of them by group for pickers, and which ones
//! the user takes most ([`Usage`]).

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

/// What reaction bars and pickers offer before the user has taken any.
pub const DEFAULTS: [&str; 12] = [
    "👍", "❤️", "😂", "🎉", "😮", "😢", "🙏", "👀", "✅", "🔥", "👏", "😀",
];

/// Up to `limit` emoji whose short name fits `query`: (emoji, short name).
/// Names starting with the query come first, then names containing it.
/// With nothing typed yet: `favourites` (see [`Usage::favourites`]).
pub fn find(query: &str, limit: usize, favourites: &[String]) -> Vec<(String, String)> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return favourites
            .iter()
            .filter_map(|emoji| {
                let found = emojis::get(emoji)?;
                Some((emoji.clone(), found.shortcode()?.to_owned()))
            })
            .take(limit)
            .collect();
    }

    let mut exact = Vec::new();
    let mut starting = Vec::new();
    let mut containing = Vec::new();
    for emoji in emojis::iter() {
        // An emoji can have several names; use the one that fits best.
        let mut best: Option<(u8, &str)> = None;
        for code in emoji.shortcodes() {
            let rank = if code == query {
                0
            } else if code.starts_with(&query) {
                1
            } else if code.contains(&query) {
                2
            } else {
                continue;
            };
            if best.is_none_or(|(known, _)| rank < known) {
                best = Some((rank, code));
            }
        }
        if let Some((rank, code)) = best {
            let entry = (emoji.as_str().to_owned(), code.to_owned());
            match rank {
                0 => exact.push(entry),
                1 => starting.push(entry),
                _ => containing.push(entry),
            }
        }
    }
    // Shorter names are the more common ones ("smile" before "smiley_cat").
    starting.sort_by_key(|(_, code)| code.len());
    containing.sort_by_key(|(_, code)| code.len());
    exact
        .into_iter()
        .chain(starting)
        .chain(containing)
        .take(limit)
        .collect()
}

/// One group of a picker: its title and its emoji with their names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Group {
    pub title: &'static str,
    /// The emoji, and the words it is found by (its name and short names).
    pub emoji: Vec<(&'static str, String)>,
}

/// Every emoji (Unicode 15, default skin tone), by group, in the usual
/// order of pickers.
pub fn groups() -> Vec<Group> {
    emojis::Group::iter()
        .map(|group| Group {
            title: match group {
                emojis::Group::SmileysAndEmotion => "Smileys",
                emojis::Group::PeopleAndBody => "People",
                emojis::Group::AnimalsAndNature => "Animals and nature",
                emojis::Group::FoodAndDrink => "Food and drink",
                emojis::Group::TravelAndPlaces => "Travel and places",
                emojis::Group::Activities => "Activities",
                emojis::Group::Objects => "Objects",
                emojis::Group::Symbols => "Symbols",
                emojis::Group::Flags => "Flags",
            },
            emoji: group
                .emojis()
                .filter(|emoji| matches!(emoji.skin_tone(), None | Some(emojis::SkinTone::Default)))
                .map(|emoji| {
                    let mut words = emoji.name().to_owned();
                    for code in emoji.shortcodes() {
                        words.push(' ');
                        words.push_str(code);
                    }
                    (emoji.as_str(), words)
                })
                .collect(),
        })
        .collect()
}

/// How often and how lately the user took each emoji (to react or in a
/// message), kept in `emoji.json` next to the profiles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    used: HashMap<String, Used>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Used {
    count: u32,
    /// Seconds since 1970.
    last: u64,
}

/// After this long, a use counts half.
const HALF_LIFE_SECS: f64 = 14.0 * 24.0 * 3600.0;

impl Usage {
    /// Where it is kept.
    pub fn file() -> Option<PathBuf> {
        Some(crate::folders::data()?.join("emoji.json"))
    }

    pub fn load() -> Self {
        Self::file()
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// The emoji was taken now (in memory; [`Usage::store`] keeps it).
    pub fn note(&mut self, emoji: &str) {
        self.add(emoji, now());
    }

    /// Count the emoji in the file, read anew first, so that two mothrix
    /// running at once (with different accounts) both count.
    pub fn store(emoji: &str) {
        let Some(file) = Self::file() else { return };
        let mut stored = Self::load();
        stored.add(emoji, now());
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec(&stored) {
            let _ = crate::folders::write_private(&file, &json);
        }
    }

    fn add(&mut self, emoji: &str, now: u64) {
        let used = self.used.entry(emoji.to_owned()).or_default();
        used.count = used.count.saturating_add(1);
        used.last = now;
    }

    /// The `n` emoji to offer first: those taken often and lately, then
    /// [`DEFAULTS`].
    pub fn favourites(&self, n: usize) -> Vec<String> {
        self.favourites_at(n, now())
    }

    fn favourites_at(&self, n: usize, now: u64) -> Vec<String> {
        let score = |used: &Used| {
            let age = now.saturating_sub(used.last) as f64;
            f64::from(used.count) * 0.5_f64.powf(age / HALF_LIFE_SECS)
        };
        let mut taken: Vec<(&String, &Used)> = self.used.iter().collect();
        taken.sort_by(|a, b| {
            score(b.1)
                .total_cmp(&score(a.1))
                .then(b.1.last.cmp(&a.1.last))
        });
        let mut out: Vec<String> = taken.into_iter().map(|(emoji, _)| emoji.clone()).collect();
        for default in DEFAULTS {
            if !out.iter().any(|emoji| emoji == default) {
                out.push(default.to_owned());
            }
        }
        out.truncate(n);
        out
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<String> {
        DEFAULTS.iter().map(|emoji| (*emoji).to_owned()).collect()
    }

    #[test]
    fn names_starting_with_the_query_come_first() {
        let found = find("tad", 6, &[]);
        assert_eq!(found[0], ("🎉".to_owned(), "tada".to_owned()));
        let found = find("smile", 6, &[]);
        assert_eq!(found[0].1, "smile");
        assert!(found.len() > 1);
    }

    #[test]
    fn a_bare_colon_offers_favourites_and_nonsense_offers_nothing() {
        let favourites = find("", 6, &defaults());
        assert_eq!(favourites.len(), 6);
        assert_eq!(favourites[0], ("👍".to_owned(), "+1".to_owned()));
        assert!(find("zzzzqqq", 6, &defaults()).is_empty());
    }

    #[test]
    fn the_limit_is_respected() {
        assert_eq!(find("a", 4, &[]).len(), 4);
    }

    #[test]
    fn emoji_taken_often_and_lately_come_first() {
        let day = 24 * 3600;
        let mut usage = Usage::default();
        assert_eq!(usage.favourites_at(3, 100 * day), ["👍", "❤️", "😂"]);
        for _ in 0..5 {
            usage.add("🦋", day);
        }
        usage.add("🍕", 99 * day);
        usage.add("🍕", 99 * day);
        // Two uses yesterday beat five long ago; the defaults fill up.
        assert_eq!(usage.favourites_at(4, 100 * day), ["🍕", "🦋", "👍", "❤️"]);
    }

    #[test]
    fn every_group_has_emoji_and_they_are_found_by_name() {
        let groups = groups();
        assert_eq!(groups.len(), 9);
        assert!(groups.iter().all(|group| !group.emoji.is_empty()));
        let total: usize = groups.iter().map(|group| group.emoji.len()).sum();
        assert!(total > 1500, "{total}");
        let party = groups[0]
            .emoji
            .iter()
            .find(|(emoji, _)| *emoji == "😀")
            .unwrap();
        assert!(party.1.contains("grinning"));
    }
}
