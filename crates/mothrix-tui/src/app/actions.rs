// SPDX-License-Identifier: MPL-2.0

//! What can be done to the selected message: one list that the strip of
//! buttons shows, the keys look up and [`App::on_bar`] checks, so that a
//! button, its key and what happens cannot disagree.

use super::*;

/// One thing that can be done to the selected message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageAction {
    pub action: BarAction,
    /// The key that does it, shown before the label.
    pub key: char,
    pub label: &'static str,
}

impl MessageAction {
    fn new(action: BarAction, key: char, label: &'static str) -> Self {
        Self { action, key, label }
    }
}

impl App {
    /// What can be done to `item` right now, in the order the strip shows.
    pub fn message_actions(&self, item: &Item) -> Vec<MessageAction> {
        use BarAction as A;
        let redacted = matches!(item.kind, ItemKind::Redacted);
        let readable = !matches!(item.kind, ItemKind::Redacted | ItemKind::Undecryptable);
        let has_code = matches!(item.kind, ItemKind::Text | ItemKind::Notice)
            && !crate::codeblock::code_blocks(&item.body).is_empty();
        let has_link = !crate::inline::parse_links(&item.body).1.is_empty();
        let mut actions = Vec::new();
        if !redacted {
            actions.push(MessageAction::new(A::Reply, 'r', "Reply"));
            actions.push(MessageAction::new(A::React, 'a', "React"));
        }
        if item.can_edit && matches!(item.kind, ItemKind::Text | ItemKind::Emote) {
            actions.push(MessageAction::new(A::Edit, 'e', "Edit"));
        }
        if item.can_delete && !redacted {
            actions.push(MessageAction::new(A::Delete, 'd', "Delete"));
        }
        if matches!(item.kind, ItemKind::Media) {
            actions.push(MessageAction::new(A::Open, 'o', "Open"));
        } else if readable && has_link {
            actions.push(MessageAction::new(A::Open, 'o', "Open link"));
        }
        if readable {
            actions.push(MessageAction::new(A::Copy, 'c', "Copy"));
        }
        if has_code {
            actions.push(MessageAction::new(A::CopyCode, 'y', "Copy code"));
        }
        if self.collapsible.contains(&item.event_id) {
            let label = if self.expanded.contains(&item.event_id) {
                "Less"
            } else {
                "More"
            };
            actions.push(MessageAction::new(A::Expand, 'x', label));
        }
        if !redacted {
            let label = if item.pinned { "Unpin" } else { "Pin" };
            actions.push(MessageAction::new(A::Pin, 'p', label));
        }
        if matches!(
            item.kind,
            ItemKind::Text | ItemKind::Notice | ItemKind::Emote | ItemKind::Media
        ) {
            actions.push(MessageAction::new(A::Forward, 'f', "Forward"));
        }
        if self.state.open_thread.is_none() && !redacted {
            actions.push(MessageAction::new(A::Thread, 't', "Thread"));
        }
        actions
    }

    /// The action a key stands for on the selected message, if any. `y`
    /// copies the whole message where there is no code to copy.
    pub(super) fn message_action_for_key(&self, key: char) -> Option<BarAction> {
        let item = self.selected_item()?;
        let actions = self.message_actions(item);
        let find = |key: char| {
            actions
                .iter()
                .find(|action| action.key == key)
                .map(|action| action.action.clone())
        };
        find(key).or_else(|| if key == 'y' { find('c') } else { None })
    }

    /// Why a key does nothing on the selected message, where that is
    /// worth saying.
    pub(super) fn unavailable_notice(key: char) -> Option<&'static str> {
        match key {
            'f' => Some("Only messages and files can be forwarded."),
            'o' => Some("Nothing to open in this message."),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: ItemKind, body: &str) -> Item {
        Item {
            key: "$1".into(),
            event_id: "$1".into(),
            body: body.into(),
            kind,
            ..Item::default()
        }
    }

    fn keys(app: &App, item: &Item) -> String {
        app.message_actions(item).iter().map(|a| a.key).collect()
    }

    #[test]
    fn each_kind_of_message_offers_what_works_on_it() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        assert_eq!(keys(&app, &item(ItemKind::Text, "hi")), "racpft");
        let mut own = item(ItemKind::Text, "see https://x.example\n```\ncode\n```");
        own.can_edit = true;
        own.can_delete = true;
        assert_eq!(keys(&app, &own), "raedocypft");
        assert_eq!(keys(&app, &item(ItemKind::Media, "a.png")), "raocpft");
        assert_eq!(keys(&app, &item(ItemKind::Redacted, "")), "");
        // Polls cannot be forwarded, so it is not offered.
        let poll = item(ItemKind::Poll(Poll::default()), "Lunch?");
        assert!(!keys(&app, &poll).contains('f'));
        // Inside a thread there is no thread to open.
        app.state.open_thread = Some("$root".into());
        assert!(!keys(&app, &item(ItemKind::Text, "hi")).contains('t'));
    }
}
