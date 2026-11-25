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

/// Make `dir` (and what is above it, if missing) a folder only the user
/// can enter: accounts, drafts and opened attachments are in such folders,
/// and on a shared computer the usual mode would let others read them.
/// One made earlier with the usual mode is closed here too.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write a file only the user can read, whole or not at all: under
/// another name first (private from the start), then moved over `path`.
/// A full disk or a kill half way leaves the old file as it was.
///
/// Where `path` is a link (a `config.toml` kept with one's dotfiles), the
/// file it leads to is written, beside itself: moving the new file over
/// the link would leave a plain file in its place and the linked one as
/// it was.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let path = &linked_to(path);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(name);
    let mut options = std::fs::File::options();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// The file `path` leads to, through links (also one whose file is not
/// there yet); `path` itself where it is none.
fn linked_to(path: &Path) -> PathBuf {
    let mut at = path.to_owned();
    // Not for ever, should links lead in a circle.
    for _ in 0..16 {
        let Ok(target) = std::fs::read_link(&at) else {
            break;
        };
        // A relative link counts from the folder it is in.
        at = match at.parent() {
            Some(dir) => dir.join(target),
            None => target,
        };
    }
    at
}

/// Remove the files in `dir` last changed more than `age` ago (opened
/// attachments, which nothing else ever deletes).
pub fn sweep(dir: &Path, age: std::time::Duration) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let old = entry
            .metadata()
            .and_then(|data| data.modified())
            .ok()
            .and_then(|changed| changed.elapsed().ok())
            .is_some_and(|since| since > age);
        if old && entry.path().is_file() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_files_and_folders_are_the_user_s_only_and_replace_what_was_there() {
        let dir = std::env::temp_dir().join(format!("mothrix-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        private_dir(&dir).unwrap();
        let file = dir.join("state.json");
        write_private(&file, b"one").unwrap();
        write_private(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        // Nothing left of the name it was written under.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&file), 0o600);
        }
        sweep(&dir, std::time::Duration::from_secs(3600));
        assert!(file.exists(), "new files stay");
        sweep(&dir, std::time::Duration::ZERO);
        assert!(!file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_file_is_written_through_its_link() {
        let dir = std::env::temp_dir().join(format!("mothrix-linked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (dotfiles, config) = (dir.join("dotfiles"), dir.join("config"));
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        // The config is kept elsewhere, and linked to: once by its whole
        // path, once from the folder next door.
        let real = dotfiles.join("mothrix.toml");
        std::fs::write(&real, "old").unwrap();
        let link = config.join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let relative = config.join("relative.toml");
        std::os::unix::fs::symlink("../dotfiles/mothrix.toml", &relative).unwrap();
        for (path, text) in [(&link, "new"), (&relative, "newer")] {
            write_private(path, text.as_bytes()).unwrap();
            assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read_to_string(&real).unwrap(), text);
        }
        // Nothing is left beside either of them.
        assert_eq!(std::fs::read_dir(&dotfiles).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&config).unwrap().count(), 2);
        // A link to a file that is not there yet makes that file.
        let fresh = config.join("fresh.toml");
        std::os::unix::fs::symlink(dotfiles.join("fresh.toml"), &fresh).unwrap();
        write_private(&fresh, b"first").unwrap();
        assert_eq!(
            std::fs::read(dotfiles.join("fresh.toml")).unwrap(),
            b"first"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
