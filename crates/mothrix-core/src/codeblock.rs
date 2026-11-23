// SPDX-License-Identifier: MPL-2.0

//! Finding fenced code blocks (three backticks) in a message, so that they
//! can be shown as blocks and copied on their own.

/// A stretch of a message body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Code { language: String, content: String },
}

/// Split a message into text and code blocks, in order. A fence that is
/// never closed runs to the end of the message, as in Markdown.
pub fn segments(body: &str) -> Vec<Segment> {
    fn flush(text: &mut Vec<&str>, out: &mut Vec<Segment>) {
        let joined = text.join("\n");
        let joined = joined.trim_matches('\n');
        if !joined.trim().is_empty() {
            out.push(Segment::Text(joined.to_owned()));
        }
        text.clear();
    }

    let mut out = Vec::new();
    let mut text: Vec<&str> = Vec::new();
    let mut code: Option<(String, Vec<&str>)> = None;
    for line in body.split('\n') {
        let trimmed = line.trim();
        match code.as_mut() {
            Some((language, lines)) => {
                if trimmed.starts_with("```") {
                    out.push(Segment::Code {
                        language: std::mem::take(language),
                        content: lines.join("\n"),
                    });
                    code = None;
                } else {
                    lines.push(line);
                }
            }
            // "```code```" on one line is inline code, not a block.
            None if trimmed.starts_with("```") && !trimmed[3..].contains("```") => {
                flush(&mut text, &mut out);
                code = Some((
                    trimmed.trim_start_matches('`').trim().to_owned(),
                    Vec::new(),
                ));
            }
            None => text.push(line),
        }
    }
    if let Some((language, lines)) = code {
        out.push(Segment::Code {
            language,
            content: lines.join("\n"),
        });
    }
    flush(&mut text, &mut out);
    out
}

/// The contents of the code blocks in a message.
pub fn code_blocks(body: &str) -> Vec<String> {
    segments(body)
        .into_iter()
        .filter_map(|segment| match segment {
            Segment::Code { content, .. } => Some(content),
            Segment::Text(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_code_are_split_in_order() {
        let body =
            "Try this:\n```rust\nfn main() {\n    run();\n}\n```\nand then\n```\nls -la\n```";
        assert_eq!(
            segments(body),
            [
                Segment::Text("Try this:".into()),
                Segment::Code {
                    language: "rust".into(),
                    content: "fn main() {\n    run();\n}".into()
                },
                Segment::Text("and then".into()),
                Segment::Code {
                    language: String::new(),
                    content: "ls -la".into()
                },
            ]
        );
        assert_eq!(code_blocks(body), ["fn main() {\n    run();\n}", "ls -la"]);
    }

    #[test]
    fn plain_text_and_inline_code_have_no_blocks() {
        assert_eq!(
            segments("hello\nworld"),
            [Segment::Text("hello\nworld".into())]
        );
        assert!(code_blocks("use ```x``` here").is_empty());
        assert!(code_blocks("a `b` c").is_empty());
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        assert_eq!(code_blocks("```\none\ntwo"), ["one\ntwo"]);
    }
}
