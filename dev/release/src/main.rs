// SPDX-License-Identifier: MPL-2.0

//! Signs mothrix's releases, with the key every installed copy trusts
//! (`crates/mothrix-core/src/update.rs` checks what this writes).
//!
//! ```text
//! cargo run -p mothrix-release -- keygen PRIVATE_KEY_FILE
//! cargo run -p mothrix-release -- sign VERSION DIR      (key in $RELEASE_SIGNING_KEY)
//! ```
//!
//! `keygen` writes the private key to a file (readable only by you; an
//! existing one is not overwritten) and the public key into the source
//! tree. It never prints the private key.
//!
//! `sign` writes `release.json` (the version and every file's SHA-256),
//! `release.json.sig` and `SHA256SUMS` for the files in DIR, and checks the
//! signature against the key built into mothrix: a secret that does not
//! match fails the release rather than shipping one that no installed copy
//! would accept.

use std::{collections::BTreeMap, path::Path, process::ExitCode};

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

/// Where mothrix keeps the public key, in this tree.
const PUBLIC_KEY_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/mothrix-core/src/release_key.pub"
);
/// The public key as built into mothrix from this tree.
const BUILT_IN: &str = include_str!("../../../crates/mothrix-core/src/release_key.pub");

const MANIFEST: &str = "release.json";
const SIGNATURE: &str = "release.json.sig";
const SUMS: &str = "SHA256SUMS";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["keygen", file] => keygen(Path::new(file)),
        ["sign", version, dir] => sign(version, Path::new(dir)),
        _ => Err("usage: mothrix-release keygen PRIVATE_KEY_FILE | sign VERSION DIR".into()),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mothrix-release: {error}");
            ExitCode::FAILURE
        }
    }
}

type Result<T> = std::result::Result<T, String>;

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn keygen(file: &Path) -> Result<()> {
    // Every installed copy trusts the key that is there, and nothing
    // signed with another: it is not replaced in passing.
    if std::fs::read_to_string(PUBLIC_KEY_FILE).is_ok_and(|key| !key.trim().is_empty()) {
        return Err(format!(
            "{PUBLIC_KEY_FILE} holds a key already, which installed copies of mothrix trust. \
             Empty that file first if a new key is really meant."
        ));
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|error| format!("no randomness: {error}"))?;
    let key = SigningKey::from_bytes(&seed);
    write_private(file, &format!("{}\n", base64(&seed)))?;
    let public = base64(key.verifying_key().as_bytes());
    std::fs::write(PUBLIC_KEY_FILE, format!("{public}\n"))
        .map_err(|error| format!("could not write {PUBLIC_KEY_FILE}: {error}"))?;
    println!(
        "The private key is in {}: keep it, and put its content into the",
        file.display()
    );
    println!("GitHub secret RELEASE_SIGNING_KEY of the environment \"release\".");
    println!("The public key ({public}) is in {PUBLIC_KEY_FILE}: commit it.");
    Ok(())
}

fn write_private(file: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut out = options.open(file).map_err(|error| {
        format!(
            "could not make {} (is there one already?): {error}",
            file.display()
        )
    })?;
    out.write_all(text.as_bytes())
        .map_err(|error| format!("could not write {}: {error}", file.display()))
}

fn sign(version: &str, dir: &Path) -> Result<()> {
    let secret = std::env::var("RELEASE_SIGNING_KEY")
        .map_err(|_| "RELEASE_SIGNING_KEY is not set".to_owned())?;
    let seed: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(secret.trim())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("RELEASE_SIGNING_KEY is not a key")?;
    let key = SigningKey::from_bytes(&seed);
    let built_in = base64::engine::general_purpose::STANDARD
        .decode(BUILT_IN.trim())
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
        .ok_or("this tree has no public key: run keygen first")?;
    if key.verifying_key() != built_in {
        return Err("RELEASE_SIGNING_KEY does not belong to the public key in this tree".into());
    }

    let mut files = BTreeMap::new();
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if [MANIFEST, SIGNATURE, SUMS].contains(&name.as_str()) || !entry.path().is_file() {
            continue;
        }
        let data = std::fs::read(entry.path()).map_err(|error| format!("{name}: {error}"))?;
        files.insert(name, hex::encode(Sha256::digest(&data)));
    }
    if files.is_empty() {
        return Err(format!("nothing to sign in {}", dir.display()));
    }
    let manifest = serde_json::to_vec_pretty(&serde_json::json!({
        "version": version,
        "files": files,
    }))
    .map_err(|error| error.to_string())?;
    let signature = key.sign(&manifest);
    built_in
        .verify_strict(&manifest, &signature)
        .map_err(|_| "the signature does not check out".to_owned())?;

    let sums: String = files
        .iter()
        .map(|(name, sha256)| format!("{sha256}  {name}\n"))
        .collect();
    let write = |name: &str, data: &[u8]| {
        std::fs::write(dir.join(name), data).map_err(|error| format!("{name}: {error}"))
    };
    write(MANIFEST, &manifest)?;
    write(
        SIGNATURE,
        format!("{}\n", base64(&signature.to_bytes())).as_bytes(),
    )?;
    write(SUMS, sums.as_bytes())?;
    println!("Signed mothrix {version}: {} files.", files.len());
    Ok(())
}
