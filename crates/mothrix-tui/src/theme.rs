// SPDX-License-Identifier: MPL-2.0

//! The colour themes (`mothrix_core::theme`) in the terminal's colours. The
//! background is never painted, so mothrix blends into whatever the
//! terminal (and its transparency/blur) already shows.

use std::borrow::Cow;

use mothrix_core::theme::{BUILT_IN, Palette, Rgb, SENDER_COLORS, sender_slot};
use ratatui::style::Color;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub border: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub unread: Color,
    pub mention: Color,
    pub error: Color,
    pub success: Color,
    /// Colours handed out to message senders, by hash of their user id.
    pub senders: [Color; SENDER_COLORS],
}

const fn color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}

/// A palette with the given name, in the terminal's colours.
const fn from_palette(name: &'static str, palette: &Palette) -> Theme {
    let mut senders = [Color::Reset; SENDER_COLORS];
    let mut index = 0;
    while index < SENDER_COLORS {
        senders[index] = color(palette.senders[index]);
        index += 1;
    }
    Theme {
        name,
        fg: color(palette.fg),
        dim: color(palette.dim),
        accent: color(palette.accent),
        border: color(palette.border),
        selection_bg: color(palette.selection_bg),
        selection_fg: color(palette.selection_fg),
        unread: color(palette.unread),
        mention: color(palette.mention),
        error: color(palette.error),
        success: color(palette.success),
        senders,
    }
}

/// A built-in palette: its name is there for the whole program.
const fn built_in(palette: &Palette) -> Theme {
    let name = match &palette.name {
        Cow::Borrowed(name) => name,
        Cow::Owned(_) => panic!("built-in themes have static names"),
    };
    from_palette(name, palette)
}

/// The built-in themes, in the order of `mothrix_core::theme::BUILT_IN`.
pub const THEMES: &[Theme] = &{
    let mut themes = [built_in(&BUILT_IN[0]); BUILT_IN.len()];
    let mut index = 1;
    while index < BUILT_IN.len() {
        themes[index] = built_in(&BUILT_IN[index]);
        index += 1;
    }
    themes
};

impl From<&Palette> for Theme {
    fn from(palette: &Palette) -> Self {
        let name = match &palette.name {
            Cow::Borrowed(name) => name,
            // Themes live as long as the program.
            Cow::Owned(name) => &*Box::leak(name.clone().into_boxed_str()),
        };
        from_palette(name, palette)
    }
}

/// Themes from the config file, set once at start.
static CUSTOM: std::sync::OnceLock<Vec<Theme>> = std::sync::OnceLock::new();

/// Add the themes of the config file to the built-in ones.
pub fn set_custom(themes: Vec<Theme>) {
    let _ = CUSTOM.set(themes);
}

/// All themes: the built-in ones, then the user's own.
pub fn all() -> Vec<Theme> {
    let mut themes = THEMES.to_vec();
    themes.extend(CUSTOM.get().into_iter().flatten().copied());
    themes
}

pub fn by_name(name: &str) -> Option<Theme> {
    all()
        .into_iter()
        .find(|theme| theme.name.eq_ignore_ascii_case(name.trim()))
}

pub fn next_after(name: &str) -> Theme {
    let themes = all();
    let index = themes
        .iter()
        .position(|theme| theme.name == name)
        .unwrap_or(0);
    themes[(index + 1) % themes.len()]
}

impl Theme {
    pub fn sender_color(&self, user_id: &str) -> Color {
        self.senders[sender_slot(user_id)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_has_the_core_themes_in_its_colours() {
        assert_eq!(THEMES.len(), BUILT_IN.len());
        for (theme, palette) in THEMES.iter().zip(BUILT_IN) {
            assert_eq!(theme.name, palette.name);
            assert_eq!(theme.accent, color(palette.accent));
            assert_eq!(theme.senders[5], color(palette.senders[5]));
        }
        assert_eq!(THEMES[0].name, "mothrix");
        assert_eq!(THEMES[0].fg, Color::Rgb(0xef, 0xdc, 0xb8));
        let own = Palette {
            name: "mine".to_owned().into(),
            ..BUILT_IN[2].clone()
        };
        let theme = Theme::from(&own);
        assert_eq!(theme.name, "mine");
        assert_eq!(theme.fg, THEMES[2].fg);
    }
}
