// SPDX-License-Identifier: MPL-2.0

//! The inline part of Markdown, for showing messages: `code`, **bold**,
//! *italic*, ~~struck~~. The markers are removed and the text they
//! enclose is tagged, then wrapped to the width of the conversation.

use unicode_width::UnicodeWidthChar;

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
pub const CODE: u8 = 4;
pub const STRIKE: u8 = 8;
/// A heading line (`# Title`).
pub const HEADING: u8 = 16;
/// A quoted line (`> text`).
pub const QUOTE: u8 = 32;
/// A web address, or the text of a `[text](address)` link.
pub const LINK: u8 = 64;
/// With `LINK`, on every other link: two links that stand side by side
/// can be told apart ([`link_of`]).
pub const LINK_ALT: u8 = 128;

/// Which link a character belongs to, as far as its flags say: none, or
/// one of two kinds that take turns. A new link starts where this changes
/// to another `Some`.
pub fn link_of(flags: u8) -> Option<bool> {
    (flags & LINK != 0).then_some(flags & LINK_ALT != 0)
}

/// The site an address leads to (`example.org` of
/// `https://user@example.org:8443/path`), for showing beside a link whose
/// text does not say; for `mailto:` whom the mail goes to. None for what
/// mothrix neither shows as a link nor opens: another kind of address,
/// and one a browser may read differently from how it looks.
///
/// The address is read the way browsers read it (`url`), not by hand:
/// they skip extra slashes after `https:`, and take `\` for `/`, so that
/// `https://evil.example\@bank.example/` leads to evil.example. An
/// address with a `\` is refused altogether.
pub fn site(address: &str) -> Option<String> {
    let plain = !address
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\\'));
    let known = ["https://", "http://", "mailto:"]
        .iter()
        .any(|scheme| address.starts_with(scheme));
    if !plain || !known {
        return None;
    }
    let url = url::Url::parse(address).ok()?;
    let site = match url.scheme() {
        "mailto" => url.path(),
        _ => url.host_str()?,
    };
    (!site.is_empty()).then(|| site.to_owned())
}

/// Whether a link's text already names the site it leads to: it is that
/// address, with or without `https://`.
fn names_site(text: &str, site: &str) -> bool {
    let text = text.trim().to_lowercase();
    let text = text
        .split_once("://")
        .map_or(text.as_str(), |(_, rest)| rest);
    let text = text.trim_start_matches("mailto:");
    text.strip_prefix(&site.to_lowercase())
        .is_some_and(|after| after.is_empty() || after.starts_with(['/', ':', '?', '#']))
}

/// A character and the styles (`BOLD` etc.) that apply to it.
pub type Styled = (char, u8);

/// The cells a character takes on the screen, as the terminal library
/// counts them. A control character takes none: it is not drawn.
///
/// U+FE0F ("show as emoji") takes one: it makes the narrow sign before it
/// (❤, ☺, ✔, ⚠) a wide one, which is how the library counts them
/// together. After a sign that is wide already this counts one too many,
/// which only wraps a line a little early.
pub fn cells(c: char) -> usize {
    if c == '\u{fe0f}' {
        1
    } else {
        c.width().unwrap_or(0)
    }
}

/// Tag the characters of `text`, dropping the Markdown markers that were
/// understood. Anything unclear (a lone `*`, snake_case) stays as typed.
#[cfg(test)]
pub fn parse(text: &str) -> Vec<Styled> {
    parse_links(text).0
}

/// Like [`parse`], and the addresses of the links in the order their
/// (`LINK`-tagged) text appears.
pub fn parse_links(text: &str) -> (Vec<Styled>, Vec<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut links = Vec::new();
    walk(&chars, 0, &mut out, &mut links);
    (blocks(&out), links)
}

/// Addresses mothrix hands to the browser or mail program.
fn opens(address: &[char]) -> bool {
    site(&address.iter().collect::<String>()).is_some()
}

/// The web address that starts at `at`, as far as it goes: up to white
/// space, without the punctuation of the sentence around it.
fn address_at(chars: &[char], at: usize) -> Option<usize> {
    let web = starts_with(chars, at, &['h', 't', 't', 'p', ':', '/', '/'])
        || starts_with(chars, at, &['h', 't', 't', 'p', 's', ':', '/', '/']);
    if !web || (at > 0 && chars[at - 1].is_alphanumeric()) {
        return None;
    }
    let mut end = at;
    while end < chars.len() && !chars[end].is_whitespace() && !matches!(chars[end], '<' | '>' | '"')
    {
        end += 1;
    }
    while end > at {
        let last = chars[end - 1];
        let unbalanced = last == ')' && !chars[at..end - 1].contains(&'(');
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '_') || unbalanced {
            end -= 1;
        } else {
            break;
        }
    }
    // More than just "https://".
    ((chars[at..end].iter().filter(|c| **c == '/').count() >= 2 && chars[end - 1] != '/'
        || end - at > 8)
        && opens(&chars[at..end]))
    .then_some(end)
}

/// How long a link's text and its address may be, in characters.
const MAX_LINK_TEXT: usize = 400;
const MAX_ADDRESS: usize = 2000;
const MAX_SPAN: usize = 5000;

/// A `[text](address)` link opening at `at`: the ranges of text and
/// address, and where the text goes on after it.
fn link_at(chars: &[char], at: usize) -> Option<(usize, usize, usize, usize, usize)> {
    if chars[at] != '[' {
        return None;
    }
    // No further than a link's text ever is: a message of nothing but
    // "[" would else be read to its end from each of them.
    let text_end = (at + 1..chars.len().min(at + MAX_LINK_TEXT))
        .find(|i| chars[*i] == ']' || chars[*i] == '\n')?;
    if chars[text_end] != ']' || text_end == at + 1 || chars.get(text_end + 1) != Some(&'(') {
        return None;
    }
    let address = text_end + 2;
    let address_end = (address..chars.len().min(address + MAX_ADDRESS))
        .find(|i| chars[*i] == ')' || chars[*i].is_whitespace())?;
    (chars[address_end] == ')' && opens(&chars[address..address_end])).then_some((
        at + 1,
        text_end,
        address,
        address_end,
        address_end + 1,
    ))
}

/// The line-level part: headings, quotes and list bullets.
fn blocks(chars: &[Styled]) -> Vec<Styled> {
    let mut out = Vec::with_capacity(chars.len());
    for (number, line) in chars.split(|(c, _)| *c == '\n').enumerate() {
        if number > 0 {
            out.push(('\n', 0));
        }
        // Markers count only as typed, not inside code or other spans.
        let plain = |at: usize| {
            line.get(at)
                .filter(|(_, flags)| *flags == 0)
                .map(|(c, _)| *c)
        };
        let hashes = (0..line.len())
            .take_while(|at| plain(*at) == Some('#'))
            .count();
        if (1..=6).contains(&hashes) && plain(hashes) == Some(' ') && line.len() > hashes + 1 {
            out.extend(
                line[hashes + 1..]
                    .iter()
                    .map(|(c, flags)| (*c, flags | HEADING)),
            );
            continue;
        }
        if plain(0) == Some('>') {
            let text = if plain(1) == Some(' ') { 2 } else { 1 };
            out.extend([('▎', QUOTE), (' ', QUOTE)]);
            out.extend(line[text..].iter().map(|(c, flags)| (*c, flags | QUOTE)));
            continue;
        }
        let indent = line.iter().take_while(|(c, _)| *c == ' ').count();
        // "1. " or "2) ": the number of a list stands out.
        let digits = (indent..line.len())
            .take_while(|at| plain(*at).is_some_and(|c| c.is_ascii_digit()))
            .count();
        let after = indent + digits;
        if (1..=3).contains(&digits)
            && matches!(plain(after), Some('.' | ')'))
            && plain(after + 1) == Some(' ')
            && line.len() > after + 2
        {
            out.extend_from_slice(&line[..indent]);
            out.extend(
                line[indent..=after]
                    .iter()
                    .map(|(c, flags)| (*c, flags | BOLD)),
            );
            out.extend_from_slice(&line[after + 1..]);
            continue;
        }
        if matches!(plain(indent), Some('-' | '*' | '+'))
            && plain(indent + 1) == Some(' ')
            && line.len() > indent + 2
        {
            out.extend_from_slice(&line[..indent]);
            out.push(('•', 0));
            out.extend_from_slice(&line[indent + 1..]);
            continue;
        }
        out.extend_from_slice(line);
    }
    out
}

fn starts_with(chars: &[char], at: usize, marker: &[char]) -> bool {
    chars.len() >= at + marker.len() && chars[at..at + marker.len()] == *marker
}

/// A span opened by `marker` at `at`: the range of its inside and where
/// the text goes on after it. `whole_words` is for `_`, which only counts
/// around words (so that snake_case is left alone).
fn span(
    chars: &[char],
    at: usize,
    marker: &[char],
    whole_words: bool,
) -> Option<(usize, usize, usize)> {
    let inside = at + marker.len();
    if !starts_with(chars, at, marker) || chars.get(inside).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    if whole_words && at > 0 && chars[at - 1].is_alphanumeric() {
        return None;
    }
    let mut close = inside + 1;
    // Nor further than a marked stretch ever is (see `MAX_LINK_TEXT`).
    while close + marker.len() <= chars.len() && close < inside + MAX_SPAN {
        // A span does not run across a blank line.
        if chars[close] == '\n' && chars.get(close + 1) == Some(&'\n') {
            return None;
        }
        let after = close + marker.len();
        if starts_with(chars, close, marker)
            && !chars[close - 1].is_whitespace()
            && !(whole_words && chars.get(after).is_some_and(|c| c.is_alphanumeric()))
            // "*a**" closes at the last star, not the first.
            && !(marker.len() == 1 && chars.get(after) == Some(&marker[0]))
        {
            return Some((inside, close, after));
        }
        close += 1;
    }
    None
}

fn walk(chars: &[char], flags: u8, out: &mut Vec<Styled>, links: &mut Vec<String>) {
    const SPANS: [(&[char], u8, bool); 5] = [
        (&['*', '*'], BOLD, false),
        (&['_', '_'], BOLD, true),
        (&['~', '~'], STRIKE, false),
        (&['*'], ITALIC, false),
        (&['_'], ITALIC, true),
    ];
    let mut i = 0;
    'text: while i < chars.len() {
        let c = chars[i];
        // "\*" is a plain star.
        if c == '\\'
            && chars
                .get(i + 1)
                .is_some_and(|next| next.is_ascii_punctuation())
        {
            out.push((chars[i + 1], flags));
            i += 2;
            continue;
        }
        if c == '`' {
            let ticks = chars[i..].iter().take_while(|c| **c == '`').count();
            let marker = vec!['`'; ticks];
            let inside = i + ticks;
            let close = (inside + 1..chars.len()).find(|at| starts_with(chars, *at, &marker));
            if let Some(close) = close {
                // Code is shown as typed: nothing inside it is Markdown.
                out.extend(
                    chars[inside..close]
                        .iter()
                        .map(|c| (if *c == '\n' { ' ' } else { *c }, flags | CODE)),
                );
                i = close + ticks;
                continue;
            }
        }
        // Addresses are kept as typed (their underscores are not italics).
        if flags & LINK == 0 {
            // Every other link is marked, to keep neighbours apart.
            let link = flags | LINK | if links.len() % 2 == 1 { LINK_ALT } else { 0 };
            if let Some(end) = address_at(chars, i) {
                out.extend(chars[i..end].iter().map(|c| (*c, link)));
                links.push(chars[i..end].iter().collect());
                i = end;
                continue;
            }
            if let Some((text, text_end, address, address_end, after)) = link_at(chars, i) {
                let before = out.len();
                walk(&chars[text..text_end], link, out, &mut Vec::new());
                if out.len() > before {
                    let address: String = chars[address..address_end].iter().collect();
                    // Where it leads, unless its text says so: a link
                    // may be called anything, another site's address too.
                    let shown: String = out[before..].iter().map(|(c, _)| *c).collect();
                    // `link_at` took only an address that has one.
                    let site = site(&address).unwrap_or_default();
                    if !site.is_empty() && !names_site(&shown, &site) {
                        out.extend(format!(" ({site})").chars().map(|c| (c, flags)));
                    }
                    links.push(address);
                }
                i = after;
                continue;
            }
        }
        for (marker, flag, whole_words) in SPANS {
            // A single marker never opens where a double one stands.
            if marker.len() == 1 && chars.get(i + 1) == Some(&marker[0]) {
                continue;
            }
            if let Some((inside, close, after)) = span(chars, i, marker, whole_words) {
                walk(&chars[inside..close], flags | flag, out, links);
                i = after;
                continue 'text;
            }
        }
        out.push((c, flags));
        i += 1;
    }
}

/// Word-wrap tagged text to `width` cells, keeping its own line breaks and
/// the indentation at the start of a line.
pub fn wrap(chars: &[Styled], width: usize) -> Vec<Vec<Styled>> {
    let width = width.max(4);
    let cells = |c: &Styled| cells(c.0);
    let mut lines = Vec::new();
    for paragraph in chars.split(|(c, _)| *c == '\n') {
        let mut line: Vec<Styled> = Vec::new();
        let mut used = 0;
        let mut i = 0;
        while i < paragraph.len() {
            let spaces_from = i;
            while i < paragraph.len() && paragraph[i].0 == ' ' {
                i += 1;
            }
            let word_from = i;
            while i < paragraph.len() && paragraph[i].0 != ' ' {
                i += 1;
            }
            let spaces = &paragraph[spaces_from..word_from];
            let word = &paragraph[word_from..i];
            let word_cells: usize = word.iter().map(cells).sum();
            // Spaces are dropped where a line was broken.
            let lead = if line.is_empty() && spaces_from != 0 {
                0
            } else {
                spaces.len()
            };
            if used + lead + word_cells <= width {
                line.extend_from_slice(&spaces[..lead]);
                line.extend_from_slice(word);
                used += lead + word_cells;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            // A word longer than a line is cut.
            for styled in word {
                if used + cells(styled) > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push(*styled);
                used += cells(styled);
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text with each styled stretch in brackets tagged by its flags.
    fn shown(text: &str) -> String {
        let mut out = String::new();
        let mut current = 0;
        for (c, flags) in parse(text) {
            if flags != current {
                if current != 0 {
                    out.push(']');
                }
                if flags != 0 {
                    out.push_str(&format!("[{flags}:"));
                }
                current = flags;
            }
            out.push(c);
        }
        if current != 0 {
            out.push(']');
        }
        out
    }

    #[test]
    fn code_bold_italic_and_strike_are_tagged_and_markers_removed() {
        assert_eq!(shown("run `ls -la` now"), "run [4:ls -la] now");
        assert_eq!(
            shown("**bold** and *it* and _it_"),
            "[1:bold] and [2:it] and [2:it]"
        );
        assert_eq!(shown("~~old~~ new"), "[8:old] new");
        assert_eq!(shown("**bold with `code`**"), "[1:bold with ][5:code]");
        assert_eq!(shown("```let x = 1;```"), "[4:let x = 1;]");
    }

    #[test]
    fn things_that_only_look_like_markdown_stay_as_typed() {
        for text in [
            "2 * 3 * 4",
            "snake_case_name and other_name",
            "a lone ` tick",
            "5 * 3 = 15*",
            "unclosed **bold",
        ] {
            assert_eq!(shown(text), text);
        }
        assert_eq!(shown(r"\*not italic\*"), "*not italic*");
        assert_eq!(shown("`a*b*c`"), "[4:a*b*c]");
    }

    #[test]
    fn headings_quotes_and_bullets_are_recognised_per_line() {
        assert_eq!(shown("### Plan\ntext"), "[16:Plan]\ntext");
        assert_eq!(shown("# **Big** one"), "[17:Big][16: one]");
        assert_eq!(shown("> said\nreply"), "[32:▎ said]\nreply");
        assert_eq!(shown("- one\n  * two"), "• one\n  • two");
        // Not headings or bullets:
        for text in [
            "#hashtag",
            "####### seven",
            "a # b",
            "-5 degrees",
            "`# code`",
        ] {
            let plain: String = parse(text).into_iter().map(|(c, _)| c).collect();
            assert!(!plain.contains('•'));
            assert!(
                parse(text).iter().all(|(_, flags)| flags & HEADING == 0),
                "{text}"
            );
        }
    }

    fn lines(text: &str, width: usize) -> Vec<String> {
        wrap(&parse(text), width)
            .into_iter()
            .map(|line| line.into_iter().map(|(c, _)| c).collect())
            .collect()
    }

    #[test]
    fn wrapping_breaks_at_words_and_keeps_line_breaks_and_indentation() {
        assert_eq!(lines("one two three four", 9), ["one two", "three", "four"]);
        assert_eq!(lines("a\n\n  b", 10), ["a", "", "  b"]);
        assert_eq!(lines("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert_eq!(lines("**bold words** here", 10), ["bold words", "here"]);
    }

    #[test]
    fn web_addresses_and_named_links_are_tagged_and_collected() {
        let text = |styled: &[Styled], flag: u8| -> String {
            styled
                .iter()
                .filter(|(_, f)| f & flag != 0)
                .map(|(c, _)| *c)
                .collect()
        };
        let (styled, links) = parse_links(
            "see https://example.org/a_b_c. and [the *docs*](https://docs.example.org/x), ok",
        );
        assert_eq!(
            links,
            ["https://example.org/a_b_c", "https://docs.example.org/x"]
        );
        assert_eq!(text(&styled, LINK), "https://example.org/a_b_cthe docs");
        // Of the named link's address the site is shown, after its text
        // (which keeps its styles).
        let shown: String = styled.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            shown,
            "see https://example.org/a_b_c. and the docs (docs.example.org), ok"
        );
        assert_eq!(text(&styled, ITALIC), "docs");
        // Two links side by side are two.
        assert_eq!(text(&styled, LINK_ALT), "the docs");

        // A link called by another site's address says where it leads; one
        // called by its own does not say it twice.
        let shown =
            |text: &str| -> String { parse_links(text).0.iter().map(|(c, _)| *c).collect() };
        assert_eq!(
            shown("[https://bank.example/login](https://user@evil.example:8443/login)"),
            "https://bank.example/login (evil.example)"
        );
        assert_eq!(
            shown("[bank.example/login](https://bank.example/x)"),
            "bank.example/login"
        );
        assert_eq!(
            shown("[write](mailto:amy@example.org?subject=x)"),
            "write (amy@example.org)"
        );

        // The site is the one a browser goes to: it skips extra slashes,
        // and what stands before an `@` is a user name.
        assert_eq!(
            shown("[https://bank.example/login](https:///evil.example/login)"),
            "https://bank.example/login (evil.example)"
        );
        assert_eq!(shown("[x](https://[::1]:8080/)"), "x ([::1])");
        // An address with a backslash is no link at all: browsers read
        // `\` as `/`, so this one leads to evil.example.
        for text in [
            "[bank.example](https://evil.example\\@bank.example/login)",
            "[pay](https://evil.example\\@bank.example/login)",
            "https://evil.example\\@bank.example/login",
            "[x](https://)",
            "[x](mailto:?subject=x)",
        ] {
            let (styled, links) = parse_links(text);
            assert!(links.is_empty(), "{text}");
            assert!(styled.iter().all(|(_, flags)| flags & LINK == 0), "{text}");
        }

        // A sentence's own brackets stay out; an address's own stay in.
        let (_, links) =
            parse_links("(https://a.example/x) https://en.wikipedia.org/wiki/Moth_(insect)");
        assert_eq!(
            links,
            [
                "https://a.example/x",
                "https://en.wikipedia.org/wiki/Moth_(insect)"
            ]
        );

        // Not links: other schemes, and code.
        let (styled, links) = parse_links("[x](javascript:alert) `https://in.code/x`");
        assert!(links.is_empty());
        assert_eq!(text(&styled, LINK), "");
    }

    #[test]
    fn a_message_of_nothing_but_marks_is_read_quickly() {
        let started = std::time::Instant::now();
        for marks in ["[", "*a ", "`x "] {
            let hostile = marks.repeat(60_000 / marks.len());
            assert!(!parse_links(&hostile).0.is_empty());
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn signs_shown_as_emoji_are_counted_as_wide_as_the_screen_draws_them() {
        use unicode_width::UnicodeWidthStr;
        for sign in ["❤️", "☺️", "✔️", "⚠️", "1️⃣", "ä", "漢", "👍"] {
            let counted: usize = sign.chars().map(cells).sum();
            assert_eq!(counted, sign.width(), "{sign}");
        }
        // A row of them wraps where the screen ends, not at twice that.
        let hearts = "❤️".repeat(30);
        let rows = wrap(&parse(&hearts), 20);
        assert!(
            rows.iter()
                .all(|row| row.iter().map(|(c, _)| cells(*c)).sum::<usize>() <= 20)
        );
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn list_numbers_stand_out() {
        let styled = parse("1. first\n 12) second\n3.14 is not a list");
        let bold: String = styled
            .iter()
            .filter(|(_, f)| f & BOLD != 0)
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(bold, "1.12)");
    }
}
