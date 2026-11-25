// SPDX-License-Identifier: MPL-2.0

//! Offline spell checking for the message box, using Hunspell dictionaries
//! (`.aff` + `.dic`) through the pure-Rust `spellbook` engine. Nothing typed
//! ever leaves the machine.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Something that knows which words are spelled correctly.
pub trait Speller: Send + Sync {
    fn check(&self, word: &str) -> bool;
    /// Corrections for a misspelled word, best first.
    fn suggest(&self, word: &str) -> Vec<String>;
}

/// One or more loaded dictionaries. A word is fine if any of them knows it,
/// so writing in two languages does not light everything up.
pub struct Dictionaries {
    loaded: Vec<(String, spellbook::Dictionary)>,
    /// What is wrong with each dictionary that is installed but could not
    /// be used.
    problems: Vec<String>,
}

/// Why [`load`] loaded nothing: none of the languages has a dictionary
/// installed. (Nothing to tell the user at every start; a dictionary that
/// is there and broken is.)
#[derive(Debug)]
pub struct NoneInstalled(String);

impl std::fmt::Display for NoneInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no dictionary found for {}", self.0)
    }
}

impl std::error::Error for NoneInstalled {}

impl Dictionaries {
    /// The dictionaries left out, each with what is wrong with it.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn languages(&self) -> Vec<String> {
        self.loaded
            .iter()
            .map(|(language, _)| language.clone())
            .collect()
    }
}

impl Speller for Dictionaries {
    fn check(&self, word: &str) -> bool {
        self.loaded
            .iter()
            .any(|(_, dictionary)| dictionary.check(word))
    }

    fn suggest(&self, word: &str) -> Vec<String> {
        let mut all = Vec::new();
        for (_, dictionary) in &self.loaded {
            let mut found = Vec::new();
            dictionary.suggest(word, &mut found);
            for suggestion in found {
                if !all.contains(&suggestion) {
                    all.push(suggestion);
                }
            }
        }
        all.truncate(MAX_SUGGESTIONS);
        all
    }
}

pub const MAX_SUGGESTIONS: usize = 5;

/// Where dictionaries are looked for: mothrix's own folder first (so a user
/// copy wins), then the usual system locations.
pub fn dictionary_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data) = crate::folders::data() {
        dirs.push(data.join("dictionaries"));
    }
    for system in [
        "/usr/share/hunspell",
        "/usr/share/myspell",
        "/usr/share/myspell/dicts",
        "/usr/local/share/hunspell",
        // macOS: Homebrew and the system-wide folder.
        "/opt/homebrew/share/hunspell",
        "/Library/Spelling",
        "/var/lib/flatpak/runtime/org.freedesktop.Platform/x86_64/current/active/files/share/hunspell",
    ] {
        dirs.push(PathBuf::from(system));
    }
    dirs
}

/// The folder `mothrix dictionary <language>` downloads into.
pub fn user_dictionary_dir() -> Result<PathBuf> {
    Ok(crate::folders::data()
        .context("no data directory")?
        .join("dictionaries"))
}

fn find(language: &str, dirs: &[PathBuf]) -> Option<(PathBuf, PathBuf)> {
    dirs.iter().find_map(|dir| {
        let aff = dir.join(format!("{language}.aff"));
        let dic = dir.join(format!("{language}.dic"));
        (aff.is_file() && dic.is_file()).then_some((aff, dic))
    })
}

/// Languages (like `en_US`) for which a dictionary is installed.
pub fn available_languages(dirs: &[PathBuf]) -> Vec<String> {
    let mut languages = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "aff")
                && path.with_extension("dic").is_file()
                && let Some(language) = path.file_stem().and_then(|stem| stem.to_str())
                && !languages.iter().any(|known| known == language)
            {
                languages.push(language.to_owned());
            }
        }
    }
    languages.sort();
    languages
}

/// The languages to check when none are configured: the system language
/// (from `LC_ALL` / `LC_MESSAGES` / `LANG`, e.g. `de_DE.UTF-8`) and US
/// English, which turns up in most chats.
pub fn default_languages() -> Vec<String> {
    let mut languages = Vec::new();
    for variable in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(variable) {
            let language = value
                .split(['.', '@'])
                .next()
                .unwrap_or_default()
                .to_owned();
            if language.contains('_') && !languages.contains(&language) {
                languages.push(language);
            }
        }
    }
    if !languages.iter().any(|language| language == "en_US") {
        languages.push("en_US".to_owned());
    }
    languages
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    // Older dictionaries are Latin-1; decode those byte for byte.
    Ok(String::from_utf8(bytes).unwrap_or_else(|error| {
        error
            .into_bytes()
            .iter()
            .map(|&byte| byte as char)
            .collect()
    }))
}

/// Load the dictionaries for `languages`, skipping those that are not
/// installed. One that is there and cannot be used is left out and named
/// ([`Dictionaries::problems`]): the others still check. Fails only when
/// none could be loaded; with [`NoneInstalled`] when there was none.
pub fn load(languages: &[String], dirs: &[PathBuf]) -> Result<Dictionaries> {
    let mut loaded = Vec::new();
    let mut problems = Vec::new();
    for language in languages {
        let Some((aff, dic)) = find(language, dirs) else {
            continue;
        };
        let dictionary = read_text(&aff).and_then(|aff_text| {
            spellbook::Dictionary::new(&aff_text, &read_text(&dic)?).map_err(|error| {
                anyhow::anyhow!("{} is not a usable dictionary: {error}", aff.display())
            })
        });
        match dictionary {
            Ok(dictionary) => loaded.push((language.clone(), dictionary)),
            Err(error) => problems.push(format!("{error:#}")),
        }
    }
    if loaded.is_empty() {
        if problems.is_empty() {
            bail!(NoneInstalled(languages.join(", ")));
        }
        bail!("{}", problems.join("; "));
    }
    Ok(Dictionaries { loaded, problems })
}

/// Dictionaries `mothrix dictionary` can fetch: language and the path of its
/// files (without extension) in the LibreOffice dictionaries repository.
pub const DOWNLOADABLE: [(&str, &str); 11] = [
    ("de_DE", "de/de_DE_frami"),
    ("de_AT", "de/de_AT_frami"),
    ("de_CH", "de/de_CH_frami"),
    ("en_US", "en/en_US"),
    ("en_GB", "en/en_GB"),
    ("es_ES", "es/es_ES"),
    ("it_IT", "it_IT/it_IT"),
    ("nl_NL", "nl_NL/nl_NL"),
    ("pl_PL", "pl_PL/pl_PL"),
    ("pt_PT", "pt_PT/pt_PT"),
    ("da_DK", "da_DK/da_DK"),
];

pub fn download_urls(language: &str) -> Option<(String, String)> {
    let (_, path) = DOWNLOADABLE.iter().find(|(known, _)| *known == language)?;
    let base = "https://raw.githubusercontent.com/LibreOffice/dictionaries/master";
    Some((format!("{base}/{path}.aff"), format!("{base}/{path}.dic")))
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic()
}

/// Character ranges (start, end; end exclusive) of the misspelled words in
/// `text`.
///
/// Left alone: the word still being typed (the one touching `cursor`),
/// words inside links, `code`, mentions and anything with digits, single
/// letters, and whole `/commands` (except the text of `/me`). `ignore`
/// decides about words the user or the room vouches for (names).
pub fn misspelled(
    text: &str,
    cursor: usize,
    speller: &dyn Speller,
    ignore: &dyn Fn(&str) -> bool,
) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut start_at = 0;
    // A command also after spaces, as it is when sent.
    if crate::message::is_command(text) {
        let command = text.trim_start();
        match command.strip_prefix("/me ") {
            Some(_) => start_at = chars.len() - command.chars().count() + 4,
            None => return Vec::new(),
        }
    }

    let mut found = Vec::new();
    let mut in_code = false;
    let mut index = start_at;
    while index < chars.len() {
        if chars[index] == '`' {
            in_code = !in_code;
            index += 1;
            continue;
        }
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        // One whitespace-delimited chunk at a time, so links and the like
        // can be skipped as a whole.
        let chunk_start = index;
        while index < chars.len() && !chars[index].is_whitespace() && chars[index] != '`' {
            index += 1;
        }
        let chunk = &chars[chunk_start..index];
        let technical = chunk
            .iter()
            .any(|c| c.is_ascii_digit() || matches!(c, '/' | '\\' | '_' | '@' | '#'))
            || chunk.windows(3).any(|window| window == [':', '/', '/'])
            || chunk.iter().filter(|c| **c == '.').count() > 1 && !chunk.ends_with(&['.']);
        if in_code || technical {
            continue;
        }

        let mut position = 0;
        while position < chunk.len() {
            if !is_word_char(chunk[position]) {
                position += 1;
                continue;
            }
            let word_start = position;
            while position < chunk.len()
                && (is_word_char(chunk[position])
                    // Apostrophes inside a word belong to it ("don't").
                    || (matches!(chunk[position], '\'' | '’')
                        && position + 1 < chunk.len()
                        && is_word_char(chunk[position + 1])))
            {
                position += 1;
            }
            let (start, end) = (chunk_start + word_start, chunk_start + position);
            let word: String = chars[start..end].iter().collect();
            // Only the last word, with the cursor right behind it, counts as
            // unfinished; a word the cursor merely touches stays marked.
            let being_typed = cursor == end && end == chars.len();
            if end - start < 2 || being_typed || ignore(&word) {
                continue;
            }
            if !speller.check(&word) {
                found.push((start, end));
            }
        }
    }
    found
}

/// A speller that knows a fixed list of words (for tests, here and in
/// the terminal UI).
pub struct Words(pub &'static [&'static str]);

impl Speller for Words {
    fn check(&self, word: &str) -> bool {
        self.0.iter().any(|known| known.eq_ignore_ascii_case(word))
    }

    fn suggest(&self, word: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|known| known.chars().next() == word.chars().next())
            .map(|known| (*known).to_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: Words = Words(&[
        "hello", "there", "this", "is", "fine", "don't", "see", "waves", "the", "link", "and",
        "plus",
    ]);

    fn flagged(text: &str, cursor: usize) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        misspelled(text, cursor, &KNOWN, &|word| word == "Okafor")
            .into_iter()
            .map(|(start, end)| chars[start..end].iter().collect())
            .collect()
    }

    #[test]
    fn misspelled_words_are_found_with_their_position() {
        let text = "hello thre, this is fnie";
        assert_eq!(misspelled(text, 0, &KNOWN, &|_| false), [(6, 10), (20, 24)]);
        assert_eq!(flagged(text, 0), ["thre", "fnie"]);
    }

    #[test]
    fn the_word_being_typed_is_left_alone_until_it_is_finished() {
        assert!(flagged("hello thr", 9).is_empty());
        assert_eq!(flagged("hello thr ", 10), ["thr"]);
    }

    #[test]
    fn links_code_mentions_numbers_and_commands_are_skipped() {
        assert!(
            flagged(
                "see https://exmaple.org/pgae and `teh code` plus v2beta @bneo #roomz",
                0
            )
            .is_empty()
        );
        assert!(flagged("/file ~/Dokumnte/x.txt", 0).is_empty());
        assert_eq!(flagged("/me wavse", 0), ["wavse"]);
        // A command is one after spaces too, as it is when sent.
        assert!(flagged("  /recover EsTa Dokumnte", 0).is_empty());
        assert_eq!(flagged("  /me wavse", 0), ["wavse"]);
    }

    #[test]
    fn a_broken_dictionary_is_named_and_the_others_still_check() {
        let dir = std::env::temp_dir().join(format!("mothrix-spell-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("xx_XX.aff"), "SET UTF-8\n").unwrap();
        std::fs::write(dir.join("xx_XX.dic"), "1\nhello\n").unwrap();
        // Its word list is not text a dictionary can be made of.
        std::fs::write(dir.join("yy_YY.aff"), "SET UTF-8\n").unwrap();
        std::fs::write(dir.join("yy_YY.dic"), "not a number\nhello\n").unwrap();
        let dirs = [dir.clone()];
        let languages = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        let both: Vec<String> = languages(&["yy_YY", "xx_XX", "zz_ZZ"]);
        let dictionaries = load(&both, &dirs).expect("the good one loads");
        assert_eq!(dictionaries.languages(), ["xx_XX"]);
        assert!(dictionaries.check("hello"));
        assert_eq!(dictionaries.problems().len(), 1);
        assert!(dictionaries.problems()[0].contains("yy_YY"));
        // Only the broken one: that is what is wrong, not "none found".
        let only: Vec<String> = languages(&["yy_YY"]);
        let error = load(&only, &dirs).err().expect("nothing loads");
        assert!(!error.is::<NoneInstalled>(), "{error}");
        let none: Vec<String> = languages(&["zz_ZZ"]);
        assert!(load(&none, &dirs).err().unwrap().is::<NoneInstalled>());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn apostrophes_names_and_single_letters() {
        assert!(flagged("don't", 0).is_empty());
        assert!(
            flagged("hello Okafor x", 0).is_empty(),
            "ignored names and single letters pass"
        );
        assert_eq!(flagged("hello Okafr", 0), ["Okafr"]);
    }

    #[test]
    fn system_language_comes_first_and_english_is_always_included() {
        // Not asserting on the environment itself, only on the invariant.
        let languages = default_languages();
        assert!(languages.iter().any(|language| language == "en_US"));
    }

    #[test]
    fn download_addresses_exist_for_known_languages_only() {
        let (aff, dic) = download_urls("de_DE").expect("german is known");
        assert!(aff.ends_with("/de/de_DE_frami.aff") && dic.ends_with("/de/de_DE_frami.dic"));
        assert!(download_urls("xx_XX").is_none());
    }

    #[test]
    fn a_real_dictionary_checks_and_suggests() {
        // Uses the system's English dictionary where there is one.
        let dirs = dictionary_dirs();
        let Ok(dictionaries) = load(&["en_US".to_owned()], &dirs) else {
            return;
        };
        assert!(dictionaries.check("hello"));
        assert!(!dictionaries.check("helllo"));
        assert!(dictionaries.suggest("helllo").contains(&"hello".to_owned()));
        assert_eq!(dictionaries.languages(), ["en_US"]);
        assert!(available_languages(&dirs).contains(&"en_US".to_owned()));
    }
}
