// SPDX-License-Identifier: MPL-2.0

//! Pictures on the terminal: which way they are drawn (kitty graphics,
//! Sixel, half blocks, not at all), the ones held for the conversation
//! shown and the logo, the rows they take in a layout, and what has to
//! be written to the terminal for them. Nothing else asks which
//! protocol is in use to show a picture.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    style::{Color, Style},
    text::{Line, Span},
};

use crate::model::Preview;

/// How pictures are drawn in the conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Graphics {
    /// Sharp pictures through the kitty graphics protocol.
    Kitty,
    /// Sharp pictures as Sixel graphics.
    Sixel,
    /// Coloured half-block characters; works in any true-colour terminal.
    #[default]
    Blocks,
    Off,
}

impl Graphics {
    /// `auto` (or anything unknown) picks kitty graphics where supported.
    pub fn from_label(label: &str, kitty_supported: bool, sixel_supported: bool) -> Self {
        match label.trim() {
            "kitty" => Self::Kitty,
            "sixel" => Self::Sixel,
            "blocks" => Self::Blocks,
            "off" => Self::Off,
            _ if kitty_supported => Self::Kitty,
            _ if sixel_supported => Self::Sixel,
            _ => Self::Blocks,
        }
    }
}

/// A Sixel picture and the cells it fills on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: u16,
    pub y: u16,
    pub columns: u16,
    pub rows: u16,
    /// Which picture: its message's event id, or the logo's key.
    pub key: String,
    /// The message is selected (its background differs).
    pub selected: bool,
}

/// How a picture goes into a layout ([`Pictures::drawn`]).
pub enum Drawn {
    /// As these lines (placeholder cells for kitty, or half blocks).
    Lines(Vec<Line<'static>>),
    /// Painted over this many rows and columns kept free for it
    /// ([`Pictures::place`] says where, once all of them are in view).
    Painted { rows: usize, columns: usize },
}

#[derive(Debug)]
pub struct Pictures {
    /// The way in use: what the setting asks for and the terminal can do.
    pub graphics: Graphics,
    /// Whether the terminal can show pictures itself (kitty graphics).
    pub kitty_supported: bool,
    /// Whether the terminal is known to draw Sixel graphics.
    pub sixel_supported: bool,
    /// The pictures held, by their message's event id (or the logo's key).
    previews: HashMap<String, Preview>,
    /// Those the terminal has a copy of (kitty), with its id there.
    kitty_ids: HashMap<String, u32>,
    next_kitty_id: u32,
    /// Those made into Sixel sequences.
    sixel: HashMap<String, String>,
    /// The size of a terminal cell in pixels, for Sixel pictures.
    cell_px: (u16, u16),
    /// For which way of drawing (and cell size) the logo was made.
    logo_for: Option<(Graphics, (u16, u16))>,
    /// Where the frame being drawn wants Sixel pictures.
    placements: Vec<Placement>,
    /// The Sixel pictures painted on the terminal right now.
    painted: Vec<Placement>,
}

impl Default for Pictures {
    fn default() -> Self {
        Self {
            graphics: Graphics::default(),
            kitty_supported: false,
            sixel_supported: false,
            previews: HashMap::new(),
            kitty_ids: HashMap::new(),
            next_kitty_id: 1,
            sixel: HashMap::new(),
            cell_px: (10, 20),
            logo_for: None,
            placements: Vec::new(),
            painted: Vec::new(),
        }
    }
}

impl Pictures {
    /// Follow the picture setting (`auto`, `kitty`, `sixel`, `blocks`,
    /// `off`). Pictures loaded from now on are drawn that way.
    pub fn set_mode(&mut self, label: &str) {
        self.graphics = Graphics::from_label(label, self.kitty_supported, self.sixel_supported);
    }

    /// The terminal's cells have this size in pixels (Sixel pictures are
    /// made to fit them).
    pub fn set_cell_px(&mut self, cell_px: (u16, u16)) {
        self.cell_px = cell_px;
    }

    /// Take a picture in, under `key`. Returns what to write to the
    /// terminal for it (kitty gets its copy).
    pub fn add(&mut self, key: String, mut preview: Preview) -> Option<String> {
        if self.graphics == Graphics::Off {
            return None;
        }
        let mut sequence = None;
        if !preview.png.is_empty() {
            match self.graphics {
                Graphics::Sixel => {
                    if let Some(sixel) = crate::sixel::for_preview(
                        &preview.png,
                        preview.png_columns,
                        preview.png_rows,
                        self.cell_px,
                    ) {
                        self.sixel.insert(key.clone(), sixel);
                    }
                }
                Graphics::Kitty => {
                    let id = self.next_kitty_id;
                    self.next_kitty_id += 1;
                    sequence = Some(crate::kitty::transmit(
                        id,
                        &preview.png,
                        preview.png_columns,
                        preview.png_rows,
                    ));
                    self.kitty_ids.insert(key.clone(), id);
                }
                Graphics::Blocks | Graphics::Off => {}
            }
        }
        // The terminal has its copy now (or does not want one).
        preview.png = Vec::new();
        self.previews.insert(key, preview);
        sequence
    }

    /// Let go of the conversation's pictures (the logo stays). Returns
    /// what to write to the terminal so that it lets go of its copies.
    pub fn clear(&mut self) -> Vec<String> {
        let logo = crate::logo::KEY;
        self.previews.retain(|key, _| key == logo);
        self.sixel.retain(|key, _| key == logo);
        let gone: Vec<String> = self
            .kitty_ids
            .keys()
            .filter(|key| *key != logo)
            .cloned()
            .collect();
        gone.iter()
            .filter_map(|key| self.kitty_ids.remove(key))
            .map(crate::kitty::delete)
            .collect()
    }

    /// Have the logo of the start screen ready, made for how pictures are
    /// drawn now (again when that changed). Nothing when pictures are
    /// off: the start screen shows the text moth then. Returns what to
    /// write to the terminal for it.
    pub fn ensure_logo(&mut self) -> Option<String> {
        let now = (self.graphics, self.cell_px);
        if self.graphics == Graphics::Off || self.logo_for == Some(now) {
            return None;
        }
        self.logo_for = Some(now);
        let key = crate::logo::KEY;
        self.previews.remove(key);
        self.sixel.remove(key);
        let stale = self.kitty_ids.remove(key).map(crate::kitty::delete);
        let fresh = crate::logo::preview(self.graphics == Graphics::Kitty)
            .and_then(|preview| self.add(key.to_owned(), preview));
        match (stale, fresh) {
            (Some(stale), Some(fresh)) => Some(stale + fresh.as_str()),
            (stale, fresh) => stale.or(fresh),
        }
    }

    /// The rows and columns the picture under `key` takes.
    pub fn size(&self, key: &str) -> Option<(usize, usize)> {
        let preview = self.previews.get(key)?;
        Some(match self.graphics {
            Graphics::Kitty | Graphics::Sixel => (preview.png_rows, preview.png_columns),
            Graphics::Blocks | Graphics::Off => (preview.rows(), preview.width),
        })
    }

    /// The picture under `key` for a layout with room for `max_width`
    /// cells of it. Lines are indented by two cells, as in a message.
    pub fn drawn(&self, key: &str, max_width: usize) -> Option<Drawn> {
        let preview = self.previews.get(key)?;
        if self.sixel.contains_key(key) && preview.png_columns <= max_width {
            return Some(Drawn::Painted {
                rows: preview.png_rows,
                columns: preview.png_columns,
            });
        }
        Some(Drawn::Lines(match self.kitty_ids.get(key) {
            Some(id) => placeholder_lines(preview, *id, max_width),
            None => block_lines(preview, max_width),
        }))
    }

    /// A new frame is drawn: nothing is placed yet.
    pub fn start_frame(&mut self) {
        self.placements.clear();
    }

    /// The frame being drawn keeps these cells free for a painted picture.
    pub fn place(&mut self, placement: Placement) {
        self.placements.push(placement);
    }

    /// The placements of the frame just drawn into `buffer` that can be
    /// painted: those whose cells are all blank (nothing covers them).
    pub fn visible(&self, buffer: &Buffer) -> Vec<Placement> {
        let free = |placement: &Placement| {
            (placement.y..placement.y + placement.rows).all(|y| {
                (placement.x..placement.x + placement.columns)
                    .all(|x| buffer.cell((x, y)).is_some_and(|cell| cell.symbol() == " "))
            })
        };
        self.placements
            .iter()
            .filter(|placement| free(placement))
            .cloned()
            .collect()
    }

    /// Whether the painted pictures are no longer the `visible` ones.
    /// Sixel pictures sit on top of the text: then the screen is to be
    /// wiped, drawn again, and [`Pictures::paint`]ed.
    pub fn stale(&self, visible: &[Placement]) -> bool {
        self.graphics == Graphics::Sixel && self.painted != visible
    }

    /// What to write to the terminal to paint `visible` onto a frame
    /// freshly drawn; the cursor stays where the frame put it.
    pub fn paint(&mut self, visible: Vec<Placement>) -> String {
        let mut out = String::from("\x1b7");
        for placement in &visible {
            if let Some(sixel) = self.sixel.get(&placement.key) {
                out.push_str(&format!("\x1b[{};{}H", placement.y + 1, placement.x + 1));
                out.push_str(sixel);
            }
        }
        out.push_str("\x1b8");
        self.painted = visible;
        out
    }

    /// What to write to the terminal when mothrix leaves it.
    pub fn leave(&self) -> Option<String> {
        (self.graphics == Graphics::Kitty).then(crate::kitty::delete_all)
    }
}

/// Reserve the cells for a picture the terminal draws itself (kitty
/// graphics): each cell holds a placeholder that names the picture (by
/// its colour) and which part of it belongs there.
fn placeholder_lines(preview: &Preview, id: u32, max_width: usize) -> Vec<Line<'static>> {
    let columns = preview
        .png_columns
        .min(max_width)
        .min(crate::kitty::MAX_CELLS);
    let (r, g, b) = crate::kitty::id_color(id);
    let style = Style::new().fg(Color::Rgb(r, g, b));
    (0..preview.png_rows.min(crate::kitty::MAX_CELLS))
        .map(|row| {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(
                (0..columns)
                    .map(|column| Span::styled(crate::kitty::placeholder(row, column), style)),
            );
            Line::from(spans)
        })
        .collect()
}

/// Draw a picture with half-block characters: each cell shows the upper
/// pixel as foreground and the lower one as background of `▀`.
fn block_lines(preview: &Preview, max_width: usize) -> Vec<Line<'static>> {
    let width = preview.width.min(max_width);
    let visible = |pixel: [u8; 4]| pixel[3] >= 128;
    let color = |pixel: [u8; 4]| Color::Rgb(pixel[0], pixel[1], pixel[2]);
    (0..preview.rows())
        .map(|row| {
            let mut spans = vec![Span::raw("  ")];
            for x in 0..width {
                let top = preview.pixel(x, row * 2);
                let bottom = preview.pixel(x, row * 2 + 1);
                spans.push(match (visible(top), visible(bottom)) {
                    (true, true) => {
                        Span::styled("▀", Style::new().fg(color(top)).bg(color(bottom)))
                    }
                    (true, false) => Span::styled("▀", Style::new().fg(color(top))),
                    (false, true) => Span::styled("▄", Style::new().fg(color(bottom))),
                    (false, false) => Span::raw(" "),
                });
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::*;

    fn preview() -> Preview {
        Preview {
            width: 2,
            height: 2,
            pixels: vec![[1, 2, 3, 255]; 4],
            png: b"png".to_vec(),
            png_columns: 6,
            png_rows: 2,
        }
    }

    fn pictures(graphics: Graphics) -> Pictures {
        Pictures {
            graphics,
            ..Pictures::default()
        }
    }

    fn text(drawn: Option<Drawn>) -> Vec<String> {
        match drawn {
            Some(Drawn::Lines(lines)) => lines.iter().map(ToString::to_string).collect(),
            _ => panic!("lines"),
        }
    }

    #[test]
    fn half_blocks_need_nothing_of_the_terminal() {
        let mut pictures = pictures(Graphics::Blocks);
        assert_eq!(pictures.add("$a".into(), preview()), None);
        assert_eq!(pictures.size("$a"), Some((1, 2)));
        assert_eq!(text(pictures.drawn("$a", 40)), ["  ▀▀"]);
        // No wider than there is room.
        assert_eq!(text(pictures.drawn("$a", 1)), ["  ▀"]);
        assert!(pictures.drawn("$other", 40).is_none());
        assert!(pictures.clear().is_empty());
        assert!(pictures.drawn("$a", 40).is_none());
        assert_eq!(pictures.leave(), None);
    }

    #[test]
    fn pictures_switched_off_are_not_kept() {
        let mut pictures = pictures(Graphics::Off);
        assert_eq!(pictures.add("$a".into(), preview()), None);
        assert!(pictures.drawn("$a", 40).is_none());
        assert_eq!(pictures.ensure_logo(), None);
    }

    #[test]
    fn kitty_gets_each_picture_once_and_is_told_to_let_go() {
        let mut pictures = pictures(Graphics::Kitty);
        let first = pictures.add("$a".into(), preview()).expect("sent");
        assert!(first.contains("i=1,c=6,r=2"), "{first:?}");
        let second = pictures.add("$b".into(), preview()).expect("sent");
        assert!(second.contains("i=2,"), "{second:?}");
        assert_eq!(pictures.size("$a"), Some((2, 6)));
        // Placeholder cells: two rows of six.
        let rows = text(pictures.drawn("$a", 40));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].matches('\u{10EEEE}').count(), 6);

        // The logo stays through a change of room.
        let logo = pictures.ensure_logo();
        assert!(logo.is_some_and(|sequence| sequence.contains("i=3,")));
        assert_eq!(pictures.ensure_logo(), None);
        let mut gone = pictures.clear();
        gone.sort();
        assert_eq!(gone, [crate::kitty::delete(1), crate::kitty::delete(2)]);
        assert!(pictures.drawn("$a", 40).is_none());
        assert!(pictures.drawn(crate::logo::KEY, 200).is_some());
        assert_eq!(pictures.ensure_logo(), None);
        assert_eq!(pictures.leave(), Some(crate::kitty::delete_all()));

        // Another way of drawing: the logo is made anew, the old one goes.
        pictures.graphics = Graphics::Blocks;
        assert_eq!(pictures.ensure_logo(), Some(crate::kitty::delete(3)));
    }

    #[test]
    fn sixel_pictures_are_painted_where_nothing_covers_them() {
        let mut pictures = pictures(Graphics::Sixel);
        // A picture that is a Sixel sequence already.
        pictures.previews.insert("$a".into(), preview());
        pictures.sixel.insert("$a".into(), "SIXEL".into());
        assert!(matches!(
            pictures.drawn("$a", 6),
            Some(Drawn::Painted {
                rows: 2,
                columns: 6
            })
        ));
        // Too wide for the room there is: half blocks instead.
        assert!(matches!(pictures.drawn("$a", 5), Some(Drawn::Lines(_))));

        let placement = |x: u16, key: &str| Placement {
            x,
            y: 1,
            columns: 6,
            rows: 2,
            key: key.into(),
            selected: false,
        };
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 5));
        pictures.start_frame();
        pictures.place(placement(2, "$a"));
        pictures.place(placement(20, "$a"));
        // Something (a popup) is drawn into the second one's cells.
        buffer.set_string(22, 2, "x", Style::new());
        let visible = pictures.visible(&buffer);
        assert_eq!(visible, [placement(2, "$a")]);

        assert!(pictures.stale(&visible));
        assert_eq!(pictures.paint(visible.clone()), "\x1b7\x1b[2;3HSIXEL\x1b8");
        assert!(!pictures.stale(&visible), "painted: nothing to do");
        // It scrolled out of view: the screen is to be wiped.
        pictures.start_frame();
        assert!(pictures.stale(&pictures.visible(&buffer)));

        // Without Sixel nothing is ever painted over the text.
        pictures.graphics = Graphics::Blocks;
        assert!(!pictures.stale(&[placement(2, "$a")]));
    }
}
