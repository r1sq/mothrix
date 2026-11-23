// SPDX-License-Identifier: MPL-2.0

//! Optional grammar and spelling checks by a LanguageTool server
//! (<https://languagetool.org/http-api/>). Nothing is sent anywhere unless
//! the user sets a server address.

use std::time::Duration;

use matrix_sdk::reqwest;
use serde_json::Value;

use crate::model::GrammarMatch;

/// The check address for what the user entered: the server's base address,
/// its `/v2` address, or the full `/v2/check` address all work.
pub fn endpoint(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = if url.contains("://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    };
    if url.ends_with("/v2/check") {
        url
    } else if url.ends_with("/v2") {
        format!("{url}/check")
    } else {
        format!("{url}/v2/check")
    }
}

/// Dictionary names like `de_DE` as the variants LanguageTool should assume
/// when it detects the language itself. Only languages that have variants.
pub fn preferred_variants(languages: &[String]) -> String {
    const KNOWN: [&str; 11] = [
        "en-US", "en-GB", "en-AU", "en-CA", "en-NZ", "en-ZA", "de-DE", "de-AT", "de-CH", "pt-PT",
        "pt-BR",
    ];
    let mut variants: Vec<String> = Vec::new();
    for language in languages {
        let variant = language.replace('_', "-");
        let family = variant.split('-').next().unwrap_or_default().to_owned();
        // One variant per language: the first one named wins.
        if KNOWN.contains(&variant.as_str())
            && !variants
                .iter()
                .any(|known| known.starts_with(&format!("{family}-")))
        {
            variants.push(variant);
        }
    }
    variants.join(",")
}

/// Language codes as typed ("de, en_gb") in the form LanguageTool wants
/// ("de-DE", "en-GB"). German, English and Portuguese need a region to be
/// spell-checked: it comes from `variants` (see [`preferred_variants`]) or
/// a default.
pub fn parse_languages(typed: &str, variants: &str) -> Vec<String> {
    let mut languages: Vec<String> = Vec::new();
    for token in typed.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let mut parts = token.split(['-', '_']).filter(|part| !part.is_empty());
        let Some(language) = parts.next().map(str::to_lowercase) else {
            continue;
        };
        if !language.chars().all(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        let code = match parts.next() {
            Some(region) => format!("{language}-{}", region.to_uppercase()),
            None => {
                let preferred = variants
                    .split(',')
                    .find(|variant| variant.starts_with(&format!("{language}-")));
                match (preferred, language.as_str()) {
                    (Some(variant), _) => variant.to_owned(),
                    (None, "de") => "de-DE".to_owned(),
                    (None, "en") => "en-US".to_owned(),
                    (None, "pt") => "pt-PT".to_owned(),
                    (None, _) => language,
                }
            }
        };
        if !languages.contains(&code) {
            languages.push(code);
        }
    }
    languages
}

/// Char ranges of the sentences (and lines) of `text`.
fn sentences(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    for (at, c) in chars.iter().enumerate() {
        let ends = *c == '\n'
            || (matches!(c, '.' | '!' | '?')
                && chars.get(at + 1).is_none_or(|next| next.is_whitespace()));
        if ends {
            out.push((start, at + 1));
            start = at + 1;
        }
    }
    if start < chars.len() {
        out.push((start, chars.len()));
    }
    out
}

/// From the findings of several languages for the same text, keep for
/// each sentence those of the language that fits it best (the fewest
/// misspellings, then the fewest findings, then the earlier language). A German sentence checked as
/// English is all mistakes, so this follows the language sentence by
/// sentence.
pub fn pick_per_sentence(text: &str, results: &[Vec<GrammarMatch>]) -> Vec<GrammarMatch> {
    let mut out = Vec::new();
    for (start, end) in sentences(text) {
        let inside = |found: &&GrammarMatch| found.start >= start && found.start < end;
        // Misspellings tell the languages apart best: the wrong language
        // knows hardly any of the words. All findings break a tie.
        let best = results.iter().min_by_key(|result| {
            let spelling = result
                .iter()
                .filter(inside)
                .filter(|found| found.spelling)
                .count();
            (spelling, result.iter().filter(inside).count())
        });
        if let Some(best) = best {
            out.extend(best.iter().filter(inside).cloned());
        }
    }
    out
}

/// The problems in a server answer, with positions counted in characters
/// of `text` (the server counts in UTF-16 units).
pub fn parse(text: &str, response: &Value) -> Vec<GrammarMatch> {
    // UTF-16 offset -> char index.
    let mut char_at = Vec::with_capacity(text.len() + 1);
    for (index, c) in text.chars().enumerate() {
        for _ in 0..c.len_utf16() {
            char_at.push(index);
        }
    }
    char_at.push(text.chars().count());

    let mut matches = Vec::new();
    for found in response["matches"].as_array().into_iter().flatten() {
        let (Some(offset), Some(length)) = (found["offset"].as_u64(), found["length"].as_u64())
        else {
            continue;
        };
        let (Some(&start), Some(&end)) = (
            char_at.get(offset as usize),
            char_at.get((offset + length) as usize),
        ) else {
            continue;
        };
        if start >= end {
            continue;
        }
        let message = ["shortMessage", "message"]
            .iter()
            .filter_map(|key| found[*key].as_str())
            .find(|message| !message.trim().is_empty())
            .unwrap_or("Possible mistake")
            .to_owned();
        matches.push(GrammarMatch {
            start,
            end,
            text: text.chars().skip(start).take(end - start).collect(),
            message,
            replacements: found["replacements"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|replacement| replacement["value"].as_str().map(str::to_owned))
                .collect(),
            spelling: found["rule"]["issueType"].as_str() == Some("misspelling"),
        });
    }
    matches
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (key, value) in pairs {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(key);
        out.push('=');
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(byte as char)
                }
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
    }
    out
}

/// Where to check, and as whom.
#[derive(Clone, Debug, Default)]
pub struct Access {
    pub url: String,
    /// Account e-mail and API key of the paid service; both empty for a
    /// server that needs no login.
    pub username: String,
    pub api_key: String,
}

/// Check `text` in the given languages, or in whatever language the
/// server detects when none are given.
pub async fn check_in(
    http: &reqwest::Client,
    url: &Access,
    text: &str,
    languages: &[String],
    variants: &str,
) -> Result<Vec<GrammarMatch>, String> {
    match languages {
        [] => check(http, url, text, "auto", variants).await,
        [only] => check(http, url, text, only, variants).await,
        several => {
            let checks = several
                .iter()
                .map(|language| check(http, url, text, language, variants));
            let mut results = Vec::new();
            let mut failure = None;
            for result in futures_util::future::join_all(checks).await {
                match result {
                    Ok(found) => results.push(found),
                    Err(error) => failure = Some(error),
                }
            }
            match (results.is_empty(), failure) {
                (true, Some(error)) => Err(error),
                _ => Ok(pick_per_sentence(text, &results)),
            }
        }
    }
}

/// Ask the server at `url` to check `text` as `language` ("auto" lets it
/// detect one). The error is a short, user-facing reason.
pub async fn check(
    http: &reqwest::Client,
    access: &Access,
    text: &str,
    language: &str,
    variants: &str,
) -> Result<Vec<GrammarMatch>, String> {
    let endpoint = endpoint(&access.url);
    let mut pairs = vec![("text", text), ("language", language)];
    // Variants only apply when the server picks the language.
    if language == "auto" && !variants.is_empty() {
        pairs.push(("preferredVariants", variants));
    }
    // The paid service identifies its users this way.
    if !access.username.is_empty() && !access.api_key.is_empty() {
        pairs.push(("username", &access.username));
        pairs.push(("apiKey", &access.api_key));
    }
    let response = http
        .post(&endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(form_encode(&pairs))
        // The first check after the server starts loads the language and
        // can take a while.
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| format!("cannot reach {endpoint}"))?;
    if matches!(response.status().as_u16(), 401 | 403) {
        return Err(format!(
            "{endpoint} refused the login (check user and API key)"
        ));
    }
    if !response.status().is_success() {
        return Err(format!(
            "{endpoint} answered {}",
            response.status().as_u16()
        ));
    }
    let body = response
        .text()
        .await
        .map_err(|_| format!("no answer from {endpoint}"))?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|_| format!("{endpoint} is not a LanguageTool server"))?;
    if !value["matches"].is_array() {
        return Err(format!("{endpoint} is not a LanguageTool server"));
    }
    Ok(parse(text, &value))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn any_form_of_the_address_gives_the_check_endpoint() {
        for given in [
            "https://lt.example.org",
            "https://lt.example.org/",
            "https://lt.example.org/v2",
            " https://lt.example.org/v2/check ",
        ] {
            assert_eq!(endpoint(given), "https://lt.example.org/v2/check");
        }
        assert_eq!(endpoint("localhost:8010"), "http://localhost:8010/v2/check");
    }

    #[test]
    fn variants_come_from_the_dictionary_names() {
        let languages = [
            "de_DE".to_owned(),
            "en_GB".into(),
            "en_US".into(),
            "es_ES".into(),
        ];
        assert_eq!(preferred_variants(&languages), "de-DE,en-GB");
    }

    #[test]
    fn positions_are_converted_from_utf16_to_characters() {
        // The emoji takes two UTF-16 units but is one character.
        let text = "🎉 this are fine";
        let response = json!({"matches": [{
            "message": "Use “is” here.", "shortMessage": "",
            "offset": 8, "length": 3,
            "replacements": [{"value": "is"}],
            "rule": {"issueType": "grammar"},
        }, {
            "message": "x", "offset": 400, "length": 2, "replacements": [],
        }]});
        let matches = parse(text, &response);
        assert_eq!(matches.len(), 1);
        let found = &matches[0];
        assert_eq!(
            (found.start, found.end, found.text.as_str()),
            (7, 10, "are")
        );
        assert_eq!(found.message, "Use “is” here.");
        assert_eq!(found.replacements, ["is"]);
        assert!(!found.spelling);
    }

    #[test]
    fn typed_languages_get_their_region() {
        assert_eq!(parse_languages("de, en", "de-AT,en-GB"), ["de-AT", "en-GB"]);
        assert_eq!(
            parse_languages("DE en_gb;fr  de", ""),
            ["de-DE", "en-GB", "fr"]
        );
        assert!(parse_languages(" , 12", "").is_empty());
    }

    #[test]
    fn each_sentence_keeps_the_language_that_fits_it() {
        let text = "Das ist ein Tset. This are fine and I has a mistaek.";
        let found = |start: usize, word: &str, spelling: bool| GrammarMatch {
            start,
            end: start + word.chars().count(),
            text: word.into(),
            spelling,
            ..GrammarMatch::default()
        };
        // As a real server answers: the German check knows some English
        // words, so it has fewer findings in the English sentence, but
        // more misspellings.
        let german = vec![
            found(12, "Tset", true),
            found(23, "are", true),
            found(38, "has", true),
            found(44, "mistaek", true),
        ];
        let english = vec![
            found(0, "Das", true),
            found(4, "ist", true),
            found(8, "ein", true),
            found(12, "Tset", true),
            found(18, "This", false),
            found(23, "are", false),
            found(38, "has", false),
            found(44, "mistaek", true),
        ];
        let picked = pick_per_sentence(text, &[german, english]);
        let words: Vec<&str> = picked.iter().map(|found| found.text.as_str()).collect();
        assert_eq!(words, ["Tset", "This", "are", "has", "mistaek"]);
    }

    #[test]
    fn form_values_are_percent_encoded() {
        assert_eq!(
            form_encode(&[("text", "a b&c=ü")]),
            "text=a%20b%26c%3D%C3%BC"
        );
    }
}
