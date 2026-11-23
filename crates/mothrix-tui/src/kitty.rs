// SPDX-License-Identifier: MPL-2.0

//! The kitty terminal graphics protocol, using its "Unicode placeholder"
//! mode: the picture is sent to the terminal once, and the place where it
//! should appear is filled with ordinary text cells holding a special
//! character. Because those are normal cells, scrolling, clipping and
//! redrawing work like for any other text.
//!
//! Protocol reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>

use base64::Engine;

/// The character kitty replaces by a piece of a picture.
const PLACEHOLDER: char = '\u{10EEEE}';

/// Combining marks that encode a row or column number (0-based index into
/// this table), from kitty's `gen/rowcolumn-diacritics.txt`. The first 128
/// entries, far more than a preview needs.
const DIACRITICS: [char; 128] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}', '\u{033F}',
    '\u{0346}', '\u{034A}', '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}',
    '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}',
    '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}', '\u{0483}', '\u{0484}',
    '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}',
    '\u{05A8}', '\u{05A9}', '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}',
    '\u{0659}', '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}', '\u{06D7}', '\u{06D8}',
    '\u{06D9}', '\u{06DA}', '\u{06DB}', '\u{06DC}', '\u{06DF}', '\u{06E0}', '\u{06E1}', '\u{06E2}',
    '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}', '\u{06EC}', '\u{0730}', '\u{0732}', '\u{0733}',
    '\u{0735}', '\u{0736}', '\u{073A}', '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', '\u{0743}',
    '\u{0745}', '\u{0747}', '\u{0749}', '\u{074A}', '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}',
    '\u{07EF}', '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}', '\u{0818}', '\u{0819}',
    '\u{081B}', '\u{081C}', '\u{081D}', '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}', '\u{0822}',
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082A}', '\u{082B}', '\u{082C}',
];

/// Largest row/column a placeholder can address with this table.
pub const MAX_CELLS: usize = DIACRITICS.len();

/// The text for the cell at (`row`, `column`) of a picture. The picture is
/// chosen by the cell's foreground colour (see [`id_color`]).
pub fn placeholder(row: usize, column: usize) -> String {
    let mut cell = String::with_capacity(12);
    cell.push(PLACEHOLDER);
    cell.push(DIACRITICS[row.min(MAX_CELLS - 1)]);
    cell.push(DIACRITICS[column.min(MAX_CELLS - 1)]);
    cell
}

/// The foreground colour (r, g, b) that selects picture `id`.
pub fn id_color(id: u32) -> (u8, u8, u8) {
    ((id >> 16) as u8, (id >> 8) as u8, id as u8)
}

/// Escape sequence that uploads a PNG as picture `id` and declares that it
/// is shown through placeholders in a box of `columns` x `rows` cells.
pub fn transmit(id: u32, png: &[u8], columns: usize, rows: usize) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let mut chunks = encoded.as_bytes().chunks(4096).peekable();
    let mut out = String::with_capacity(encoded.len() + 256);
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        // q=2: the terminal must not answer, or its reply would be read as typing.
        if first {
            out.push_str(&format!(
                "\x1b_Ga=T,U=1,f=100,q=2,i={id},c={columns},r={rows},m={more};"
            ));
            first = false;
        } else {
            out.push_str(&format!("\x1b_Gm={more};"));
        }
        // Base64 is ASCII, so this cannot fail.
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push_str("\x1b\\");
    }
    out
}

/// Escape sequence that removes picture `id` and frees its memory.
pub fn delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// Escape sequence that removes every picture mothrix uploaded.
pub fn delete_all() -> String {
    "\x1b_Ga=d,d=A,q=2\x1b\\".to_owned()
}

/// Whether the terminal we run in understands this protocol, judged by the
/// environment (asking the terminal would need a round trip at startup).
pub fn supported() -> bool {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    !var("KITTY_WINDOW_ID").is_empty()
        || var("TERM").contains("kitty")
        || var("TERM_PROGRAM").eq_ignore_ascii_case("ghostty")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_carry_row_and_column_marks() {
        let cell: Vec<char> = placeholder(0, 0).chars().collect();
        assert_eq!(cell, ['\u{10EEEE}', '\u{0305}', '\u{0305}']);
        let cell: Vec<char> = placeholder(2, 1).chars().collect();
        assert_eq!(cell, ['\u{10EEEE}', '\u{030E}', '\u{030D}']);
    }

    #[test]
    fn ids_map_to_truecolour_components() {
        assert_eq!(id_color(5), (0, 0, 5));
        assert_eq!(id_color(0x01_02_03), (1, 2, 3));
    }

    #[test]
    fn small_pictures_are_sent_in_one_chunk() {
        let sequence = transmit(7, b"png-bytes", 20, 6);
        assert!(
            sequence.starts_with("\x1b_Ga=T,U=1,f=100,q=2,i=7,c=20,r=6,m=0;"),
            "{sequence:?}"
        );
        assert!(sequence.ends_with("\x1b\\"));
        assert_eq!(sequence.matches("\x1b_G").count(), 1);
    }

    #[test]
    fn large_pictures_are_split_and_only_the_last_chunk_ends_the_upload() {
        let sequence = transmit(7, &vec![0u8; 10_000], 20, 6);
        let chunks: Vec<&str> = sequence
            .split("\x1b\\")
            .filter(|part| !part.is_empty())
            .collect();
        assert_eq!(chunks.len(), 4);
        assert!(chunks[0].contains("m=1;") && chunks[0].contains("a=T"));
        assert!(chunks[1].starts_with("\x1b_Gm=1;"));
        assert!(chunks[3].starts_with("\x1b_Gm=0;"));
        assert!(chunks.iter().all(|chunk| chunk.len() <= 4096 + 64));
    }
}
