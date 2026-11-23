// SPDX-License-Identifier: MPL-2.0

//! The mothrix logo (the moth before a ringed planet, on nothing) for the
//! start screen, as a picture like those in messages. Its soft edge
//! blends into the terminal's background where the terminal blends
//! (kitty), and becomes a fine dither where a pixel is only shown or not
//! (sixel, blocks).

use std::sync::OnceLock;

use crate::model::Preview;

/// The key the logo is kept under, next to the pictures of messages.
pub const KEY: &str = "mothrix:logo";

/// 512 px, the logo alone on a transparent background.
const PNG: &[u8] = include_bytes!("../../../assets/mothrix-logo-512.png");

/// The logo made small for the start screen. `smooth`: the terminal
/// blends partly transparent pixels itself.
pub fn preview(smooth: bool) -> Option<Preview> {
    // Made once: decoding and scaling 512 pixels square takes a moment.
    static SMOOTH: OnceLock<Option<Preview>> = OnceLock::new();
    static DITHERED: OnceLock<Option<Preview>> = OnceLock::new();
    let made = if smooth { &SMOOTH } else { &DITHERED };
    made.get_or_init(|| make(smooth)).clone()
}

/// Pixels across (and down) when drawn with half blocks: more than a
/// message's picture gets, or the moth is hard to make out.
const BLOCKS: u32 = 44;

fn make(smooth: bool) -> Option<Preview> {
    let mut preview = mothrix_core::preview::make_preview(PNG)?;
    let small = image::load_from_memory(PNG)
        .ok()?
        .resize_exact(BLOCKS, BLOCKS, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    preview.width = BLOCKS as usize;
    preview.height = BLOCKS as usize;
    preview.pixels = small.pixels().map(|pixel| pixel.0).collect();
    if !smooth {
        let width = preview.width;
        for (at, pixel) in preview.pixels.iter_mut().enumerate() {
            dither(pixel, at % width, at / width);
        }
        if !preview.png.is_empty() {
            let mut sharp = image::load_from_memory(&preview.png).ok()?.to_rgba8();
            for (x, y, pixel) in sharp.enumerate_pixels_mut() {
                dither(&mut pixel.0, x as usize, y as usize);
            }
            let mut png = Vec::new();
            image::DynamicImage::ImageRgba8(sharp)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .ok()?;
            preview.png = png;
        }
    }
    Some(preview)
}

/// Fully there or not at all, by a 4×4 ordered (Bayer) pattern: the
/// fainter a pixel, the fewer of its neighbours stay.
fn dither(pixel: &mut [u8; 4], x: usize, y: usize) {
    const BAYER: [[u16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let threshold = BAYER[y % 4][x % 4] * 16 + 8;
    pixel[3] = if u16::from(pixel[3]) > threshold {
        255
    } else {
        0
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_fills_the_middle_and_leaves_the_corners_clear() {
        let smooth = preview(true).expect("the logo decodes");
        let middle = smooth.pixel(smooth.width / 2, smooth.height / 2);
        assert!(middle[3] > 240, "the moth's body is there: {middle:?}");
        assert_eq!(smooth.pixel(0, 0)[3], 0, "a corner is gone");
        assert!(!smooth.png.is_empty());
        // Where a pixel is only shown or not, every alpha is all or nothing.
        let dithered = preview(false).expect("the logo decodes");
        assert!(dithered.pixels.iter().all(|p| p[3] == 0 || p[3] == 255));
        let sharp = image::load_from_memory(&dithered.png).unwrap().to_rgba8();
        assert!(sharp.pixels().all(|p| p.0[3] == 0 || p.0[3] == 255));
    }

    #[test]
    fn a_half_faint_stretch_keeps_about_half_its_pixels() {
        let kept = (0..4)
            .flat_map(|y| (0..4).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let mut pixel = [0, 0, 0, 128];
                dither(&mut pixel, x, y);
                pixel[3] == 255
            })
            .count();
        assert_eq!(kept, 8);
    }
}
