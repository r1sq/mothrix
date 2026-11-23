// SPDX-License-Identifier: MPL-2.0

//! The text moth of the terminal's start screen, drawn in its theme's
//! colours where the logo cannot be (pictures off, too little room).

/// The moth: rounded upper wings with eye spots, lower wings of slashes
/// with diamonds between them, under a moon. Every row is as wide as the
/// widest, so that centring keeps it in one piece.
pub const MOTH: [&str; 10] = [
    r"             .  )  .             ",
    r"   *    \               /    *   ",
    r"         \     ___     /         ",
    r"  .--~~--.  \ (@ @) /  .--~~--.  ",
    r" /   .◇.   \   |=|   /   .◇.   \ ",
    r"|   ( ◆ )   \_ |=| _/   ( ◆ )   |",
    r" \   `◇'   .-' |=| `-.   `◇'   / ",
    r"  `-.___.-'    |=|    `-.___.-'  ",
    r"        / ◆ /  |=|  \ ◆ \        ",
    r"       / ◇ /  . V .  \ ◇ \       ",
];
pub const MOTH_WIDTH: usize = 33;

/// Which colour a character of the moth takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// The wings: the theme's accent.
    Wing,
    /// The body: plain text colour.
    Body,
    /// Eyes and dark diamonds: the unread colour.
    Eye,
    /// Light diamonds: the success colour.
    Gem,
    /// Moon and stars: dim.
    Sky,
}

/// The part of the moth at this row and column.
pub fn part(row: usize, column: usize, c: char) -> Part {
    match c {
        '@' | '◆' => Part::Eye,
        '◇' => Part::Gem,
        _ if row == 0 => Part::Sky,
        '*' | '~' => Part::Sky,
        _ if (14..19).contains(&column) => Part::Body,
        _ => Part::Wing,
    }
}

/// The moth as runs of characters of one part, row by row.
pub fn moth_runs() -> Vec<Vec<(String, Part)>> {
    MOTH.iter()
        .enumerate()
        .map(|(row, text)| {
            let mut runs: Vec<(String, Part)> = Vec::new();
            for (column, c) in text.chars().enumerate() {
                let part = part(row, column, c);
                match runs.last_mut() {
                    Some((run, same)) if *same == part => run.push(c),
                    _ => runs.push((c.to_string(), part)),
                }
            }
            runs
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_is_as_wide_and_the_runs_spell_it() {
        for (row, runs) in MOTH.iter().zip(moth_runs()) {
            assert_eq!(row.chars().count(), MOTH_WIDTH);
            let joined: String = runs.iter().map(|(text, _)| text.as_str()).collect();
            assert_eq!(&joined, row);
        }
        assert_eq!(part(3, 15, '@'), Part::Eye);
        assert_eq!(part(4, 16, '='), Part::Body);
        assert_eq!(part(0, 13, '.'), Part::Sky);
    }
}
