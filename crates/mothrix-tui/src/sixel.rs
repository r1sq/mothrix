// SPDX-License-Identifier: MPL-2.0

//! Pictures as Sixel graphics, for terminals that draw those (foot,
//! WezTerm, Konsole, xterm in VT340 mode, Windows Terminal ...) but do not
//! speak the kitty graphics protocol. A picture becomes one escape
//! sequence that paints it at the cursor.

use std::fmt::Write;

use image::RgbaImage;

/// Most colours a Sixel picture can use here.
const COLORS: usize = 255;

/// Whether the terminal is known to draw Sixel. There is no way to see
/// this without asking the terminal, so this goes by its name; `images =
/// "sixel"` in the config switches it on anywhere.
pub fn supported() -> bool {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let term = var("TERM");
    let program = var("TERM_PROGRAM");
    term.contains("foot")
        || term.contains("mlterm")
        || term.contains("sixel")
        || term.contains("contour")
        || matches!(program.as_str(), "WezTerm" | "iTerm.app" | "contour")
        || !var("WT_SESSION").is_empty()
        || var("KONSOLE_VERSION")
            .parse::<u32>()
            .is_ok_and(|version| version >= 220400)
}

/// Encode a picture. Transparent pixels are left unpainted.
pub fn encode(picture: &RgbaImage) -> String {
    let (width, height) = (picture.width() as usize, picture.height() as usize);
    if width == 0 || height == 0 {
        return String::new();
    }
    // Reduce to a palette; the picture's own colours when they are few.
    let quantizer = color_quant::NeuQuant::new(10, COLORS, picture.as_raw());
    let palette = quantizer.color_map_rgba();
    let indices: Vec<Option<u8>> = picture
        .pixels()
        .map(|pixel| (pixel[3] >= 128).then(|| quantizer.index_of(&pixel.0) as u8))
        .collect();

    // P2 = 1: pixels that are not painted keep what is there.
    let mut out = String::with_capacity(width * height / 2);
    let _ = write!(out, "\x1bP0;1;0q\"1;1;{width};{height}");
    for (index, color) in palette.as_chunks::<4>().0.iter().enumerate() {
        let percent = |value: u8| (u32::from(value) * 100 + 127) / 255;
        let _ = write!(
            out,
            "#{index};2;{};{};{}",
            percent(color[0]),
            percent(color[1]),
            percent(color[2])
        );
    }

    // Six pixel rows at a time; within a band, one pass per colour in it.
    let mut used = vec![false; COLORS + 1];
    let mut row = vec![0u8; width];
    for band in (0..height).step_by(6) {
        used.iter_mut().for_each(|flag| *flag = false);
        let rows = (height - band).min(6);
        for index in indices[band * width..(band + rows) * width]
            .iter()
            .flatten()
        {
            used[*index as usize] = true;
        }
        let mut first = true;
        for color in (0..=COLORS).filter(|color| used[*color]) {
            row.iter_mut().for_each(|bits| *bits = 0);
            for bit in 0..rows {
                let line = &indices[(band + bit) * width..(band + bit + 1) * width];
                for (x, index) in line.iter().enumerate() {
                    if *index == Some(color as u8) {
                        row[x] |= 1 << bit;
                    }
                }
            }
            if !first {
                // Back to the start of the band for the next colour.
                out.push('$');
            }
            first = false;
            let _ = write!(out, "#{color}");
            // Trailing empty columns need not be written.
            let end = row.iter().rposition(|bits| *bits != 0).map_or(0, |x| x + 1);
            let mut x = 0;
            while x < end {
                let bits = row[x];
                let run = row[x..end]
                    .iter()
                    .take_while(|other| **other == bits)
                    .count();
                let symbol = char::from(63 + bits);
                if run > 3 {
                    let _ = write!(out, "!{run}{symbol}");
                } else {
                    for _ in 0..run {
                        out.push(symbol);
                    }
                }
                x += run;
            }
        }
        out.push('-');
    }
    out.push_str("\x1b\\");
    out
}

/// The picture of a preview, scaled to fill its box of cells on a terminal
/// whose cells are `cell` pixels in size.
pub fn for_preview(png: &[u8], columns: usize, rows: usize, cell: (u16, u16)) -> Option<String> {
    let picture = image::load_from_memory(png).ok()?;
    let width = (columns as u32 * u32::from(cell.0)).clamp(1, 2000);
    let height = (rows as u32 * u32::from(cell.1)).clamp(1, 2000);
    let scaled = picture
        .resize_exact(width, height, image::imageops::FilterType::Triangle)
        .to_rgba8();
    Some(encode(&scaled))
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    #[test]
    fn a_picture_becomes_one_sixel_sequence() {
        // Left half red, right half blue, 8 pixels high: two bands.
        let mut picture = RgbaImage::new(4, 8);
        for (x, _, pixel) in picture.enumerate_pixels_mut() {
            *pixel = if x < 2 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            };
        }
        let sixel = encode(&picture);
        assert!(sixel.starts_with("\x1bP0;1;0q\"1;1;4;8"), "{sixel:?}");
        assert!(sixel.ends_with("\x1b\\"));
        // Two bands, each closed with "-".
        assert_eq!(sixel.matches('-').count(), 2);
        // Full columns in the first band are "~" (all six bits), and the
        // second band has two rows: "B" (bits 0 and 1).
        assert!(sixel.contains("~~"), "{sixel:?}");
        assert!(sixel.contains("BB"), "{sixel:?}");
        // Both colours are painted in each band, with a return in between.
        assert_eq!(sixel.matches('$').count(), 2);
    }

    #[test]
    fn transparent_pixels_are_not_painted_and_runs_are_shortened() {
        let mut picture = RgbaImage::new(40, 6);
        for (x, _, pixel) in picture.enumerate_pixels_mut() {
            *pixel = if x < 30 {
                Rgba([0, 200, 0, 255])
            } else {
                Rgba([0, 0, 0, 0])
            };
        }
        let sixel = encode(&picture);
        // Thirty full columns as one run; nothing for the transparent rest.
        assert!(sixel.contains("!30~"), "{sixel:?}");
        assert_eq!(sixel.matches('$').count(), 0);
        assert_eq!(encode(&RgbaImage::new(0, 0)), "");
    }
}
