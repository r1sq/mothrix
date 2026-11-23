// SPDX-License-Identifier: MPL-2.0

//! Key bindings from the config file. A binding gives one of mothrix's
//! shortcuts a second key: the key chosen is turned into the built-in one
//! before anything else sees it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The shortcuts that can be given another key: (name in the config, the
/// built-in key).
pub const ACTIONS: [(&str, &str); 13] = [
    ("quit", "ctrl+q"),
    ("find_room", "ctrl+k"),
    ("search", "ctrl+f"),
    ("settings", "f2"),
    ("accounts", "f3"),
    ("unread", "f4"),
    ("emoji", "f5"),
    ("theme", "ctrl+t"),
    ("paste", "ctrl+v"),
    ("previous_room", "alt+up"),
    ("next_room", "alt+down"),
    ("switch_pane", "tab"),
    ("scroll_up", "pageup"),
];

/// A key as written in the config: `ctrl+k`, `alt+up`, `f5`, `ctrl+shift+p`.
pub fn parse_key(spec: &str) -> Option<KeyEvent> {
    let mut modifiers = KeyModifiers::NONE;
    let mut code = None;
    for part in spec.split('+').map(|part| part.trim().to_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
            "alt" | "meta" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            "up" => code = Some(KeyCode::Up),
            "down" => code = Some(KeyCode::Down),
            "left" => code = Some(KeyCode::Left),
            "right" => code = Some(KeyCode::Right),
            "tab" => code = Some(KeyCode::Tab),
            "enter" => code = Some(KeyCode::Enter),
            "esc" => code = Some(KeyCode::Esc),
            "space" => code = Some(KeyCode::Char(' ')),
            "pageup" => code = Some(KeyCode::PageUp),
            "pagedown" => code = Some(KeyCode::PageDown),
            "home" => code = Some(KeyCode::Home),
            "end" => code = Some(KeyCode::End),
            key if key.len() > 1 && key.starts_with('f') => {
                code = Some(KeyCode::F(
                    key[1..].parse().ok().filter(|n| (1..=12).contains(n))?,
                ));
            }
            key if key.chars().count() == 1 => code = key.chars().next().map(KeyCode::Char),
            _ => return None,
        }
    }
    Some(KeyEvent::new(code?, modifiers))
}

/// Whether two key presses are the same key. Terminals differ in whether
/// they report Shift for a capital letter, so letters compare without case.
fn same(a: &KeyEvent, b: &KeyEvent) -> bool {
    let fold = |code: KeyCode| match code {
        KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
        other => other,
    };
    let relevant = KeyModifiers::CONTROL | KeyModifiers::ALT;
    fold(a.code) == fold(b.code) && (a.modifiers & relevant) == (b.modifiers & relevant)
}

/// The bindings of a config: (key chosen, built-in key it stands for).
/// Entries that cannot be used come back as messages.
pub fn keymap(
    configured: &std::collections::BTreeMap<String, String>,
) -> (Vec<(KeyEvent, KeyEvent)>, Vec<String>) {
    let mut map = Vec::new();
    let mut problems = Vec::new();
    for (action, spec) in configured {
        let Some((_, default)) = ACTIONS.iter().find(|(name, _)| name == action) else {
            let names: Vec<&str> = ACTIONS.iter().map(|(name, _)| *name).collect();
            problems.push(format!(
                "[keys] {action}: unknown (there are: {})",
                names.join(", ")
            ));
            continue;
        };
        let Some(key) = parse_key(spec) else {
            problems.push(format!(
                "[keys] {action}: '{spec}' is not a key (like ctrl+k, alt+up, f5)"
            ));
            continue;
        };
        // A plain letter would no longer be typeable.
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && !matches!(key.code, KeyCode::F(_));
        if plain {
            problems.push(format!(
                "[keys] {action}: '{spec}' needs ctrl or alt, or an F key"
            ));
            continue;
        }
        if let Some(default) = parse_key(default) {
            map.push((key, default));
        }
    }
    (map, problems)
}

/// The built-in key a pressed key stands for, if it is bound.
pub fn translate(map: &[(KeyEvent, KeyEvent)], key: KeyEvent) -> KeyEvent {
    map.iter()
        .find(|(bound, _)| same(bound, &key))
        .map(|(_, default)| KeyEvent::new(default.code, default.modifiers))
        .unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_read_as_written() {
        assert_eq!(
            parse_key("Ctrl+K"),
            Some(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("alt + up"),
            Some(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT))
        );
        assert_eq!(
            parse_key("f5"),
            Some(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE))
        );
        assert_eq!(parse_key("f99"), None);
        assert_eq!(parse_key("ctrl+banana"), None);
    }

    #[test]
    fn a_bound_key_stands_for_the_built_in_one() {
        let configured = [
            ("search".to_owned(), "ctrl+s".to_owned()),
            ("settings".to_owned(), "f9".to_owned()),
            ("quit".to_owned(), "x".to_owned()),
            ("fly".to_owned(), "ctrl+y".to_owned()),
        ]
        .into_iter()
        .collect();
        let (map, problems) = keymap(&configured);
        assert_eq!(map.len(), 2);
        assert_eq!(problems.len(), 2, "{problems:?}");

        let pressed = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(
            translate(&map, pressed),
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL)
        );
        let other = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(translate(&map, other), other);
    }
}
