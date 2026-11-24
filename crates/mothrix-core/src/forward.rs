// SPDX-License-Identifier: MPL-2.0

//! Forwarding: what goes into the copy of a message (who wrote it), and
//! taking its file along to the homeserver of the account that sends
//! the copy, when that is another one. The message travels as the JSON
//! of its type (`{"msgtype": "m.image", …}`).

use anyhow::{Context, Result};
use matrix_sdk::{
    Client,
    media::{MediaFormat, MediaRequestParameters},
    ruma::{OwnedMxcUri, events::room::MediaSource},
};
use serde_json::Value;

/// The copy of a message to forward says who wrote it: a text is quoted
/// under "Forwarded from Name:", a file gets that line as what is
/// written with it (before what was written with it already).
pub fn attributed(mut content: Value, from: &str) -> Value {
    let label = format!("Forwarded from {from}");
    let text = |key: &str| content[key].as_str().map(str::to_owned);
    let body = text("body").unwrap_or_default();
    let formatted = text("formatted_body")
        .filter(|_| content["format"].as_str() == Some("org.matrix.custom.html"));
    let label_html = format!("Forwarded from <b>{}</b>", escape(from));
    match content["msgtype"].as_str().unwrap_or_default() {
        kind @ ("m.text" | "m.notice" | "m.emote") => {
            // "* Name did something", as an emote reads.
            let (body, formatted) = if kind == "m.emote" {
                (
                    format!("* {from} {body}"),
                    formatted.map(|html| format!("* {} {html}", escape(from))),
                )
            } else {
                (body, formatted)
            };
            let quoted: Vec<String> = body.lines().map(|line| format!("> {line}")).collect();
            let html = formatted.unwrap_or_else(|| escape(&body).replace('\n', "<br>"));
            content["msgtype"] = "m.text".into();
            content["body"] = format!("{label}:\n{}", quoted.join("\n")).into();
            content["format"] = "org.matrix.custom.html".into();
            content["formatted_body"] =
                format!("<p>{label_html}:</p><blockquote>{html}</blockquote>").into();
        }
        "m.image" | "m.file" | "m.audio" | "m.video" => {
            // Without a file name of its own, the body is the name: then
            // nothing was written with the file.
            let captioned = text("filename").is_some_and(|name| name != body);
            if captioned {
                content["body"] = format!("{label}:\n{body}").into();
                if let Some(html) = formatted {
                    content["formatted_body"] = format!("<p>{label_html}:</p>{html}").into();
                }
            } else {
                content["filename"] = body.into();
                content["body"] = label.into();
            }
        }
        // A place, a verification request: as it is.
        _ => {}
    }
    content
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The files a message points to (the file itself, its small picture),
/// as (address, type). An encrypted file is carried as it is stored:
/// bytes of no type, its key stays in the message.
pub fn files(content: &Value) -> Vec<(String, String)> {
    let plain = |value: &Value, info: &Value| {
        let mime = info["mimetype"].as_str().unwrap_or(OCTETS).to_owned();
        value.as_str().map(|uri| (uri.to_owned(), mime))
    };
    let encrypted = |file: &Value| {
        file["url"]
            .as_str()
            .map(|uri| (uri.to_owned(), OCTETS.to_owned()))
    };
    let info = &content["info"];
    let mut found: Vec<(String, String)> = [
        plain(&content["url"], info),
        encrypted(&content["file"]),
        plain(&info["thumbnail_url"], &info["thumbnail_info"]),
        encrypted(&info["thumbnail_file"]),
    ]
    .into_iter()
    .flatten()
    .filter(|(uri, _)| uri.starts_with("mxc://"))
    .collect();
    found.dedup();
    found
}

const OCTETS: &str = "application/octet-stream";

/// The homeserver a file's address names.
pub fn server_of(uri: &str) -> &str {
    let rest = uri.strip_prefix("mxc://").unwrap_or_default();
    rest.split('/').next().unwrap_or_default()
}

/// The message with a file's address changed for another.
pub fn moved(content: &str, from: &str, to: &str) -> String {
    // As JSON strings, which is how they stand in the text.
    let quote = |uri: &str| Value::from(uri).to_string();
    content.replace(&quote(from), &quote(to))
}

/// Take the files of a message (the JSON of its type) along to the
/// homeserver of `to`: read through `from`, the account that showed the
/// message, and uploaded there, since the other server may not be able
/// to fetch them (servers that do not federate). Files that are on that
/// homeserver already stay where they are.
pub async fn carry(from: &Client, to: &Client, content: String) -> Result<String> {
    let Some(home) = to.user_id().map(|user| user.server_name().to_string()) else {
        return Ok(content);
    };
    let parsed: Value = serde_json::from_str(&content)?;
    let mut content = content;
    for (uri, mime) in files(&parsed) {
        if server_of(&uri) == home {
            continue;
        }
        let bytes = from
            .media()
            .get_media_content(
                &MediaRequestParameters {
                    source: MediaSource::Plain(OwnedMxcUri::from(uri.as_str())),
                    format: MediaFormat::File,
                },
                true,
            )
            .await
            .context("could not read the file to forward")?;
        let mime: mime::Mime = mime.parse().unwrap_or(mime::APPLICATION_OCTET_STREAM);
        let uploaded = to
            .media()
            .upload(&mime, bytes, None)
            .await
            .context("could not upload the file to the other account's server")?;
        content = moved(&content, &uri, uploaded.content_uri.as_str());
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_forwarded_text_is_quoted_under_its_writer() {
        let copy = attributed(
            json!({"msgtype": "m.text", "body": "one\ntwo <3"}),
            "Amy & Co",
        );
        assert_eq!(copy["body"], "Forwarded from Amy & Co:\n> one\n> two <3");
        assert_eq!(
            copy["formatted_body"],
            "<p>Forwarded from <b>Amy &amp; Co</b>:</p><blockquote>one<br>two &lt;3</blockquote>"
        );
        assert_eq!(copy["format"], "org.matrix.custom.html");

        // What was formatted stays formatted.
        let copy = attributed(
            json!({"msgtype": "m.notice", "body": "*hi*", "format": "org.matrix.custom.html",
                   "formatted_body": "<em>hi</em>"}),
            "Amy",
        );
        assert_eq!(copy["msgtype"], "m.text");
        assert_eq!(
            copy["formatted_body"],
            "<p>Forwarded from <b>Amy</b>:</p><blockquote><em>hi</em></blockquote>"
        );

        let copy = attributed(json!({"msgtype": "m.emote", "body": "waves"}), "Amy");
        assert_eq!(copy["body"], "Forwarded from Amy:\n> * Amy waves");
    }

    #[test]
    fn a_forwarded_file_says_who_sent_it_and_keeps_its_name() {
        let copy = attributed(
            json!({"msgtype": "m.image", "body": "moth.png", "url": "mxc://a/1"}),
            "Amy",
        );
        assert_eq!(copy["body"], "Forwarded from Amy");
        assert_eq!(copy["filename"], "moth.png");
        assert_eq!(copy["url"], "mxc://a/1");

        // Something was written with it: that stays, after the line.
        let copy = attributed(
            json!({"msgtype": "m.file", "body": "Look!", "filename": "moth.pdf"}),
            "Amy",
        );
        assert_eq!(copy["body"], "Forwarded from Amy:\nLook!");
        assert_eq!(copy["filename"], "moth.pdf");

        let place = json!({"msgtype": "m.location", "body": "here", "geo_uri": "geo:1,2"});
        assert_eq!(attributed(place.clone(), "Amy"), place);
    }

    #[test]
    fn a_message_s_files_are_found_with_their_types() {
        let plain = json!({
            "msgtype": "m.image", "body": "moth.png", "url": "mxc://a.example/1",
            "info": {"mimetype": "image/png", "thumbnail_url": "mxc://a.example/2",
                     "thumbnail_info": {"mimetype": "image/jpeg"}},
        });
        assert_eq!(
            files(&plain),
            [
                ("mxc://a.example/1".to_owned(), "image/png".to_owned()),
                ("mxc://a.example/2".to_owned(), "image/jpeg".to_owned()),
            ]
        );
        let encrypted = json!({
            "msgtype": "m.file", "body": "a.pdf",
            "file": {"url": "mxc://a.example/3", "key": {}},
            "info": {"mimetype": "application/pdf",
                     "thumbnail_file": {"url": "mxc://a.example/4"}},
        });
        assert_eq!(
            files(&encrypted),
            [
                ("mxc://a.example/3".to_owned(), OCTETS.to_owned()),
                ("mxc://a.example/4".to_owned(), OCTETS.to_owned()),
            ]
        );
        assert!(files(&json!({"msgtype": "m.text", "body": "mxc://a.example/5"})).is_empty());
        assert_eq!(server_of("mxc://a.example:8448/1"), "a.example:8448");
        assert_eq!(server_of("https://a.example/1"), "");
    }

    #[test]
    fn a_file_s_address_is_changed_wherever_it_stands() {
        let content = json!({"url": "mxc://a/1", "body": "see mxc://a/1",
                             "info": {"thumbnail_url": "mxc://a/1"}})
        .to_string();
        let changed: Value =
            serde_json::from_str(&moved(&content, "mxc://a/1", "mxc://b/9")).unwrap();
        assert_eq!(changed["url"], "mxc://b/9");
        assert_eq!(changed["info"]["thumbnail_url"], "mxc://b/9");
        // Only where it is the whole value, not inside a text.
        assert_eq!(changed["body"], "see mxc://a/1");
    }
}
