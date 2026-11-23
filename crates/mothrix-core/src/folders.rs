// SPDX-License-Identifier: MPL-2.0

//! Where mothrix keeps its files. On a desktop: the system's folders for
//! settings, data and caches, each with a `mothrix` folder in it. An app
//! that must keep everything in one place it is given (iOS: the app group
//! the app and its notification extension share) names that folder with
//! [`set_root`] before anything else runs; then `config`, `data` and
//! `cache` are folders in it.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Keep everything under `root` from now on. Only the first call counts;
/// false if another root was set already.
pub fn set_root(root: &Path) -> bool {
    ROOT.set(root.to_owned()).is_ok() || ROOT.get().is_some_and(|set| set == root)
}

/// The folder given with [`set_root`], if any.
pub fn root() -> Option<&'static Path> {
    ROOT.get().map(PathBuf::as_path)
}

/// Settings: `config.toml`, `words.txt`.
pub fn config() -> Option<PathBuf> {
    within("config", dirs::config_dir)
}

/// One folder per account, `emoji.json`, `state.json`, dictionaries.
pub fn data() -> Option<PathBuf> {
    within("data", dirs::data_dir)
}

/// Attachments opened and pictures pasted.
pub fn cache() -> Option<PathBuf> {
    within("cache", dirs::cache_dir)
}

fn within(name: &str, system: fn() -> Option<PathBuf>) -> Option<PathBuf> {
    match root() {
        Some(root) => Some(root.join(name)),
        None => Some(system()?.join("mothrix")),
    }
}
