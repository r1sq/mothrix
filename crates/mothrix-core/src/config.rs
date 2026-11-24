// SPDX-License-Identifier: MPL-2.0

//! The config file (`config.toml`), which the terminal and the iPhone app
//! each keep: what is in it, reading and writing it. Each front end maps
//! it onto its own state; what one of them does not change (the
//! terminal's key bindings, own themes) stays as it was when it saves.
//!
//! A new preference is a field of [`Prefs`] with its default.

use std::{collections::BTreeMap, path::Path};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{accounts::Account, model::Section, prefs::Prefs, sidebar::Folds, spell};

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    /// The colour theme, by name (see [`crate::theme`]).
    #[serde(default)]
    pub theme: String,
    /// What the settings dialog changes, read and written as they are.
    #[serde(flatten)]
    pub prefs: Prefs,
    /// Dictionaries to use, like ["de_DE", "en_US"]. Empty: the system
    /// language plus US English.
    #[serde(default)]
    pub spell_languages: Vec<String>,
    /// Sidebar sections that are folded, like ["LOW PRIORITY"].
    #[serde(default)]
    pub folded: Vec<String>,
    /// With several accounts: the profile whose rooms the sidebar shows.
    /// Empty (the default): all accounts, each under its own line.
    #[serde(default)]
    pub account: String,
    /// The order of the accounts, as profile names. Accounts not named
    /// here follow, `default` first.
    #[serde(default)]
    pub account_order: Vec<String>,
    /// Names you gave accounts, by profile: `default = "Private"`.
    #[serde(default)]
    pub account_names: BTreeMap<String, String>,
    /// Your own sidebar sections (mixed list), in display order.
    #[serde(default)]
    pub sections: Vec<Section>,
    /// Other keys for the terminal's shortcuts, like `search = "ctrl+s"`.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// Your own colour themes.
    #[serde(default)]
    pub themes: Vec<CustomTheme>,
}

/// A theme in the config file: a name, optionally the built-in theme it
/// starts from, and the colours to change (`#rrggbb`). Read by
/// [`crate::theme::custom`].
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CustomTheme {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base: String,
    /// Painted by the iPhone app only; the terminal keeps its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_bg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_fg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<String>,
    /// Colours for the names of people writing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub senders: Vec<String>,
}

impl Default for Config {
    /// What an empty file holds: the defaults the fields are read with.
    fn default() -> Self {
        toml::from_str("").expect("every field has a default")
    }
}

/// What a front end shows that the file keeps: the settings, the accounts
/// as listed (order and names), and the sidebar.
pub struct Shown<'a> {
    pub prefs: &'a Prefs,
    pub accounts: &'a [Account],
    /// The account shown alone, if any.
    pub account_filter: Option<usize>,
    pub sections: &'a [Section],
    pub folded: &'a Folds,
}

impl Config {
    /// The config in `path`; the defaults where there is none or it cannot
    /// be read.
    pub fn load(path: &Path) -> Self {
        Self::read(path).unwrap_or_default()
    }

    /// The config in `path`, the defaults where there is no such file, and
    /// what is wrong with a file that is there but cannot be used. Who
    /// saves the config or needs its proxy asks here: a file with a typing
    /// mistake in it must not be taken for an empty one, written over, or
    /// lead to connecting without the proxy it names.
    pub fn read(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => anyhow::bail!("{} cannot be read: {error}", path.display()),
        };
        toml::from_str(&text).map_err(|error| {
            // Where it is, when the parser knows: for a value of the wrong
            // kind among the settings it only knows the file's start.
            let what = error.message();
            match error.span().filter(|span| span.start > 0) {
                Some(span) => {
                    let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
                    anyhow::anyhow!("{} has a mistake in line {line}: {what}", path.display())
                }
                None => anyhow::anyhow!("{} has a mistake: {what}", path.display()),
            }
        })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // It may hold an API key: readable by the user only.
        crate::folders::write_private(path, toml::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    /// The proxy to use: the config's, else the environment's.
    pub fn proxy(&self) -> Option<String> {
        let configured = self.prefs.proxy.trim();
        if !configured.is_empty() {
            return Some(configured.to_owned());
        }
        ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    /// An HTTP client for what is not Matrix (LanguageTool, downloads),
    /// through the proxy if there is one.
    pub fn http_client(&self) -> matrix_sdk::reqwest::Client {
        let builder = matrix_sdk::reqwest::Client::builder();
        let builder = match self
            .proxy()
            .map(|proxy| matrix_sdk::reqwest::Proxy::all(&proxy))
        {
            Some(Ok(proxy)) => builder.proxy(proxy),
            // A proxy that cannot be understood is no reason to go around
            // it: nothing gets out, and the request says why not.
            Some(Err(_)) => match matrix_sdk::reqwest::Proxy::all("http://proxy.invalid:9") {
                Ok(nowhere) => builder.proxy(nowhere),
                Err(_) => builder,
            },
            None => builder,
        };
        builder.build().unwrap_or_default()
    }

    /// The dictionaries named, or the defaults.
    pub fn dictionaries(&self) -> Vec<String> {
        if self.spell_languages.is_empty() {
            spell::default_languages()
        } else {
            self.spell_languages.clone()
        }
    }

    /// Put profiles in the order the user chose; ones not named keep
    /// their place after the others.
    pub fn order_profiles(&self, profiles: &mut [String]) {
        profiles.sort_by_key(|profile| {
            self.account_order
                .iter()
                .position(|name| name == profile)
                .unwrap_or(usize::MAX)
        });
    }

    /// Give the accounts the names chosen for them.
    pub fn name_accounts(&self, accounts: &mut [Account]) {
        for account in accounts {
            if let Some(name) = self.account_names.get(&account.profile)
                && !name.trim().is_empty()
            {
                account.label = name.trim().to_owned();
            }
        }
    }

    /// The account to show alone, if one is chosen and there are several.
    pub fn account_filter(&self, accounts: &[Account]) -> Option<usize> {
        accounts
            .iter()
            .position(|account| account.profile == self.account)
            .filter(|_| accounts.len() > 1)
    }

    /// This config with what is shown now. What only the file holds, or
    /// only the other front end uses, stays: the theme, key bindings, own
    /// themes, dictionaries, and what is known of accounts that are not
    /// running (`--profile`).
    pub fn with_shown(&self, shown: Shown) -> Self {
        let several = shown.accounts.len() > 1;
        let mut account_names = self.account_names.clone();
        for account in shown.accounts {
            if account.label == account.profile {
                account_names.remove(&account.profile);
            } else {
                account_names.insert(account.profile.clone(), account.label.clone());
            }
        }
        let folded = shown.folded.keys();
        Self {
            prefs: shown.prefs.clone(),
            account_order: if several {
                shown
                    .accounts
                    .iter()
                    .map(|account| account.profile.clone())
                    .collect()
            } else {
                self.account_order.clone()
            },
            account_names,
            // Kept while running with one account only.
            account: match shown.account_filter {
                Some(account) => shown.accounts[account].profile.clone(),
                None if several => String::new(),
                None => self.account.clone(),
            },
            sections: shown.sections.to_vec(),
            folded,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::SyncStatus;

    fn account(profile: &str) -> Account {
        Account {
            profile: profile.into(),
            label: profile.into(),
            user_id: format!("@me:{profile}"),
            own_name: None,
            sync: SyncStatus::Connecting,
            verified: None,
        }
    }

    #[test]
    fn a_file_with_a_mistake_is_not_taken_for_an_empty_one() {
        let dir = std::env::temp_dir().join(format!("mothrix-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        // No file: the defaults, and nothing wrong.
        assert!(Config::read(&file).unwrap().prefs.proxy.is_empty());
        std::fs::write(
            &file,
            "proxy = \"http://proxy:3128\"\nnotifications = \"yes\"\n",
        )
        .unwrap();
        let error = Config::read(&file).err().expect("a mistake").to_string();
        assert!(error.contains("config.toml has a mistake: "), "{error}");
        assert!(error.contains("expected a boolean"), "{error}");
        std::fs::write(&file, "proxy = \"http://proxy:3128\"\ntheme = \n").unwrap();
        let error = Config::read(&file).err().expect("a mistake").to_string();
        assert!(error.contains("has a mistake in line 2"), "{error}");
        // The defaults for showing, but who asks `read` knows not to save.
        assert!(Config::load(&file).prefs.proxy.is_empty());
        std::fs::write(&file, "proxy = \"http://proxy:3128\"\n").unwrap();
        let config = Config::read(&file).unwrap();
        config.save(&file).unwrap();
        assert_eq!(
            Config::read(&file).unwrap().proxy().as_deref(),
            Some("http://proxy:3128")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_proxy_in_the_file_wins_and_is_kept_when_saving() {
        let config: Config = toml::from_str(r#"proxy = " http://172.28.28.49:3128 ""#).unwrap();
        assert_eq!(config.proxy().as_deref(), Some("http://172.28.28.49:3128"));
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("proxy = "));
        // None: direct, unless the environment names one.
        assert!(Config::default().prefs.proxy.is_empty());
    }

    #[test]
    fn profiles_follow_the_chosen_order_and_new_ones_come_last() {
        let config: Config = toml::from_str(r#"account_order = ["work", "default"]"#).unwrap();
        let mut profiles = vec!["default".to_owned(), "school".into(), "work".into()];
        config.order_profiles(&mut profiles);
        assert_eq!(profiles, ["work", "default", "school"]);
    }

    #[test]
    fn what_is_shown_goes_in_and_the_rest_stays() {
        let config: Config = toml::from_str(
            r##"
            theme = "nord"
            [keys]
            search = "ctrl+s"
            [account_names]
            gone = "Old"
            "##,
        )
        .unwrap();
        let mut accounts = vec![account("default"), account("work")];
        accounts[1].label = "Job".into();
        let prefs = Prefs {
            mixed_list: true,
            ..Prefs::default()
        };
        let sections = vec![Section {
            name: "Family".into(),
            rooms: vec!["!a".into()],
        }];
        let folded = Folds::new(["work:CHATS".to_owned()]);
        let saved = config.with_shown(Shown {
            prefs: &prefs,
            accounts: &accounts,
            account_filter: Some(1),
            sections: &sections,
            folded: &folded,
        });
        assert_eq!(saved.theme, "nord");
        assert_eq!(saved.keys.get("search").map(String::as_str), Some("ctrl+s"));
        assert!(saved.prefs.mixed_list);
        assert_eq!(saved.account, "work");
        assert_eq!(saved.account_order, ["default", "work"]);
        assert_eq!(saved.account_names.len(), 2);
        assert_eq!(saved.sections, sections);
        assert_eq!(saved.folded, ["work:CHATS"]);

        let mut again = vec![account("default"), account("work")];
        saved.name_accounts(&mut again);
        assert_eq!(again[1].label, "Job");
        assert_eq!(saved.account_filter(&again), Some(1));
    }
}
