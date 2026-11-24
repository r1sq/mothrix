// SPDX-License-Identifier: MPL-2.0

//! Updates of mothrix itself, as the app sees them: what is known of a
//! newer release, telling the user once, `/update`, and the settings row.
//! Checking, verifying and installing are core's (`update.rs`); the main
//! loop carries out [`Effect::CheckForUpdate`] and [`Effect::InstallUpdate`]
//! and hands back what came of them.

use super::*;
use crate::update::{Available, Build};

/// What the app knows about updates.
#[derive(Debug)]
pub struct Updates {
    pub build: Build,
    /// Whether this build has the key releases are signed with.
    pub can_check: bool,
    pub available: Option<Available>,
    /// Downloading and installing right now.
    pub installing: bool,
    /// The version the user was last told about.
    told: Option<String>,
}

impl Updates {
    pub fn new(build: Build, can_check: bool) -> Self {
        Self {
            build,
            can_check,
            available: None,
            installing: false,
            told: None,
        }
    }

    /// Whether to look for a release now and then.
    pub fn looks(&self, prefs: &mothrix_core::prefs::Prefs) -> bool {
        prefs.updates && self.can_check && self.build != Build::Development
    }
}

impl App {
    /// What a look for a new release found; `asked`: by `/update`.
    pub fn update_checked(&mut self, found: Result<Option<Available>, String>, asked: bool) {
        match found {
            Ok(Some(available)) => {
                let new = self.updates.told.as_ref() != Some(&available.version);
                self.updates.told = Some(available.version.clone());
                self.updates.available = Some(available.clone());
                if asked && self.updates.build == Build::Release && available.file.is_some() {
                    self.install_update(available);
                } else if new || asked {
                    self.notice = Some(self.update_news(&available));
                }
            }
            Ok(None) => {
                self.updates.available = None;
                if asked {
                    self.notice =
                        Some(format!("mothrix {} is the newest release.", crate::VERSION));
                }
            }
            Err(error) if asked => {
                self.notice = Some(format!("Could not look for a new release: {error}"));
            }
            Err(error) => tracing::warn!("looking for a new release: {error}"),
        }
    }

    /// Installing did not work.
    pub fn update_failed(&mut self, error: String) {
        self.updates.installing = false;
        self.notice = Some(format!("Could not update: {error}"));
    }

    /// What to tell about a newer release.
    fn update_news(&self, available: &Available) -> String {
        let version = &available.version;
        match (self.updates.build, &available.file) {
            (Build::Packaged, _) => format!("mothrix {version} is out: update your package."),
            (_, Some(_)) => format!("mothrix {version} is out: /update installs it."),
            (_, None) => format!("mothrix {version} is out: {}", available.page),
        }
    }

    fn install_update(&mut self, available: Available) {
        // Once: two installs would write and move the same files.
        if self.updates.installing {
            return;
        }
        self.updates.installing = true;
        self.notice = Some(format!("Downloading mothrix {}…", available.version));
        self.effects.push(Effect::InstallUpdate(available));
    }

    /// `/update`: install the newer release, or look for one first.
    pub(super) fn update_command(&mut self) {
        let reason = match self.updates.build {
            Build::Development => Some(format!(
                "This build is not a release ({}): it does not update itself.",
                crate::VERSION
            )),
            _ if !self.updates.can_check => {
                Some("This build has no release key: it cannot look for updates.".to_owned())
            }
            _ if self.updates.installing => Some("The new release is on its way.".to_owned()),
            _ => None,
        };
        if let Some(reason) = reason {
            self.notice = Some(reason);
            return;
        }
        match self.updates.available.clone() {
            Some(available) if self.updates.build == Build::Release && available.file.is_some() => {
                self.install_update(available)
            }
            Some(available) => self.notice = Some(self.update_news(&available)),
            None => {
                self.notice = Some("Looking for a new release…".to_owned());
                self.effects.push(Effect::CheckForUpdate);
            }
        }
    }

    /// The settings row's value.
    pub(super) fn updates_setting(&self) -> String {
        let looks = if self.state.prefs.updates {
            "check daily"
        } else {
            "off"
        };
        let extra = match (&self.updates.available, self.updates.build) {
            (_, Build::Development) => " (not a release build)".to_owned(),
            _ if !self.updates.can_check => " (no release key in this build)".to_owned(),
            (Some(available), _) => format!(" · {} is out", available.version),
            (None, Build::Packaged) => " (tells only: a package)".to_owned(),
            (None, Build::Release) => String::new(),
        };
        format!("{looks}{extra}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(file: bool) -> Available {
        Available {
            version: "9.0.0".into(),
            page: "https://github.com/r1sq/mothrix/releases/tag/v9.0.0".into(),
            file: file.then(|| ("f".into(), "https://x/f".into(), "00".into())),
        }
    }

    fn app(build: Build) -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.updates = Updates::new(build, true);
        app
    }

    fn installs(app: &mut App) -> bool {
        app.take_effects()
            .iter()
            .any(|effect| matches!(effect, Effect::InstallUpdate(_)))
    }

    #[test]
    fn a_new_release_is_told_once_and_update_installs_it() {
        let mut app = app(Build::Release);
        app.update_checked(Ok(Some(release(true))), false);
        assert_eq!(
            app.notice.take().as_deref(),
            Some("mothrix 9.0.0 is out: /update installs it.")
        );
        // The daily look finds it again: no second notice, nothing installed.
        app.update_checked(Ok(Some(release(true))), false);
        assert_eq!(app.notice, None);
        assert!(!installs(&mut app));
        assert!(app.updates_setting().contains("9.0.0 is out"));

        app.update_command();
        assert!(installs(&mut app));
        assert!(app.updates.installing);
        // Asked again while downloading: nothing more.
        app.update_command();
        assert!(!installs(&mut app));
        app.update_failed("disk full".into());
        assert_eq!(app.notice.as_deref(), Some("Could not update: disk full"));
    }

    #[test]
    fn update_looks_first_when_nothing_is_known() {
        let mut app = app(Build::Release);
        app.update_command();
        assert!(
            app.take_effects()
                .iter()
                .any(|effect| matches!(effect, Effect::CheckForUpdate))
        );
        // What the look finds is installed at once, as asked.
        app.update_checked(Ok(Some(release(true))), true);
        assert!(installs(&mut app));
        // Nothing newer.
        let mut app = self::app(Build::Release);
        app.update_checked(Ok(None), true);
        assert!(app.notice.unwrap().contains("is the newest release"));
    }

    #[test]
    fn packaged_builds_only_tell_and_others_never_update() {
        let mut app = app(Build::Packaged);
        app.update_checked(Ok(Some(release(true))), false);
        assert_eq!(
            app.notice.take().as_deref(),
            Some("mothrix 9.0.0 is out: update your package.")
        );
        app.update_command();
        assert!(!installs(&mut app));
        assert!(app.notice.unwrap().contains("update your package"));

        let mut app = self::app(Build::Development);
        assert!(!app.updates.looks(&app.state.prefs));
        app.update_command();
        assert!(!installs(&mut app));
        assert!(app.notice.unwrap().contains("not a release"));

        let mut app = self::app(Build::Release);
        app.state.prefs.updates = false;
        assert!(!app.updates.looks(&app.state.prefs));
        assert!(app.updates_setting().starts_with("off"));
    }
}
