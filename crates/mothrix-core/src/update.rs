// SPDX-License-Identifier: MPL-2.0

//! Updates of the terminal client from its GitHub releases. A self-updater
//! runs what it downloads, so nothing from the network counts until a
//! signature says so: each release carries `release.json` (its version and
//! the SHA-256 of every file) and `release.json.sig`, an Ed25519 signature
//! of it made with a key that is not on GitHub in plain form. The public
//! half is built in (`release_key.pub`, written by `dev/release keygen`);
//! without it, nothing updates. A changed file, another key, and an old
//! release under a new tag are all refused (the signed manifest names its
//! version).
//!
//! Only release builds install anything; packaged ones (`.deb`, PKGBUILD)
//! say that a release is out and leave updating to the package manager,
//! and builds that are not releases never look ([`Build`]). Fetching goes
//! through [`Fetch`]: the client with the proxy setting ([`Http`]), or a
//! stand-in in tests.

use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use ed25519_dalek::Signature;
pub use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The signed list of a release's files.
pub const MANIFEST: &str = "release.json";
/// Its signature (base64).
pub const SIGNATURE: &str = "release.json.sig";

/// The key releases are signed with (base64), empty until one is made.
const KEY: &str = include_str!("release_key.pub");

const MAX_MANIFEST: usize = 64 << 10;
const MAX_SIGNATURE: usize = 1 << 10;
const MAX_FILE: usize = 256 << 20;

/// The key releases must be signed with; `None` if this build has none,
/// and then updates stay off rather than running something unverified.
pub fn public_key() -> Option<VerifyingKey> {
    parse_key(KEY)
}

/// A base64 Ed25519 public key.
pub fn parse_key(text: &str) -> Option<VerifyingKey> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .ok()?;
    VerifyingKey::from_bytes(&bytes.try_into().ok()?).ok()
}

/// What a build may do about updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    /// A release file from GitHub: it looks for releases and installs them.
    Release,
    /// Built by a package (`MOTHRIX_PACKAGED`): it says that a release is
    /// out; the package manager updates it.
    Packaged,
    /// Not a release (its version names a commit): it never looks.
    Development,
}

impl Build {
    /// From the version shown (`MOTHRIX_VERSION`), the version of the
    /// workspace and whether a package built it.
    pub fn of(shown: &str, version: &str, packaged: bool) -> Self {
        match (shown == version, packaged) {
            (false, _) => Self::Development,
            (true, true) => Self::Packaged,
            (true, false) => Self::Release,
        }
    }
}

/// What `release.json` says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// File name to its SHA-256 (hex).
    pub files: BTreeMap<String, String>,
}

/// A newer release, its manifest signed with the right key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    pub version: String,
    /// Its page on GitHub.
    pub page: String,
    /// This system's file: its name, address and SHA-256; `None` when the
    /// release has none for it.
    pub file: Option<(String, String, String)>,
}

/// Fetching from the network: [`Http`], or a stand-in in tests.
pub trait Fetch {
    /// The body at `url`; an error past `limit` bytes.
    fn get(&self, url: &str, limit: usize) -> impl Future<Output = Result<Vec<u8>>> + Send;
    /// Where `url` leads after its redirects (asked with HEAD).
    fn lands(&self, url: &str) -> impl Future<Output = Result<String>> + Send;
}

/// Fetching with the client of the config (its proxy).
pub struct Http {
    pub client: matrix_sdk::reqwest::Client,
    /// Who asks (GitHub wants a User-Agent): "mothrix/0.2.0".
    pub agent: String,
}

impl Fetch for Http {
    fn get(&self, url: &str, limit: usize) -> impl Future<Output = Result<Vec<u8>>> + Send {
        let request = self
            .client
            .get(url)
            .header("User-Agent", &self.agent)
            .header("Accept", "application/octet-stream");
        let url = url.to_owned();
        async move {
            let mut response = request
                .send()
                .await
                .with_context(|| format!("could not reach {url}"))?
                .error_for_status()?;
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                body.extend_from_slice(&chunk);
                if body.len() > limit {
                    bail!("{url} is larger than expected");
                }
            }
            Ok(body)
        }
    }

    fn lands(&self, url: &str) -> impl Future<Output = Result<String>> + Send {
        let request = self.client.head(url).header("User-Agent", &self.agent);
        let url = url.to_owned();
        async move {
            let response = request
                .send()
                .await
                .with_context(|| format!("could not reach {url}"))?
                .error_for_status()?;
            Ok(response.url().to_string())
        }
    }
}

/// Where the releases of `repository` (`https://github.com/owner/name`)
/// are: the latest (a redirect to its page, which GitHub does not limit
/// as it limits its API to 60 questions an hour for an address), the
/// downloads and the pages.
struct Releases {
    latest: String,
    download: String,
    page: String,
}

impl Releases {
    fn of(repository: &str) -> Result<Self> {
        let path = repository
            .trim_end_matches('/')
            .strip_prefix("https://github.com/")
            .ok_or_else(|| anyhow!("updates come from GitHub, not {repository}"))?;
        Ok(Self {
            latest: format!("https://github.com/{path}/releases/latest"),
            download: format!("https://github.com/{path}/releases/download"),
            page: format!("https://github.com/{path}/releases/tag"),
        })
    }
}

/// The file a release has for this system, if it builds one.
pub fn file_name(version: &str) -> Option<String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some(format!("mothrix-{version}-x86_64-linux")),
        ("windows", "x86_64") => Some(format!("mothrix-{version}-x86_64-windows.exe")),
        _ => None,
    }
}

/// The latest release of `repository`, if it is newer than `current` and
/// its manifest is signed with `key`. An unsigned or wrongly signed one is
/// an error, not "nothing new".
pub async fn check(
    fetch: &impl Fetch,
    repository: &str,
    current: &str,
    key: &VerifyingKey,
) -> Result<Option<Available>> {
    let releases = Releases::of(repository)?;
    let landed = fetch.lands(&releases.latest).await?;
    let tag = landed
        .strip_prefix(&format!("{}/", releases.page))
        .filter(|tag| {
            !tag.is_empty()
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-+".contains(c))
        })
        .ok_or_else(|| anyhow!("GitHub names no latest release ({landed})"))?
        .to_owned();
    let version = tag.strip_prefix('v').unwrap_or(&tag).to_owned();
    if !newer(&version, current) {
        return Ok(None);
    }
    let base = format!("{}/{tag}", releases.download);
    let manifest = fetch
        .get(&format!("{base}/{MANIFEST}"), MAX_MANIFEST)
        .await?;
    let signature = fetch
        .get(&format!("{base}/{SIGNATURE}"), MAX_SIGNATURE)
        .await?;
    let manifest = verify(&manifest, &signature, key)?;
    if manifest.version != version {
        bail!(
            "release {tag} is signed as {} (an old release under a new name?)",
            manifest.version
        );
    }
    let file = file_name(&version).and_then(|name| {
        let sha256 = manifest.files.get(&name)?.clone();
        Some((name.clone(), format!("{base}/{name}"), sha256))
    });
    Ok(Some(Available {
        version,
        page: format!("{}/{tag}", releases.page),
        file,
    }))
}

/// A manifest whose signature (base64) `key` made.
pub fn verify(manifest: &[u8], signature: &[u8], key: &VerifyingKey) -> Result<Manifest> {
    let signature = base64::engine::general_purpose::STANDARD
        .decode(String::from_utf8_lossy(signature).trim())
        .ok()
        .and_then(|bytes| Signature::try_from(bytes.as_slice()).ok())
        .ok_or_else(|| anyhow!("the release's signature is not one"))?;
    key.verify_strict(manifest, &signature)
        .map_err(|_| anyhow!("the release is not signed with mothrix's key"))?;
    serde_json::from_slice(manifest).context("the release's manifest was not understood")
}

/// Whether `version` (X.Y.Z) comes after `current`.
pub fn newer(version: &str, current: &str) -> bool {
    let parse = |text: &str| -> Option<(u64, u64, u64)> {
        let mut parts = text.split('.').map(|part| part.parse().ok());
        let version = (parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    };
    match (parse(version), parse(current)) {
        (Some(version), Some(current)) => version > current,
        _ => false,
    }
}

/// This system's file of `available`, checked against its SHA-256.
pub async fn download(fetch: &impl Fetch, available: &Available) -> Result<Vec<u8>> {
    let (name, url, sha256) = available
        .file
        .as_ref()
        .ok_or_else(|| anyhow!("mothrix {} has no file for this system", available.version))?;
    let data = fetch.get(url, MAX_FILE).await?;
    if hex::encode(Sha256::digest(&data)) != sha256.to_lowercase() {
        bail!("{name} is not the file the release lists");
    }
    Ok(data)
}

/// What [`install`] did, to undo it if the new program does not start.
pub struct Installed {
    exe: PathBuf,
    old: PathBuf,
}

impl Installed {
    /// Where the new program now is: the path the old one had. The
    /// running process cannot ask for it any more: the new program was
    /// moved over the name it was started under, and on Linux its own
    /// path (`current_exe`) then reads "… (deleted)".
    pub fn program(&self) -> &Path {
        &self.exe
    }

    /// Put the old program back.
    pub fn undo(self) -> Result<()> {
        // (Windows does not move a file over another.)
        if cfg!(not(unix)) {
            std::fs::remove_file(&self.exe)?;
        }
        std::fs::rename(&self.old, &self.exe)?;
        Ok(())
    }
}

/// Put a verified new program in place of `exe`. Windows will not
/// overwrite a running program but lets it be renamed, so the current one
/// moves aside (`.old`, removed at the next start by [`cleanup`]) and the
/// new one takes its name.
pub fn install(exe: &Path, data: &[u8]) -> Result<Installed> {
    install_if(exe, data, |_| Ok(()))
}

/// [`install`], after the new program was started once and said it is
/// mothrix `version`. One built for another system (another C library,
/// say) would take the old one's place and then not start: there would be
/// no mothrix left to put it right with.
pub fn install_tried(exe: &Path, data: &[u8], version: &str) -> Result<Installed> {
    install_if(exe, data, |new| says_version(new, version, TRIAL))
}

/// How long the new program gets to say its version.
const TRIAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Start `new` with `--version` and see that it says it is mothrix
/// `version`, within `limit`: a program that hangs instead (waiting for
/// a library, a terminal, anything) must not keep the running mothrix
/// waiting for ever, and is not one to put in the old one's place.
fn says_version(new: &Path, version: &str, limit: std::time::Duration) -> Result<()> {
    use std::process::Stdio;
    let refused = || anyhow!("the new mothrix does not start on this system; the old one stays");
    let mut child = std::process::Command::new(new)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| refused())?;
    // (One line fits the pipe: nothing has to be read while it runs.)
    let started = std::time::Instant::now();
    while matches!(child.try_wait(), Ok(None)) {
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(refused());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let said = child
        .wait_with_output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    if said
        .trim()
        .strip_prefix("mothrix ")
        .is_some_and(|rest| rest.starts_with(version))
    {
        Ok(())
    } else {
        Err(refused())
    }
}

fn install_if(
    exe: &Path,
    data: &[u8],
    good: impl FnOnce(&Path) -> Result<()>,
) -> Result<Installed> {
    // Under a name of this process's own: `/update` here and
    // `mothrix update` in another terminal would else write one file.
    let new = with_suffix(exe, &format!("{NEW}.{}", std::process::id()));
    let old = with_suffix(exe, "old");
    if let Err(error) = write_program(&new, data) {
        let _ = std::fs::remove_file(&new);
        return Err(describe(error, exe));
    }
    // Who may start the old one may start the new one, no more.
    if let Ok(now) = std::fs::metadata(exe) {
        let _ = std::fs::set_permissions(&new, now.permissions());
    }
    if let Err(error) = good(&new) {
        let _ = std::fs::remove_file(&new);
        return Err(error);
    }
    let _ = std::fs::remove_file(&old);
    // Where a file can take a running program's place in one step (not
    // on Windows), there is no moment without a mothrix: the old one gets
    // its second name first, then the new one moves over the first.
    let aside = if cfg!(unix) {
        std::fs::hard_link(exe, &old)
    } else {
        std::fs::rename(exe, &old)
    };
    if let Err(error) = aside {
        let _ = std::fs::remove_file(&new);
        return Err(describe(error, exe));
    }
    if let Err(error) = std::fs::rename(&new, exe) {
        // Whatever happens, mothrix must still be there next time.
        if cfg!(unix) {
            let _ = std::fs::remove_file(&old);
        } else {
            std::fs::rename(&old, exe).context("could not put the old mothrix back")?;
        }
        let _ = std::fs::remove_file(&new);
        return Err(describe(error, exe));
    }
    Ok(Installed {
        exe: exe.to_owned(),
        old,
    })
}

/// What a new program is called while it is written and tried, with the
/// installing process's id after it (`mothrix.new.1234`).
const NEW: &str = "new";

/// A new program still there after this long was left by an install that
/// did not end (a crash, a kill): one under way takes seconds from
/// writing the file to moving it into place.
const LEFT_OVER: std::time::Duration = std::time::Duration::from_secs(3600);

/// Remove what an earlier update left: the old program, and new ones
/// that never got into place. Not a new one written lately: another
/// mothrix may be installing right now, and would find its file gone.
/// (Going by the file's age needs nothing of the system; which process
/// wrote it, and whether that one still lives, would.) Failing is
/// harmless: the next start tries again.
pub fn cleanup(exe: &Path) {
    cleanup_older(exe, LEFT_OVER);
}

fn cleanup_older(exe: &Path, left_over: std::time::Duration) {
    let _ = std::fs::remove_file(with_suffix(exe, "old"));
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
        return;
    };
    let new = format!("{}.{NEW}", name.to_string_lossy());
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let ours = entry
            .file_name()
            .to_string_lossy()
            .strip_prefix(&new)
            // `mothrix.new` (as it was called before) or `mothrix.new.<pid>`.
            .is_some_and(|rest| {
                rest.is_empty()
                    || rest
                        .strip_prefix('.')
                        .is_some_and(|pid| pid.parse::<u32>().is_ok())
            });
        let old = entry
            .metadata()
            .and_then(|data| data.modified())
            .ok()
            .and_then(|changed| changed.elapsed().ok())
            .is_some_and(|since| since >= left_over);
        if ours && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{suffix}"));
    PathBuf::from(name)
}

fn write_program(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    // A file of our own making, not one (or a link) someone put there
    // under this name for us to fill.
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o755);
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()
}

/// The usual cause, said: mothrix is somewhere its user cannot write.
fn describe(error: std::io::Error, exe: &Path) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        anyhow!(
            "mothrix is in a folder you cannot write to ({}): update it the way it was installed",
            exe.parent().unwrap_or(exe).display()
        )
    } else {
        anyhow!("could not put the new mothrix in place: {error}")
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    const REPO: &str = "https://github.com/r1sq/mothrix";

    /// Answers from memory; records what was asked.
    #[derive(Default)]
    struct Stand {
        bodies: HashMap<String, Vec<u8>>,
        /// Where an address leads.
        redirects: HashMap<String, String>,
        asked: Mutex<Vec<String>>,
    }

    impl Fetch for Stand {
        fn get(&self, url: &str, limit: usize) -> impl Future<Output = Result<Vec<u8>>> + Send {
            self.asked.lock().unwrap().push(url.to_owned());
            let body = self
                .bodies
                .get(url)
                .cloned()
                .ok_or_else(|| anyhow!("404 {url}"))
                .and_then(|body| {
                    if body.len() > limit {
                        bail!("too large");
                    }
                    Ok(body)
                });
            async move { body }
        }

        fn lands(&self, url: &str) -> impl Future<Output = Result<String>> + Send {
            self.asked.lock().unwrap().push(url.to_owned());
            let landed = Ok(self.redirects.get(url).cloned().unwrap_or(url.to_owned()));
            async move { landed }
        }
    }

    const LATEST: &str = "https://github.com/r1sq/mothrix/releases/latest";

    fn page(tag: &str) -> String {
        format!("https://github.com/r1sq/mothrix/releases/tag/{tag}")
    }

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    /// A release `tag` whose manifest says `version`, with one file for
    /// this system holding `data`, signed with `signer`.
    fn release(tag: &str, version: &str, data: &[u8], signer: &SigningKey) -> Stand {
        let base = format!("https://github.com/r1sq/mothrix/releases/download/{tag}");
        let name = file_name(version).unwrap_or_else(|| "none".into());
        let manifest = Manifest {
            version: version.into(),
            files: BTreeMap::from([(name.clone(), hex::encode(Sha256::digest(data)))]),
        };
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let signature =
            base64::engine::general_purpose::STANDARD.encode(signer.sign(&manifest).to_bytes());
        let mut stand = Stand::default();
        stand.redirects.insert(LATEST.into(), page(tag));
        stand.bodies.insert(format!("{base}/{MANIFEST}"), manifest);
        stand
            .bodies
            .insert(format!("{base}/{SIGNATURE}"), signature.into_bytes());
        stand.bodies.insert(format!("{base}/{name}"), data.to_vec());
        stand
    }

    fn run<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn a_newer_signed_release_is_offered_and_its_file_checked() {
        let signer = key(1);
        let stand = release("v0.2.0", "0.2.0", b"new mothrix", &signer);
        let available = run(check(&stand, REPO, "0.1.0", &signer.verifying_key()))
            .unwrap()
            .expect("newer");
        assert_eq!(available.version, "0.2.0");
        assert_eq!(
            available.page,
            "https://github.com/r1sq/mothrix/releases/tag/v0.2.0"
        );
        if file_name("0.2.0").is_some() {
            assert_eq!(run(download(&stand, &available)).unwrap(), b"new mothrix");
        }
        // Not newer: nothing, and no manifest fetched.
        let stand = release("v0.1.0", "0.1.0", b"same", &signer);
        assert_eq!(
            run(check(&stand, REPO, "0.1.0", &signer.verifying_key())).unwrap(),
            None
        );
        assert_eq!(stand.asked.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_latest_release_is_where_its_page_leads_and_nothing_else() {
        let signer = key(1);
        // No release yet: GitHub leads to the list of releases.
        let mut stand = release("v0.2.0", "0.2.0", b"x", &signer);
        stand.redirects.insert(
            LATEST.into(),
            "https://github.com/r1sq/mothrix/releases".into(),
        );
        let error = run(check(&stand, REPO, "0.1.0", &signer.verifying_key())).unwrap_err();
        assert!(error.to_string().contains("no latest release"), "{error}");
        // A page of another repository, or a tag that is no plain name.
        for elsewhere in [
            "https://github.com/someone/else/releases/tag/v0.2.0".to_owned(),
            page("v0.2.0/../../x"),
            page("v0.2.0?x=1"),
        ] {
            stand.redirects.insert(LATEST.into(), elsewhere);
            assert!(run(check(&stand, REPO, "0.1.0", &signer.verifying_key())).is_err());
        }
        // GitHub's API is not asked.
        assert!(
            stand
                .asked
                .lock()
                .unwrap()
                .iter()
                .all(|url| !url.contains("api.github.com"))
        );
    }

    #[test]
    fn another_key_a_changed_file_or_an_old_release_renamed_are_refused() {
        let signer = key(1);
        let stranger = key(2);
        let stand = release("v0.2.0", "0.2.0", b"x", &stranger);
        let error = run(check(&stand, REPO, "0.1.0", &signer.verifying_key())).unwrap_err();
        assert!(error.to_string().contains("not signed"), "{error}");

        // 0.1.5's signed manifest, published as v0.2.0.
        let mut stand = release("v0.1.5", "0.1.5", b"x", &signer);
        stand.redirects.insert(LATEST.into(), page("v0.2.0"));
        let old = "https://github.com/r1sq/mothrix/releases/download/v0.1.5";
        let new = "https://github.com/r1sq/mothrix/releases/download/v0.2.0";
        for name in [MANIFEST, SIGNATURE] {
            let body = stand.bodies[&format!("{old}/{name}")].clone();
            stand.bodies.insert(format!("{new}/{name}"), body);
        }
        let error = run(check(&stand, REPO, "0.1.0", &signer.verifying_key())).unwrap_err();
        assert!(error.to_string().contains("signed as 0.1.5"), "{error}");

        // The file changed after signing.
        if let Some(name) = file_name("0.2.0") {
            let mut stand = release("v0.2.0", "0.2.0", b"good", &signer);
            let available = run(check(&stand, REPO, "0.1.0", &signer.verifying_key()))
                .unwrap()
                .unwrap();
            stand
                .bodies
                .insert(format!("{new}/{name}"), b"evil".to_vec());
            let error = run(download(&stand, &available)).unwrap_err();
            assert!(error.to_string().contains("not the file"), "{error}");
        }
    }

    #[test]
    fn versions_compare_by_number_and_builds_know_what_they_may_do() {
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "0.99.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.2.0-rc1", "0.1.0"));
        assert!(!newer("0.2", "0.1.0"));
        assert_eq!(Build::of("0.1.0", "0.1.0", false), Build::Release);
        assert_eq!(Build::of("0.1.0", "0.1.0", true), Build::Packaged);
        assert_eq!(
            Build::of("0.1.0 (ce613f6)", "0.1.0", true),
            Build::Development
        );
    }

    #[test]
    fn the_new_program_takes_the_old_one_s_place_and_can_be_put_back() {
        let dir = std::env::temp_dir().join(format!("mothrix-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("mothrix");
        std::fs::write(&exe, b"old").unwrap();
        let installed = install(&exe, b"new").unwrap();
        // What the restart starts: the new program, not the one moved aside.
        assert_eq!(installed.program(), exe);
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(with_suffix(&exe, "old")).unwrap(), b"old");
        installed.undo().unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");

        install(&exe, b"newer").unwrap();
        // What does not start and say it is that version is not put in
        // the old one's place.
        assert!(install_tried(&exe, b"not a program", "9.0.0").is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"newer");
        #[cfg(unix)]
        {
            let script = b"#!/bin/sh\necho mothrix 9.0.0\n";
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
            install_tried(&exe, script, "9.0.0").unwrap();
            assert_eq!(std::fs::read(&exe).unwrap(), script);
            // With the old one's mode.
            assert_eq!(
                std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert!(install_tried(&exe, script, "9.1.0").is_err());
        }
        std::fs::write(&exe, b"newer").unwrap();
        cleanup(&exe);
        assert!(!with_suffix(&exe, "old").exists());
        assert_eq!(std::fs::read(&exe).unwrap(), b"newer");
        // Nothing of the installs is left beside it.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_start_leaves_another_mothrix_s_install_alone_and_removes_what_a_crash_left() {
        let dir = std::env::temp_dir().join(format!("mothrix-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("mothrix");
        std::fs::write(&exe, b"this one").unwrap();
        // Another mothrix is installing: its new program is written, and
        // not in place yet.
        let theirs = dir.join("mothrix.new.4242");
        std::fs::write(&theirs, b"being installed").unwrap();
        // Files of the user's, whatever they are called.
        let users = [dir.join("mothrix.newsletter"), dir.join("mothrix.new.txt")];
        for file in &users {
            std::fs::write(file, b"mine").unwrap();
        }
        cleanup(&exe);
        assert!(theirs.exists(), "an install under way keeps its file");
        // Once it is old, a crash left it there (also under the name
        // earlier versions wrote).
        let before = dir.join("mothrix.new");
        std::fs::write(&before, b"left over").unwrap();
        cleanup_older(&exe, std::time::Duration::ZERO);
        assert!(!theirs.exists() && !before.exists());
        assert!(users.iter().all(|file| file.exists()));
        assert_eq!(std::fs::read(&exe).unwrap(), b"this one");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_new_program_that_hangs_is_not_waited_for() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("mothrix-trial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("mothrix.new");
        // It would say the right thing, after a minute.
        std::fs::write(&program, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let started = std::time::Instant::now();
        let limit = std::time::Duration::from_millis(300);
        assert!(says_version(&program, "9.0.0", limit).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::fs::write(&program, "#!/bin/sh\necho mothrix 9.0.0\n").unwrap();
        says_version(&program, "9.0.0", std::time::Duration::from_secs(10)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_built_in_key_is_none_or_a_key() {
        // Empty until `dev/release keygen` writes one.
        assert_eq!(public_key().is_some(), !KEY.trim().is_empty());
        assert!(parse_key("not a key").is_none());
    }
}
