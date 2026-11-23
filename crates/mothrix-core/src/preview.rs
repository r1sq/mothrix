// SPDX-License-Identifier: MPL-2.0

//! Turn downloaded images into small [`Preview`]s for the conversation view.

use image::{GenericImageView, imageops::FilterType};

use crate::model::Preview;

/// Largest preview, in terminal cells. Two pixels fit into one cell
/// vertically, so this is 48 x 28 pixels.
pub const MAX_COLS: u32 = 48;
pub const MAX_ROWS: u32 = 14;

/// Box for the sharper version shown through the kitty graphics protocol.
const HD_MAX_COLS: u32 = 60;
const HD_MAX_ROWS: u32 = 18;
/// How far a small picture may be enlarged.
const HD_MAX_UPSCALE: f64 = 3.0;
/// Pixels sent per cell column (terminal cells are roughly 10 px wide).
const HD_CELL_WIDTH_PX: u32 = 12;

/// Decode `bytes`, turned as the camera said it was held (EXIF
/// orientation: phones store a photo as the sensor saw it and say how to
/// turn it).
fn decode_upright(bytes: &[u8]) -> Option<image::DynamicImage> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().ok();
    let mut picture = image::DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        picture.apply_orientation(orientation);
    }
    Some(picture)
}

/// Decode `bytes` and scale the picture down to at most [`MAX_COLS`] x
/// [`MAX_ROWS`] cells, keeping its aspect ratio. Small pictures are not
/// blown up. Returns `None` for anything the decoder rejects.
pub fn make_preview(bytes: &[u8]) -> Option<Preview> {
    let picture = decode_upright(bytes)?;
    let (width, height) = picture.dimensions();
    if width == 0 || height == 0 {
        return None;
    }

    // Terminal cells are about twice as tall as wide, and a cell holds two
    // stacked pixels, so preview pixels are roughly square.
    let scale = (MAX_COLS as f64 / width as f64)
        .min((MAX_ROWS * 2) as f64 / height as f64)
        .min(1.0);
    let target_width = ((width as f64 * scale).round() as u32).max(1);
    let mut target_height = ((height as f64 * scale).round() as u32).max(2);
    target_height += target_height % 2;

    let scaled = picture
        .resize_exact(target_width, target_height, FilterType::Triangle)
        .to_rgba8();

    // The sharper version for terminals with real graphics: a box of cells
    // with the picture's shape (a cell is about twice as tall as wide), and
    // a PNG with enough pixels to fill it crisply.
    let box_scale =
        (HD_MAX_COLS as f64 / width as f64).min((HD_MAX_ROWS * 2) as f64 / height as f64);
    // Tiny pictures may grow, but not into a blur.
    let box_scale = box_scale.min(HD_MAX_UPSCALE);
    let png_columns = ((width as f64 * box_scale).round() as u32).clamp(2, HD_MAX_COLS);
    let png_rows = ((height as f64 * box_scale / 2.0).round() as u32).clamp(1, HD_MAX_ROWS);
    let detail = picture.resize(
        (png_columns * HD_CELL_WIDTH_PX).min(width.max(png_columns * 4)),
        (png_rows * HD_CELL_WIDTH_PX * 2).min(height.max(png_rows * 8)),
        FilterType::Triangle,
    );
    let mut png = Vec::new();
    if detail
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .is_err()
    {
        png.clear();
    }

    Some(Preview {
        width: target_width as usize,
        height: target_height as usize,
        pixels: scaled.pixels().map(|pixel| pixel.0).collect(),
        png,
        png_columns: png_columns as usize,
        png_rows: png_rows as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut picture = image::RgbaImage::new(width, height);
        for (x, _, pixel) in picture.enumerate_pixels_mut() {
            *pixel = if x < width / 2 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            };
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(picture)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("encodable");
        bytes
    }

    #[test]
    fn large_pictures_are_scaled_into_the_box_keeping_their_shape() {
        let preview = make_preview(&png(960, 480)).expect("decodes");
        assert_eq!((preview.width, preview.height), (48, 24));
        assert_eq!(preview.rows(), 12);
        assert_eq!(preview.pixels.len(), 48 * 24);
        assert_eq!(preview.pixel(2, 5), [255, 0, 0, 255]);
        assert_eq!(preview.pixel(45, 5), [0, 0, 255, 255]);

        // The sharper version fills a 60 x 15 cell box with a real PNG.
        assert_eq!((preview.png_columns, preview.png_rows), (60, 15));
        let detail = image::load_from_memory(&preview.png).expect("valid png");
        assert_eq!(detail.dimensions(), (720, 360));

        let tall = make_preview(&png(100, 1000)).expect("decodes");
        assert_eq!(tall.height, 28);
        assert!(tall.width <= 4);
    }

    #[test]
    fn small_pictures_keep_their_size_with_an_even_height() {
        let preview = make_preview(&png(10, 5)).expect("decodes");
        assert_eq!((preview.width, preview.height), (10, 6));
        // Enlarged at most three times, and never beyond its own pixels.
        assert_eq!((preview.png_columns, preview.png_rows), (30, 8));
        let detail = image::load_from_memory(&preview.png).expect("valid png");
        assert!(
            detail.width() >= 10 && detail.width() <= 120,
            "{}",
            detail.width()
        );
    }

    /// A JPEG, left half red and right half blue, whose EXIF says it is
    /// upside down (orientation 3).
    fn jpeg_upside_down(width: u32, height: u32) -> Vec<u8> {
        let picture = image::load_from_memory(&png(width, height))
            .unwrap()
            .to_rgb8();
        let mut plain = Vec::new();
        image::DynamicImage::ImageRgb8(picture)
            .write_to(
                &mut std::io::Cursor::new(&mut plain),
                image::ImageFormat::Jpeg,
            )
            .expect("encodable");
        // An APP1 segment: "Exif", a little-endian TIFF header and one IFD
        // entry, Orientation (0x0112), SHORT, 1, value 3.
        let mut tiff = b"II*\0\x08\0\0\0".to_vec();
        tiff.extend([1, 0]);
        tiff.extend([0x12, 0x01, 3, 0, 1, 0, 0, 0, 3, 0, 0, 0]);
        tiff.extend([0, 0, 0, 0]);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(tiff);
        let length = (app1.len() + 2) as u16;
        let mut bytes = plain[..2].to_vec();
        bytes.extend([0xFF, 0xE1]);
        bytes.extend(length.to_be_bytes());
        bytes.extend(app1);
        bytes.extend(&plain[2..]);
        bytes
    }

    #[test]
    fn a_photo_is_turned_as_its_exif_says() {
        let preview = make_preview(&jpeg_upside_down(40, 20)).expect("decodes");
        // Turned half round: blue now on the left, red on the right.
        let [r, _, b, _] = preview.pixel(2, 5);
        assert!(b > 200 && r < 60, "left is {r},{b}");
        let [r, _, b, _] = preview.pixel(37, 5);
        assert!(r > 200 && b < 60, "right is {r},{b}");
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(make_preview(b"not an image").is_none());
    }
}
