// SPDX-License-Identifier: MPL-2.0

//! Searching messages, in one chat or all of them.
//!
//! Two sources are combined:
//!
//! - the local message cache, which holds everything this device has loaded
//!   so far, already decrypted. It is the only way to search encrypted
//!   rooms, and it works offline.
//! - the homeserver's search, which reaches the full history but only of
//!   unencrypted rooms (the server cannot read the others).

use std::collections::{HashMap, HashSet};

use matrix_sdk::{
    Client, Room,
    ruma::{
        RoomId, UserId,
        api::client::search::search_events::v3::{Categories, Criteria, Request},
    },
};
use matrix_sdk_base::event_cache::store::EventCacheStoreLockState;
use serde_json::Value;

use crate::model::SearchHit;

/// Most results one search returns (newest first).
pub const MAX_HITS: usize = 200;

/// The lowercase words of a query. A message matches when it contains all
/// of them, in any order.
pub fn terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

pub fn matches(text: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    terms.iter().all(|term| text.contains(term))
}

/// One line of `text` around the first matching word, at most `width`
/// characters, with `…` where it was cut.
pub fn snippet(text: &str, terms: &[String], width: usize) -> String {
    let flat: Vec<char> = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect();
    if flat.len() <= width {
        return flat.into_iter().collect();
    }
    let lower: String = flat.iter().flat_map(|c| c.to_lowercase().next()).collect();
    let lower: Vec<char> = lower.chars().collect();
    // Character position of the earliest matching word.
    let first = terms
        .iter()
        .filter_map(|term| {
            let term: Vec<char> = term.chars().collect();
            lower
                .windows(term.len().max(1))
                .position(|window| window == term.as_slice())
        })
        .min()
        .unwrap_or(0);
    let start = first
        .saturating_sub(width / 3)
        .min(flat.len().saturating_sub(width));
    let end = (start + width).min(flat.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&flat[start + usize::from(start > 0)..end - usize::from(end < flat.len())]);
    if end < flat.len() {
        out.push('…');
    }
    out
}

/// Turn a raw event into a hit if it is a message matching `terms`.
fn hit_from_event(
    event: &Value,
    room_id: &str,
    room_name: &str,
    terms: &[String],
) -> Option<SearchHit> {
    if event["type"].as_str()? != "m.room.message" {
        return None;
    }
    let content = &event["content"];
    // An edit repeats the message; the original (or its replacement, as
    // shown in the room) is found on its own.
    if content["m.relates_to"]["rel_type"].as_str() == Some("m.replace") {
        return None;
    }
    let body = content["body"].as_str()?;
    if !matches(body, terms) {
        return None;
    }
    Some(SearchHit {
        room_id: room_id.to_owned(),
        room_name: room_name.to_owned(),
        event_id: event["event_id"].as_str()?.to_owned(),
        sender: event["sender"].as_str().unwrap_or_default().to_owned(),
        ts_ms: event["origin_server_ts"].as_u64().unwrap_or_default(),
        text: body.to_owned(),
    })
}

async fn room_name(room: &Room) -> String {
    match room.cached_display_name() {
        Some(name) => name.to_string(),
        None => room
            .display_name()
            .await
            .map(|name| name.to_string())
            .unwrap_or_default(),
    }
}

/// Search `query` in the room `only_room`, or in every joined room.
pub async fn search(client: &Client, query: &str, only_room: Option<&RoomId>) -> Vec<SearchHit> {
    let terms = terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let rooms: Vec<Room> = match only_room {
        Some(room_id) => client.get_room(room_id).into_iter().collect(),
        None => client.joined_rooms(),
    };
    let mut names = HashMap::new();
    for room in &rooms {
        names.insert(room.room_id().to_string(), room_name(room).await);
    }

    let mut hits: Vec<SearchHit> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. The local cache.
    if let Ok(EventCacheStoreLockState::Clean(store) | EventCacheStoreLockState::Dirty(store)) =
        client.event_cache_store().lock().await
    {
        for room in &rooms {
            let room_id = room.room_id();
            let Ok(events) = store
                .get_room_events(room_id, Some("m.room.message"), None)
                .await
            else {
                continue;
            };
            for event in events {
                let Ok(value) = serde_json::from_str::<Value>(event.raw().json().get()) else {
                    continue;
                };
                let name = names
                    .get(room_id.as_str())
                    .map(String::as_str)
                    .unwrap_or_default();
                if let Some(hit) = hit_from_event(&value, room_id.as_str(), name, &terms)
                    && seen.insert(hit.event_id.clone())
                {
                    hits.push(hit);
                }
            }
        }
    }

    // 2. The server, for history this device never loaded. Best effort:
    // some servers do not offer search at all.
    let mut criteria = Criteria::new(query.trim().to_owned());
    if let Some(room_id) = only_room {
        criteria.filter.rooms = Some(vec![room_id.to_owned()]);
    }
    let mut categories = Categories::new();
    categories.room_events = Some(criteria);
    if let Ok(response) = client.send(Request::new(categories)).await {
        for result in response.search_categories.room_events.results {
            let Some(raw) = result.result else { continue };
            let Ok(value) = serde_json::from_str::<Value>(raw.json().get()) else {
                continue;
            };
            let Some(room_id) = value["room_id"].as_str() else {
                continue;
            };
            let Some(name) = names.get(room_id) else {
                continue;
            };
            if let Some(hit) = hit_from_event(&value, room_id, name, &terms)
                && seen.insert(hit.event_id.clone())
            {
                hits.push(hit);
            }
        }
    }

    // Show display names instead of user ids where the room knows them.
    let mut sender_names: HashMap<(String, String), String> = HashMap::new();
    for hit in &mut hits {
        let key = (hit.room_id.clone(), hit.sender.clone());
        if !sender_names.contains_key(&key) {
            let mut name = crate::convert::localpart(&hit.sender);
            if let (Ok(room_id), Ok(user_id)) =
                (RoomId::parse(&hit.room_id), UserId::parse(&hit.sender))
                && let Some(room) = client.get_room(&room_id)
                && let Ok(Some(member)) = room.get_member_no_sync(&user_id).await
            {
                name = member.name().to_owned();
            }
            sender_names.insert(key.clone(), name);
        }
        hit.sender = sender_names[&key].clone();
    }

    hits.sort_by_key(|hit| std::cmp::Reverse(hit.ts_ms));
    hits.truncate(MAX_HITS);
    hits
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn all_words_must_appear_in_any_order_and_case() {
        let terms = terms("Pizza FRIDAY");
        assert!(matches("On friday we get pizza", &terms));
        assert!(!matches("On friday we get sushi", &terms));
        assert!(!matches("anything", &super::terms("   ")));
    }

    #[test]
    fn snippets_stay_on_one_line_and_centre_on_the_match() {
        assert_eq!(snippet("short\ntext", &terms("text"), 40), "short text");
        let long = format!("{} needle {}", "a ".repeat(60), "b ".repeat(60));
        let cut = snippet(&long, &terms("needle"), 40);
        assert!(cut.contains("needle"), "{cut}");
        assert!(cut.starts_with('…') && cut.ends_with('…'), "{cut}");
        assert!(cut.chars().count() <= 40, "{cut}");
    }

    #[test]
    fn only_matching_messages_become_hits_and_edits_are_skipped() {
        let terms = terms("lunch");
        let message = json!({
            "type": "m.room.message", "event_id": "$1", "sender": "@amy:x", "origin_server_ts": 5,
            "content": {"msgtype": "m.text", "body": "Lunch at noon?"},
        });
        let hit = hit_from_event(&message, "!r", "Club", &terms).expect("a hit");
        assert_eq!(
            (hit.event_id.as_str(), hit.room_name.as_str(), hit.ts_ms),
            ("$1", "Club", 5)
        );

        let edit = json!({
            "type": "m.room.message", "event_id": "$2", "sender": "@amy:x",
            "content": {"body": "* Lunch at one?", "m.relates_to": {"rel_type": "m.replace", "event_id": "$1"}},
        });
        assert!(hit_from_event(&edit, "!r", "Club", &terms).is_none());

        let other = json!({"type": "m.reaction", "event_id": "$3", "content": {"body": "lunch"}});
        assert!(hit_from_event(&other, "!r", "Club", &terms).is_none());
    }
}
