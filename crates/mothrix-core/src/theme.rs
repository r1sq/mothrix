// SPDX-License-Identifier: MPL-2.0

//! The colour themes both front ends offer: the built-in ones and the
//! user's own from the config file (`[[themes]]`). Plain colours only;
//! each front end turns them into its own. The terminal never paints the
//! background, so mothrix blends into whatever the terminal (and its
//! transparency) shows; the iPhone app paints it.

use std::{borrow::Cow, fmt};

use serde::{Serialize, Serializer};

use crate::config::{Config, CustomTheme};

/// A colour, written `#rrggbb` (also in JSON).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// `0xrrggbb`.
    pub const fn hex(hex: u32) -> Self {
        Self((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    /// A colour as written in the config: `#rrggbb`.
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#')?;
        (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(())?;
        Some(Self::hex(u32::from_str_radix(hex, 16).ok()?))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The colours of one theme.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Palette {
    pub name: Cow<'static, str>,
    /// Painted by the iPhone app only; the terminal keeps its own.
    pub background: Rgb,
    pub fg: Rgb,
    pub dim: Rgb,
    pub accent: Rgb,
    pub border: Rgb,
    pub selection_bg: Rgb,
    pub selection_fg: Rgb,
    pub unread: Rgb,
    pub mention: Rgb,
    pub error: Rgb,
    pub success: Rgb,
    /// Colours handed out to message senders, by hash of their user id
    /// ([`sender_slot`]).
    pub senders: [Rgb; SENDER_COLORS],
}

/// How many colours a theme has for senders.
pub const SENDER_COLORS: usize = 6;

const fn rgb(hex: u32) -> Rgb {
    Rgb::hex(hex)
}

/// The built-in themes; the first is the default.
pub const BUILT_IN: &[Palette] = &[
    // The logo's colours: the night it sits in, the moth's gold and
    // cream, the planet's orange and violet.
    Palette {
        name: Cow::Borrowed("mothrix"),
        background: rgb(0x140a1e),
        fg: rgb(0xefdcb8),
        dim: rgb(0x7c6488),
        accent: rgb(0xf2c46d),
        border: rgb(0x3a2448),
        selection_bg: rgb(0x2e1a3d),
        selection_fg: rgb(0xf6e6c4),
        unread: rgb(0xef8d27),
        mention: rgb(0xe8608f),
        error: rgb(0xe0565b),
        success: rgb(0xa9c98a),
        senders: [
            rgb(0xf2c46d),
            rgb(0xc08cf0),
            rgb(0xef8d27),
            rgb(0xe87fa8),
            rgb(0x8fb8de),
            rgb(0xa9c98a),
        ],
    },
    Palette {
        name: Cow::Borrowed("tokyo-night"),
        background: rgb(0x1a1b26),
        fg: rgb(0xc0caf5),
        dim: rgb(0x565f89),
        accent: rgb(0x7aa2f7),
        border: rgb(0x3b4261),
        selection_bg: rgb(0x283457),
        selection_fg: rgb(0xc0caf5),
        unread: rgb(0xe0af68),
        mention: rgb(0xf7768e),
        error: rgb(0xf7768e),
        success: rgb(0x9ece6a),
        senders: [
            rgb(0x7aa2f7),
            rgb(0xbb9af7),
            rgb(0x7dcfff),
            rgb(0x9ece6a),
            rgb(0xe0af68),
            rgb(0xff9e64),
        ],
    },
    Palette {
        name: Cow::Borrowed("catppuccin"),
        // Mocha's base.
        background: rgb(0x1e1e2e),
        fg: rgb(0xcdd6f4),
        dim: rgb(0x6c7086),
        accent: rgb(0xcba6f7),
        border: rgb(0x45475a),
        selection_bg: rgb(0x313244),
        selection_fg: rgb(0xcdd6f4),
        unread: rgb(0xf9e2af),
        mention: rgb(0xf38ba8),
        error: rgb(0xf38ba8),
        success: rgb(0xa6e3a1),
        senders: [
            rgb(0x89b4fa),
            rgb(0xcba6f7),
            rgb(0x94e2d5),
            rgb(0xa6e3a1),
            rgb(0xf9e2af),
            rgb(0xfab387),
        ],
    },
    Palette {
        name: Cow::Borrowed("gruvbox"),
        background: rgb(0x282828),
        fg: rgb(0xebdbb2),
        dim: rgb(0x928374),
        accent: rgb(0xfabd2f),
        border: rgb(0x504945),
        selection_bg: rgb(0x3c3836),
        selection_fg: rgb(0xfbf1c7),
        unread: rgb(0xfe8019),
        mention: rgb(0xfb4934),
        error: rgb(0xfb4934),
        success: rgb(0xb8bb26),
        senders: [
            rgb(0x83a598),
            rgb(0xd3869b),
            rgb(0x8ec07c),
            rgb(0xb8bb26),
            rgb(0xfabd2f),
            rgb(0xfe8019),
        ],
    },
    Palette {
        name: Cow::Borrowed("nord"),
        // nord0.
        background: rgb(0x2e3440),
        fg: rgb(0xd8dee9),
        dim: rgb(0x616e88),
        accent: rgb(0x88c0d0),
        border: rgb(0x434c5e),
        selection_bg: rgb(0x3b4252),
        selection_fg: rgb(0xeceff4),
        unread: rgb(0xebcb8b),
        mention: rgb(0xbf616a),
        error: rgb(0xbf616a),
        success: rgb(0xa3be8c),
        senders: [
            rgb(0x81a1c1),
            rgb(0xb48ead),
            rgb(0x88c0d0),
            rgb(0xa3be8c),
            rgb(0xebcb8b),
            rgb(0xd08770),
        ],
    },
    Palette {
        name: Cow::Borrowed("everforest"),
        // Dark, medium: bg0.
        background: rgb(0x2d353b),
        fg: rgb(0xd3c6aa),
        dim: rgb(0x7a8478),
        accent: rgb(0xa7c080),
        border: rgb(0x475258),
        selection_bg: rgb(0x3d484d),
        selection_fg: rgb(0xd3c6aa),
        unread: rgb(0xdbbc7f),
        mention: rgb(0xe67e80),
        error: rgb(0xe67e80),
        success: rgb(0xa7c080),
        senders: [
            rgb(0x7fbbb3),
            rgb(0xd699b6),
            rgb(0x83c092),
            rgb(0xa7c080),
            rgb(0xdbbc7f),
            rgb(0xe69875),
        ],
    },
    Palette {
        name: Cow::Borrowed("rose-pine"),
        background: rgb(0x191724),
        fg: rgb(0xe0def4),
        dim: rgb(0x6e6a86),
        accent: rgb(0xc4a7e7),
        border: rgb(0x403d52),
        selection_bg: rgb(0x26233a),
        selection_fg: rgb(0xe0def4),
        unread: rgb(0xf6c177),
        mention: rgb(0xeb6f92),
        error: rgb(0xeb6f92),
        success: rgb(0x9ccfd8),
        senders: [
            rgb(0x9ccfd8),
            rgb(0xc4a7e7),
            rgb(0x31748f),
            rgb(0xebbcba),
            rgb(0xf6c177),
            rgb(0xeb6f92),
        ],
    },
];

/// The theme used when none (or an unknown one) is chosen.
pub fn default_palette() -> Palette {
    BUILT_IN[0].clone()
}

/// The theme of that name among `themes`, ignoring case and spaces
/// around it.
pub fn find<'a>(themes: &'a [Palette], name: &str) -> Option<&'a Palette> {
    themes
        .iter()
        .find(|theme| theme.name.eq_ignore_ascii_case(name.trim()))
}

/// Which of a theme's [`Palette::senders`] a user's name gets: always
/// the same one for the same user.
pub fn sender_slot(user_id: &str) -> usize {
    let hash = user_id.bytes().fold(0u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u32)
    });
    hash as usize % SENDER_COLORS
}

impl Palette {
    /// The colour of a sender's name.
    pub fn sender_color(&self, user_id: &str) -> Rgb {
        self.senders[sender_slot(user_id)]
    }
}

/// A theme of the user's: its `base` (a built-in theme, the default if
/// none or unknown) with the colours given changed. What cannot be read
/// comes back as a message.
pub fn custom(entry: &CustomTheme) -> Result<Palette, String> {
    let name = &entry.name;
    if name.trim().is_empty() {
        return Err("[[themes]]: a theme needs a name".to_owned());
    }
    let base = find(BUILT_IN, &entry.base).unwrap_or(&BUILT_IN[0]);
    let mut theme = Palette {
        name: Cow::Owned(name.trim().to_owned()),
        ..base.clone()
    };
    let colors = [
        ("background", &entry.background, &mut theme.background),
        ("fg", &entry.fg, &mut theme.fg),
        ("dim", &entry.dim, &mut theme.dim),
        ("accent", &entry.accent, &mut theme.accent),
        ("border", &entry.border, &mut theme.border),
        ("selection_bg", &entry.selection_bg, &mut theme.selection_bg),
        ("selection_fg", &entry.selection_fg, &mut theme.selection_fg),
        ("unread", &entry.unread, &mut theme.unread),
        ("mention", &entry.mention, &mut theme.mention),
        ("error", &entry.error, &mut theme.error),
        ("success", &entry.success, &mut theme.success),
    ];
    for (key, value, slot) in colors {
        let Some(value) = value else { continue };
        *slot = Rgb::parse(value).ok_or_else(|| {
            format!("theme '{name}': {key} = '{value}' is not a colour (#rrggbb)")
        })?;
    }
    if !entry.senders.is_empty() {
        let parsed: Option<Vec<Rgb>> = entry.senders.iter().map(|v| Rgb::parse(v)).collect();
        let parsed =
            parsed.ok_or_else(|| format!("theme '{name}': senders must be colours (#rrggbb)"))?;
        for (slot, color) in theme.senders.iter_mut().zip(parsed.iter().cycle()) {
            *slot = *color;
        }
    }
    Ok(theme)
}

/// The user's own themes, in the order written, and what could not be
/// read of them (those themes are left out).
pub fn own(entries: &[CustomTheme]) -> (Vec<Palette>, Vec<String>) {
    let mut themes = Vec::new();
    let mut problems = Vec::new();
    for entry in entries {
        match custom(entry) {
            Ok(theme) => themes.push(theme),
            Err(problem) => problems.push(problem),
        }
    }
    (themes, problems)
}

/// Every theme to choose from: the built-in ones, then the user's own.
/// Problems with the own ones as in [`own`].
pub fn all(entries: &[CustomTheme]) -> (Vec<Palette>, Vec<String>) {
    let (own, problems) = own(entries);
    let mut themes = BUILT_IN.to_vec();
    themes.extend(own);
    (themes, problems)
}

impl Config {
    /// Every theme to choose from, built-in and own ([`all`]).
    pub fn all_themes(&self) -> (Vec<Palette>, Vec<String>) {
        all(&self.themes)
    }

    /// The theme chosen (`theme = "…"`), or the default.
    pub fn palette(&self) -> Palette {
        let (themes, _) = self.all_themes();
        find(&themes, &self.theme)
            .cloned()
            .unwrap_or_else(default_palette)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(toml_text: &str) -> CustomTheme {
        toml::from_str(toml_text).unwrap()
    }

    #[test]
    fn built_in_themes_are_found_by_name_whatever_the_case() {
        for theme in BUILT_IN {
            assert_eq!(find(BUILT_IN, &theme.name.to_uppercase()), Some(theme));
        }
        assert_eq!(find(BUILT_IN, " Nord ").unwrap().name, "nord");
        assert_eq!(find(BUILT_IN, "nonesuch"), None);
        assert_eq!(default_palette().name, "mothrix");
        assert_eq!(BUILT_IN[0].background, Rgb(0x14, 0x0a, 0x1e));
    }

    #[test]
    fn an_own_theme_changes_the_colours_given_and_keeps_the_rest() {
        let theme = custom(&entry(
            r##"
            name = " mine "
            base = "Gruvbox"
            accent = "#ff8800"
            fg = "#FFFFFF"
            background = "#fbf1c7"
            senders = ["#010203", "#040506"]
            "##,
        ))
        .unwrap();
        let base = find(BUILT_IN, "gruvbox").unwrap();
        assert_eq!(theme.name, "mine");
        assert_eq!(theme.accent, Rgb(0xff, 0x88, 0x00));
        assert_eq!(theme.fg, Rgb(255, 255, 255));
        assert_eq!(theme.background, Rgb(0xfb, 0xf1, 0xc7));
        assert_eq!(theme.dim, base.dim);
        // Fewer sender colours than slots: they repeat.
        assert_eq!(theme.senders[0], Rgb(1, 2, 3));
        assert_eq!(theme.senders[1], Rgb(4, 5, 6));
        assert_eq!(theme.senders[2], Rgb(1, 2, 3));
    }

    #[test]
    fn an_own_theme_without_a_known_base_starts_from_the_default() {
        let theme = custom(&entry(r#"name = "plain""#)).unwrap();
        assert_eq!(theme.background, BUILT_IN[0].background);
        assert_eq!(theme.senders, BUILT_IN[0].senders);
    }

    #[test]
    fn what_cannot_be_read_is_reported_and_left_out() {
        let config: Config = toml::from_str(
            r##"
            theme = "MINE"
            [[themes]]
            name = "bad"
            accent = "orange"
            [[themes]]
            name = ""
            [[themes]]
            name = "worse"
            senders = ["#123456", "blue"]
            [[themes]]
            name = "mine"
            background = "#ffffff"
            "##,
        )
        .unwrap();
        let (themes, problems) = config.all_themes();
        assert_eq!(
            problems,
            [
                "theme 'bad': accent = 'orange' is not a colour (#rrggbb)",
                "[[themes]]: a theme needs a name",
                "theme 'worse': senders must be colours (#rrggbb)",
            ]
        );
        let names: Vec<&str> = themes.iter().map(|theme| &*theme.name).collect();
        assert_eq!(
            names,
            [
                "mothrix",
                "tokyo-night",
                "catppuccin",
                "gruvbox",
                "nord",
                "everforest",
                "rose-pine",
                "mine"
            ]
        );
        assert_eq!(config.palette().name, "mine");
        assert_eq!(config.palette().background, Rgb(255, 255, 255));
        assert_eq!(Config::default().palette(), BUILT_IN[0]);
    }

    #[test]
    fn a_palette_goes_to_json_with_hex_colours() {
        let json = serde_json::to_value(find(BUILT_IN, "nord").unwrap()).unwrap();
        assert_eq!(json["name"], "nord");
        assert_eq!(json["background"], "#2e3440");
        assert_eq!(json["selection_bg"], "#3b4252");
        assert_eq!(json["senders"][5], "#d08770");
        assert_eq!(json["senders"].as_array().unwrap().len(), SENDER_COLORS);
        assert_eq!(Rgb(1, 0xab, 0xff).to_string(), "#01abff");
        assert_eq!(Rgb::parse(" #01ABff "), Some(Rgb(1, 0xab, 0xff)));
        assert_eq!(Rgb::parse("#01abf"), None);
        assert_eq!(Rgb::parse("01abff"), None);
    }

    #[test]
    fn a_sender_keeps_their_colour() {
        // The hash the terminal has always used: names keep their colour.
        assert_eq!(sender_slot(""), 0);
        assert_eq!(sender_slot("@amy:example.org"), 5);
        assert_eq!(sender_slot("@dan:example.org"), 1);
        assert_eq!(sender_slot("@eve:example.org"), 4);
        let theme = &BUILT_IN[0];
        assert_eq!(theme.sender_color("@dan:example.org"), theme.senders[1]);
    }
}
