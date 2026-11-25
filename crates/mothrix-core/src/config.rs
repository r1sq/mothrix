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

    /// The proxies to use ([`proxy_rules`]), from this config and the
    /// environment.
    fn proxies(&self) -> Result<Vec<matrix_sdk::reqwest::Proxy>> {
        proxy_rules(&self.prefs.proxy, |name| std::env::var(name).ok())
            .iter()
            .map(ProxyRule::proxy)
            .collect()
    }

    /// An HTTP client for what is not Matrix (LanguageTool, downloads),
    /// through the proxy if there is one.
    pub fn http_client(&self) -> matrix_sdk::reqwest::Client {
        client_through(self.proxies())
    }

    /// [`Config::http_client`] of the config in `path`. A file with a
    /// mistake in it may name a proxy all the same: the client then
    /// reaches nothing, rather than go around it.
    pub fn http_client_at(path: &Path) -> matrix_sdk::reqwest::Client {
        client_through(Self::proxies_at(path))
    }

    fn proxies_at(path: &Path) -> Result<Vec<matrix_sdk::reqwest::Proxy>> {
        Self::read(path)?.proxies()
    }

    /// The HTTP client an account talks to its homeserver with, through
    /// the same proxies as everything else. The SDK would make its own
    /// from one proxy address: for every request, also to the hosts
    /// `NO_PROXY` names, and it writes the address (which may hold a
    /// password) into the log. Otherwise this is the client the SDK
    /// makes: its name, its time limit, no TLS older than 1.2.
    pub fn matrix_http_client(&self) -> Result<matrix_sdk::reqwest::Client> {
        let builder = matrix_sdk::reqwest::Client::builder()
            .user_agent("matrix-rust-sdk")
            .min_tls_version(matrix_sdk::reqwest::tls::Version::TLS_1_2)
            .timeout(std::time::Duration::from_secs(30));
        Ok(through(builder, self.proxies()?).build()?)
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

    /// Take out what is kept for the account of `profile`: its rooms in
    /// the sections, and what was folded for it. For when the profile is
    /// signed out or another account signs in under its name, outside a
    /// running front end. Returns whether there was anything.
    pub fn forget_profile(&mut self, profile: &str) -> bool {
        let mut sections = crate::sections::OwnSections::new(self.sections.clone());
        let prefixes = [crate::accounts::prefix_for(profile)];
        let mut changed = sections.forget_account(&prefixes, 0);
        if changed {
            self.sections = sections.to_vec();
        }
        let mut folded = Folds::new(self.folded.iter().cloned());
        if folded.forget_profile(profile) {
            self.folded = folded.keys();
            changed = true;
        }
        changed
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
                // Those running, in their order; then those that are not
                // (left out at this start), which keep a place for next
                // time.
                let running: Vec<String> = shown
                    .accounts
                    .iter()
                    .map(|account| account.profile.clone())
                    .collect();
                let others: Vec<String> = self
                    .account_order
                    .iter()
                    .filter(|profile| !running.contains(profile))
                    .cloned()
                    .collect();
                running.into_iter().chain(others).collect()
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

/// The kinds of proxy the HTTP library knows.
const PROXY_KINDS: [&str; 6] = ["http", "https", "socks4", "socks4a", "socks5", "socks5h"];

/// Which requests a proxy takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Takes {
    All,
    Https,
    Http,
}

/// One proxy, for some of the requests, and the hosts that are reached
/// without it (`NO_PROXY`, as it is written there).
#[derive(Debug, PartialEq, Eq)]
struct ProxyRule {
    takes: Takes,
    address: String,
    except: Option<String>,
}

impl ProxyRule {
    fn proxy(&self) -> Result<matrix_sdk::reqwest::Proxy> {
        use matrix_sdk::reqwest::{NoProxy, Proxy};
        // Not the address itself: it may hold a password, and this ends
        // up on the screen and in the log.
        let unusable = || {
            anyhow::anyhow!(
                "the proxy cannot be used: its address has to be like http://proxy:3128 \
                 (or https://, socks5://)"
            )
        };
        // "proxy:3128" is an HTTP proxy.
        let address = if self.address.contains("://") {
            self.address.clone()
        } else {
            format!("http://{}", self.address)
        };
        // Checked here: the HTTP library takes any address with a host,
        // and then goes without a proxy where it does not know the kind
        // (a slip like "sock5://").
        url::Url::parse(&address)
            .ok()
            .filter(|url| url.has_host() && PROXY_KINDS.contains(&url.scheme()))
            .ok_or_else(unusable)?;
        let proxy = match self.takes {
            Takes::All => Proxy::all(&address),
            Takes::Https => Proxy::https(&address),
            Takes::Http => Proxy::http(&address),
        }
        .map_err(|_| unusable())?;
        Ok(proxy.no_proxy(self.except.as_deref().and_then(NoProxy::from_string)))
    }
}

/// The proxies to use, in the order they are asked. One named in the
/// config (`configured`) takes every request: it is there for networks
/// with no other way out. Else the environment's (`env` reads a
/// variable), each for what its variable is for (`HTTPS_PROXY` does not
/// take `http://` requests, a LanguageTool server on this computer for
/// one) and none for the hosts `NO_PROXY` names.
fn proxy_rules(configured: &str, env: impl Fn(&str) -> Option<String>) -> Vec<ProxyRule> {
    let configured = configured.trim();
    if !configured.is_empty() {
        return vec![ProxyRule {
            takes: Takes::All,
            address: configured.to_owned(),
            except: None,
        }];
    }
    // The first that says something: an empty one names no proxy.
    let named = |names: [&str; 2]| {
        names
            .iter()
            .filter_map(|name| env(name))
            .map(|value| value.trim().to_owned())
            .find(|value| !value.is_empty())
    };
    let except = named(["NO_PROXY", "no_proxy"]);
    [
        (Takes::Https, ["HTTPS_PROXY", "https_proxy"]),
        (Takes::Http, ["HTTP_PROXY", "http_proxy"]),
        (Takes::All, ["ALL_PROXY", "all_proxy"]),
    ]
    .into_iter()
    .filter_map(|(takes, names)| {
        Some(ProxyRule {
            takes,
            address: named(names)?,
            except: except.clone(),
        })
    })
    .collect()
}

/// An HTTP client that goes through `proxies`.
fn client_through(proxies: Result<Vec<matrix_sdk::reqwest::Proxy>>) -> matrix_sdk::reqwest::Client {
    // A server that stops answering must not keep a download or a
    // grammar check waiting for ever.
    let builder = matrix_sdk::reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .read_timeout(std::time::Duration::from_secs(60));
    let builder = match proxies {
        Ok(proxies) => through(builder, proxies),
        // A proxy that cannot be understood is no reason to go around
        // it: nothing gets out, and the request says why not.
        Err(_) => match matrix_sdk::reqwest::Proxy::all("http://proxy.invalid:9") {
            Ok(nowhere) => builder.proxy(nowhere),
            Err(_) => builder,
        },
    };
    builder.build().unwrap_or_default()
}

/// `builder` with these proxies, and no others: without any, the HTTP
/// library would read the environment itself, by rules of its own.
fn through(
    builder: matrix_sdk::reqwest::ClientBuilder,
    proxies: Vec<matrix_sdk::reqwest::Proxy>,
) -> matrix_sdk::reqwest::ClientBuilder {
    proxies
        .into_iter()
        .fold(builder.no_proxy(), |builder, proxy| builder.proxy(proxy))
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
        // Nor to connect: the file names a proxy, mistake or not.
        assert!(Config::proxies_at(&file).is_err());
        std::fs::write(&file, "proxy = \"http://proxy:3128\"\n").unwrap();
        let config = Config::read(&file).unwrap();
        config.save(&file).unwrap();
        assert_eq!(
            Config::read(&file).unwrap().prefs.proxy,
            "http://proxy:3128"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_proxy_in_the_file_wins_and_is_kept_when_saving() {
        let config: Config = toml::from_str(r#"proxy = " http://172.28.28.49:3128 ""#).unwrap();
        let env = |name: &str| (name == "HTTPS_PROXY").then(|| "http://other:1".to_owned());
        // For everything, whatever the environment says.
        assert_eq!(
            proxy_rules(&config.prefs.proxy, env),
            [ProxyRule {
                takes: Takes::All,
                address: "http://172.28.28.49:3128".into(),
                except: None,
            }]
        );
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("proxy = "));
        // None: direct, unless the environment names one.
        assert!(Config::default().prefs.proxy.is_empty());
        assert!(proxy_rules("", |_| None).is_empty());
    }

    #[test]
    fn the_environment_s_proxies_each_take_what_they_are_for() {
        let env = |name: &str| match name {
            // An empty one names nothing; the other spelling counts.
            "HTTPS_PROXY" => Some(" ".to_owned()),
            "https_proxy" => Some("http://secure:3128".to_owned()),
            "ALL_PROXY" => Some("socks5://socks:1080".to_owned()),
            "NO_PROXY" => Some("localhost,127.0.0.1,.lan".to_owned()),
            _ => None,
        };
        let except = Some("localhost,127.0.0.1,.lan".to_owned());
        let rules = proxy_rules(" ", env);
        assert_eq!(
            rules,
            [
                // Not for `http://`: that goes on to ALL_PROXY.
                ProxyRule {
                    takes: Takes::Https,
                    address: "http://secure:3128".into(),
                    except: except.clone(),
                },
                ProxyRule {
                    takes: Takes::All,
                    address: "socks5://socks:1080".into(),
                    except,
                },
            ]
        );
        // All of them are proxies the HTTP library takes, SOCKS too.
        for rule in &rules {
            rule.proxy().unwrap();
        }
        // Without a kind it is an HTTP proxy.
        ProxyRule {
            takes: Takes::All,
            address: "proxy:3128".into(),
            except: None,
        }
        .proxy()
        .unwrap();
        let http = proxy_rules("", |name| {
            (name == "http_proxy").then(|| "http://plain:3128".to_owned())
        });
        assert_eq!(http[0].takes, Takes::Http);
    }

    #[test]
    fn a_proxy_that_cannot_be_understood_stops_requests_and_keeps_its_password() {
        for address in [
            "http://user:secret@",
            "gopher://user:secret@proxy:70",
            "sock5://user:secret@proxy:1080",
            "http://user:secret@pro xy:3128",
        ] {
            let rule = ProxyRule {
                takes: Takes::All,
                address: address.into(),
                except: None,
            };
            let error = rule.proxy().expect_err("no proxy").to_string();
            assert!(error.starts_with("the proxy cannot be used"), "{error}");
            assert!(!error.contains("secret"), "{error}");
        }
        // No client for the homeserver, rather than one that goes direct.
        let config: Config = toml::from_str(r#"proxy = "http://user:secret@""#).unwrap();
        assert!(config.matrix_http_client().is_err());
    }

    #[test]
    fn a_profile_s_sections_and_folds_leave_the_file_with_it() {
        let section = |name: &str, rooms: &[&str]| Section {
            name: name.into(),
            rooms: rooms.iter().map(|room| (*room).into()).collect(),
        };
        let mut config = Config {
            sections: vec![
                section("Team", &["!a", "work/!b"]),
                section("Clients", &["work/!c"]),
            ],
            folded: vec!["@work".into(), "work:Team".into(), "Team".into()],
            ..Config::default()
        };
        assert!(config.forget_profile("work"));
        assert_eq!(config.sections, [section("Team", &["!a"])]);
        assert_eq!(config.folded, ["Team"]);
        assert!(!config.forget_profile("work"));
        // The default profile's rooms are the ones without a prefix.
        assert!(config.forget_profile("default"));
        assert!(config.sections.is_empty());
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
