// SPDX-License-Identifier: MPL-2.0

//! A message's Markdown as the app draws it: code blocks, and runs of
//! text with their styles and links. The parsing is the terminal's
//! (`mothrix_core::inline`, `mothrix_core::codeblock`), so both show the same.

use mothrix_core::{
    codeblock::{self, Segment},
    inline,
};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text { runs: Vec<Run> },
    Code { language: String, content: String },
}

/// Text with one set of styles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Run {
    pub text: String,
    /// Styles: "bold", "italic", "code", "strike", "heading", "quote".
    pub styles: Vec<&'static str>,
    /// Where a link leads.
    pub link: Option<String>,
}

const STYLES: [(u8, &str); 6] = [
    (inline::BOLD, "bold"),
    (inline::ITALIC, "italic"),
    (inline::CODE, "code"),
    (inline::STRIKE, "strike"),
    (inline::HEADING, "heading"),
    (inline::QUOTE, "quote"),
];

pub fn blocks(body: &str) -> Vec<Block> {
    codeblock::segments(body)
        .into_iter()
        .map(|segment| match segment {
            Segment::Text(text) => Block::Text { runs: runs(&text) },
            // The app copies a block as it is given: without what a
            // terminal or editor it is pasted into would obey.
            Segment::Code { language, content } => Block::Code {
                language,
                content: mothrix_core::message::copyable(&content),
            },
        })
        .collect()
}

fn runs(text: &str) -> Vec<Run> {
    let (chars, links) = inline::parse_links(text);
    let mut out: Vec<Run> = Vec::new();
    // The link the text is in: its place among `links`, and which of the
    // two kinds of link it is (a neighbour is of the other).
    let mut in_link: Option<(usize, bool)> = None;
    let mut next_link = 0usize;
    let mut current: Option<(u8, Option<usize>, String)> = None;
    let finish = |flags: u8, link: Option<usize>, text: String, out: &mut Vec<Run>| {
        out.push(Run {
            text,
            styles: STYLES
                .iter()
                .filter(|(flag, _)| flags & flag != 0)
                .map(|(_, name)| *name)
                .collect(),
            // A link's text can be several runs (part of it bold): each
            // leads where the link does.
            link: link.and_then(|link| links.get(link).cloned()),
        });
    };
    for (c, flags) in chars {
        match inline::link_of(flags) {
            Some(kind) if in_link.is_none_or(|(_, last)| last != kind) => {
                in_link = Some((next_link, kind));
                next_link += 1;
            }
            Some(_) => {}
            None => in_link = None,
        }
        let link = in_link.map(|(link, _)| link);
        match &mut current {
            Some((same, at, text)) if *same == flags && *at == link => text.push(c),
            _ => {
                if let Some((flags, link, text)) = current.take() {
                    finish(flags, link, text, &mut out);
                }
                current = Some((flags, link, c.to_string()));
            }
        }
    }
    if let Some((flags, link, text)) = current {
        finish(flags, link, text, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, styles: &[&'static str], link: Option<&str>) -> Run {
        Run {
            text: text.into(),
            styles: styles.to_vec(),
            link: link.map(Into::into),
        }
    }

    #[test]
    fn styles_and_links_become_runs() {
        assert_eq!(
            blocks("a **b** `c` see https://x.example ok"),
            [Block::Text {
                runs: vec![
                    run("a ", &[], None),
                    run("b", &["bold"], None),
                    run(" ", &[], None),
                    run("c", &["code"], None),
                    run(" see ", &[], None),
                    run("https://x.example", &[], Some("https://x.example")),
                    run(" ok", &[], None),
                ]
            }]
        );
        // What is not Markdown stays as typed.
        assert_eq!(
            blocks("snake_case and 2 * 3"),
            [Block::Text {
                runs: vec![run("snake_case and 2 * 3", &[], None)]
            }]
        );
    }

    #[test]
    fn code_blocks_stand_apart() {
        assert_eq!(
            blocks("look:\n```rust\nfn main() {}\n```\ndone"),
            [
                Block::Text {
                    runs: vec![run("look:", &[], None)]
                },
                Block::Code {
                    language: "rust".into(),
                    content: "fn main() {}".into()
                },
                Block::Text {
                    runs: vec![run("done", &[], None)]
                },
            ]
        );
    }
}
