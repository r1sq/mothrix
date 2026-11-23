// SPDX-License-Identifier: MPL-2.0

//! Markdown formats for the message box (the format menu, `F6`), applied
//! where the cursor is, since a terminal gives programs no selection:
//! around the word at the cursor, or at the start of its line. Applying a
//! format that is there already takes it away again.

/// One format of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bold,
    Italic,
    Strike,
    Code,
    CodeBlock,
    Heading,
    Quote,
    List,
    Numbered,
    Link,
}

/// What starts a line of each kind; one replaces another.
const LINE_MARKS: [(Format, &str); 4] = [
    (Format::Heading, "# "),
    (Format::Quote, "> "),
    (Format::List, "- "),
    (Format::Numbered, "1. "),
];

impl Format {
    /// In the order the menu shows.
    pub const ALL: [Format; 10] = [
        Self::Bold,
        Self::Italic,
        Self::Strike,
        Self::Code,
        Self::CodeBlock,
        Self::Heading,
        Self::Quote,
        Self::List,
        Self::Numbered,
        Self::Link,
    ];

    /// The key that picks it in the menu.
    pub fn key(self) -> char {
        match self {
            Self::Bold => 'b',
            Self::Italic => 'i',
            Self::Strike => 's',
            Self::Code => 'c',
            Self::CodeBlock => 'k',
            Self::Heading => 'h',
            Self::Quote => 'q',
            Self::List => 'l',
            Self::Numbered => 'n',
            Self::Link => 'u',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bold => "Bold",
            Self::Italic => "Italic",
            Self::Strike => "Strikethrough",
            Self::Code => "Code",
            Self::CodeBlock => "Code block",
            Self::Heading => "Heading",
            Self::Quote => "Quote",
            Self::List => "List",
            Self::Numbered => "Numbered list",
            Self::Link => "Link",
        }
    }

    /// What it looks like as Markdown, for the menu.
    pub fn example(self) -> &'static str {
        match self {
            Self::Bold => "**word**",
            Self::Italic => "_word_",
            Self::Strike => "~~word~~",
            Self::Code => "`word`",
            Self::CodeBlock => "``` line ```",
            Self::Heading => "# line",
            Self::Quote => "> line",
            Self::List => "- line",
            Self::Numbered => "1. line",
            Self::Link => "[word](address)",
        }
    }
}

/// `text` with `format` applied at `cursor` (in chars), and where the
/// cursor is then.
pub fn apply(text: &str, cursor: usize, format: Format) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    match format {
        Format::Bold => around_word(&chars, cursor, "**"),
        Format::Italic => around_word(&chars, cursor, "_"),
        Format::Strike => around_word(&chars, cursor, "~~"),
        Format::Code => around_word(&chars, cursor, "`"),
        Format::Link => link(&chars, cursor),
        Format::CodeBlock => code_block(&chars, cursor),
        Format::Heading | Format::Quote | Format::List | Format::Numbered => {
            line_mark(&chars, cursor, format)
        }
    }
}

/// Where the word at `cursor` starts and ends (the same when there is
/// none).
fn word_at(chars: &[char], cursor: usize) -> (usize, usize) {
    let mut start = cursor;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = cursor;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    (start, end)
}

fn joined(parts: &[&[char]]) -> String {
    parts.iter().flat_map(|part| part.iter()).collect()
}

fn around_word(chars: &[char], cursor: usize, mark: &str) -> (String, usize) {
    let mark: Vec<char> = mark.chars().collect();
    let (start, end) = word_at(chars, cursor);
    let word = &chars[start..end];
    if word.is_empty() {
        // Nothing to wrap: the marks, with the cursor between them.
        let text = joined(&[&chars[..cursor], &mark, &mark, &chars[cursor..]]);
        return (text, cursor + mark.len());
    }
    let marked = word.len() >= 2 * mark.len() && word.starts_with(&mark) && word.ends_with(&mark);
    let new: Vec<char> = if marked {
        word[mark.len()..word.len() - mark.len()].to_vec()
    } else {
        [&mark[..], word, &mark[..]].concat()
    };
    let text = joined(&[&chars[..start], &new, &chars[end..]]);
    (text, start + new.len())
}

fn link(chars: &[char], cursor: usize) -> (String, usize) {
    let (start, end) = word_at(chars, cursor);
    let word: String = chars[start..end].iter().collect();
    let (new, at) = if word.is_empty() {
        ("[]()".to_owned(), 1)
    } else if word.starts_with("http://") || word.starts_with("https://") {
        // An address: the text it shows is to be typed.
        (format!("[]({word})"), 1)
    } else {
        // A word: the address is to be typed.
        let count = word.chars().count();
        (format!("[{word}]()"), count + 3)
    };
    let new: Vec<char> = new.chars().collect();
    let text = joined(&[&chars[..start], &new, &chars[end..]]);
    (text, start + at)
}

/// Where the line with the cursor starts and ends.
fn line_at(chars: &[char], cursor: usize) -> (usize, usize) {
    let start = chars[..cursor]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |at| at + 1);
    let end = chars[cursor..]
        .iter()
        .position(|c| *c == '\n')
        .map_or(chars.len(), |at| cursor + at);
    (start, end)
}

fn line_mark(chars: &[char], cursor: usize, format: Format) -> (String, usize) {
    let (start, end) = line_at(chars, cursor);
    let line = &chars[start..end];
    let mark = |format: Format| {
        LINE_MARKS
            .iter()
            .find(|(kind, _)| *kind == format)
            .map(|(_, mark)| mark.chars().collect::<Vec<char>>())
            .unwrap_or_default()
    };
    // The mark the line has, if any: taken away first.
    let had = LINE_MARKS
        .iter()
        .find(|(_, mark)| line.starts_with(&mark.chars().collect::<Vec<_>>()))
        .map(|(kind, _)| *kind);
    let removed = had.map_or(0, |kind| mark(kind).len());
    let rest = &line[removed..];
    let added = if had == Some(format) {
        Vec::new()
    } else {
        mark(format)
    };
    let text = joined(&[&chars[..start], &added, rest, &chars[end..]]);
    let cursor = (cursor.max(start + removed) - removed) + added.len();
    (text, cursor)
}

fn code_block(chars: &[char], cursor: usize) -> (String, usize) {
    let (start, end) = line_at(chars, cursor);
    let fence: Vec<char> = "```\n".chars().collect();
    let close: Vec<char> = "\n```".chars().collect();
    let text = joined(&[
        &chars[..start],
        &fence,
        &chars[start..end],
        &close,
        &chars[end..],
    ]);
    (text, cursor + fence.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply at the `|` in `marked`; the result with `|` at the cursor.
    fn at(marked: &str, format: Format) -> String {
        let cursor = marked.chars().position(|c| c == '|').unwrap();
        let text: String = marked.chars().filter(|c| *c != '|').collect();
        let (text, cursor) = apply(&text, cursor, format);
        let mut chars: Vec<char> = text.chars().collect();
        chars.insert(cursor, '|');
        chars.into_iter().collect()
    }

    #[test]
    fn inline_formats_wrap_the_word_at_the_cursor_and_unwrap_it_again() {
        assert_eq!(at("say he|llo now", Format::Bold), "say **hello**| now");
        assert_eq!(at("say **hello**| now", Format::Bold), "say hello| now");
        assert_eq!(at("hello|", Format::Italic), "_hello_|");
        assert_eq!(at("|x", Format::Strike), "~~x~~|");
        assert_eq!(at("run cargo| test", Format::Code), "run `cargo`| test");
        // No word there: the marks, the cursor between them.
        assert_eq!(at("a |", Format::Bold), "a **|**");
        assert_eq!(at("|", Format::Code), "`|`");
        // Non-ASCII words count as words.
        assert_eq!(at("grüß|e", Format::Bold), "**grüße**|");
    }

    #[test]
    fn a_link_wraps_a_word_or_an_address() {
        assert_eq!(at("see do|cs", Format::Link), "see [docs](|)");
        assert_eq!(at("https://x.org|", Format::Link), "[|](https://x.org)");
        assert_eq!(at("|", Format::Link), "[|]()");
    }

    #[test]
    fn line_formats_mark_the_line_and_replace_or_remove_another() {
        assert_eq!(
            at("one\ntw|o\nthree", Format::Heading),
            "one\n# tw|o\nthree"
        );
        assert_eq!(at("# tw|o", Format::Heading), "tw|o");
        assert_eq!(at("- it|em", Format::Numbered), "1. it|em");
        assert_eq!(at("> |quote", Format::List), "- |quote");
        assert_eq!(at("|", Format::Quote), "> |");
        // The cursor inside the mark that goes.
        assert_eq!(at("1|. item", Format::Numbered), "|item");
    }

    #[test]
    fn a_code_block_fences_the_line_or_makes_an_empty_one() {
        assert_eq!(at("|", Format::CodeBlock), "```\n|\n```");
        assert_eq!(
            at("text\nlet x| = 1;", Format::CodeBlock),
            "text\n```\nlet x| = 1;\n```"
        );
    }

    #[test]
    fn every_format_has_its_own_key() {
        let mut keys: Vec<char> = Format::ALL.iter().map(|format| format.key()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), Format::ALL.len());
    }
}
