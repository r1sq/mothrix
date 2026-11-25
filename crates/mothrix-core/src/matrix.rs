// SPDX-License-Identifier: MPL-2.0

//! Everything that talks to matrix-sdk: session storage, login, and the
//! background task that feeds the UI with rooms and timeline snapshots.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use matrix_sdk::room::{IncludeRelations, RelationsOptions};
use matrix_sdk::{
    Client, Room, RoomMemberships, RoomState,
    attachment::{AttachmentInfo, BaseAudioInfo, BaseImageInfo, Thumbnail},
    authentication::matrix::MatrixSession,
    deserialized_responses::SyncOrStrippedState,
    encryption::verification::VerificationRequest,
    encryption::{
        BackupDownloadStrategy, CrossSigningResetAuthType, EncryptionSettings,
        recovery::RecoveryState,
    },
    media::{MediaFormat, MediaRequestParameters},
    notification_settings::{NotificationSettings, RoomNotificationMode},
    room::RoomMemberRole,
    room::edit::EditedContent,
    ruma::{
        EventId, MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedUserId, RoomId, RoomOrAliasId,
        UInt, UserId,
        api::client::{
            receipt::create_receipt::v3::ReceiptType, room::create_room,
            session::get_login_types::v3::LoginType, uiaa,
        },
        events::{
            AnyMessageLikeEventContent, AnySyncMessageLikeEvent, AnySyncTimelineEvent,
            InitialStateEvent, Mentions, StateEventType, SyncStateEvent,
            key::verification::request::ToDeviceKeyVerificationRequestEvent,
            poll::{
                start::PollKind,
                unstable_end::UnstablePollEndEventContent,
                unstable_response::UnstablePollResponseEventContent,
                unstable_start::{
                    NewUnstablePollStartEventContent, UnstablePollAnswer, UnstablePollAnswers,
                    UnstablePollStartContentBlock, UnstablePollStartEventContent,
                },
            },
            receipt::ReceiptThread,
            relation::RelationType,
            room::{
                MediaSource,
                encryption::RoomEncryptionEventContent,
                member::MembershipState,
                message::{
                    FormattedBody, MessageType, OriginalSyncRoomMessageEvent,
                    RoomMessageEventContent, RoomMessageEventContentWithoutRelation,
                    TextMessageEventContent,
                },
            },
            space::child::SpaceChildEventContent,
        },
        serde::Raw,
    },
};
use matrix_sdk_ui::{
    eyeball_im::{Vector, VectorDiff},
    room_list_service::filters::new_filter_non_left,
    sync_service::{State as SyncState, SyncService},
    timeline::{
        AttachmentConfig, AttachmentSource, EventTimelineItem, MsgLikeKind, RoomExt, Timeline,
        TimelineDetails, TimelineEventItemId, TimelineFocus, TimelineItem, TimelineItemContent,
        TimelineReadReceiptTracking, default_event_filter,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    convert::{self, Names, StablePolls},
    model::{
        Item, Member, Notify, PollFormat, Preview, RoomDetails, RoomEntry, RoomMember, SearchHit,
        Section, VerifyState,
    },
    preview,
    room_watch::{RoomWatch, Seen},
    stable_polls::{self, StablePoll},
    verify::{self, VerifyInput},
};

// ---------------------------------------------------------------------------
// Paths and session storage
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Paths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn for_profile(profile: &str) -> Result<Self> {
        let config = crate::folders::config().context("no config directory on this system")?;
        let data = crate::folders::data().context("no data directory on this system")?;
        Ok(Self {
            config_file: config.join("config.toml"),
            data_dir: data.join(profile),
        })
    }

    fn session_file(&self) -> PathBuf {
        self.data_dir.join("session.json")
    }

    fn store_dir(&self) -> PathBuf {
        self.data_dir.join("store")
    }

    pub fn log_file(&self) -> PathBuf {
        self.data_dir.join("mothrix.log")
    }
}

/// The profiles that hold a signed-in account, "default" first.
pub fn signed_in_profiles() -> Vec<String> {
    let Some(data) = crate::folders::data() else {
        return Vec::new();
    };
    let mut profiles: Vec<String> = std::fs::read_dir(data)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("session.json").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    profiles.sort_by_key(|profile| (profile != "default", profile.to_lowercase()));
    profiles
}

/// Whether a profile holds an account: its session file is there. Also
/// one that cannot be read counts (damaged, or a later mothrix's): the
/// store beside it holds that account's keys, and "no account here"
/// would let a sign-in delete them.
pub fn has_session(paths: &Paths) -> bool {
    paths.session_file().exists()
}

/// The Matrix id a profile is signed in as, read from its session file;
/// none also when the file cannot be read ([`has_session`] says whether
/// there is an account).
pub fn signed_in_as(paths: &Paths) -> Option<String> {
    let stored: StoredSession =
        serde_json::from_slice(&std::fs::read(paths.session_file()).ok()?).ok()?;
    Some(stored.session.meta.user_id.to_string())
}

#[derive(Serialize, Deserialize)]
struct StoredSession {
    homeserver: String,
    session: MatrixSession,
    /// Where the passphrase of the store is ([`crate::store_key`]); none
    /// for a store that is not encrypted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    store_key: Option<String>,
}

/// Attachments are handed to the app rather than opened here
/// ([`hand_media_over`]).
static HAND_MEDIA_OVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Do not open attachments with the desktop's viewer or play audio with
/// mpv: download them and say where they are ([`Update::MediaFile`]), for
/// an app that shows them itself (iOS: QuickLook).
pub fn hand_media_over() {
    HAND_MEDIA_OVER.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The name this process takes the SDK's cross-process store locks under,
/// when it shares account stores with another process (see
/// [`share_stores`]).
static SHARED_AS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// This process shares the account stores with another one that runs at
/// the same time: the iOS app and its notification extension, each under
/// its own `holder` name ("app", "nse"). Instead of mothrix's own lock,
/// which keeps a second process out, the SDK's cross-process locks take
/// turns on the store and its encryption keys. Call it before the first
/// account starts; only the first call counts.
pub fn share_stores(holder: &str) {
    let _ = SHARED_AS.set(holder.to_owned());
}

/// The sections to keep in the account, with the number of the write
/// ([`crate::sections::Written`]); for [`keep_sections`].
type SectionsToKeep = Option<(u64, Vec<Section>)>;

/// Tell the UI what the account stores of its sections: first as the
/// server has it (or, offline, the local store), then each change a sync
/// brings into the local store ([`crate::sections::Watch`] says which of
/// them are news). `waiting`: the write [`keep_sections`] has still to
/// get to the server, if any. It is given up when another device's
/// content is told: it is a picture from before that, and would go over
/// what the other device made of the sections, hours later when it waited
/// for the network.
async fn watch_sections(
    client: Client,
    waiting: tokio::sync::watch::Sender<SectionsToKeep>,
    tx: mpsc::UnboundedSender<Update>,
) {
    use matrix_sdk::ruma::events::GlobalAccountDataEventType;
    let kind = GlobalAccountDataEventType::from(crate::sections::EVENT_TYPE);
    // In one spelling, whichever way it came: the server's answer and
    // the store's copy of the same content are then the same text.
    let json = |raw: Option<Raw<matrix_sdk::ruma::events::AnyGlobalAccountDataEventContent>>| {
        raw.map(
            |raw| match serde_json::from_str::<Value>(raw.json().get()) {
                Ok(value) => value.to_string(),
                Err(_) => raw.json().get().to_owned(),
            },
        )
    };
    let in_store = || async {
        json(
            client
                .account()
                .account_data_raw(kind.clone())
                .await
                .ok()
                .flatten(),
        )
    };
    let device = client
        .device_id()
        .map(ToString::to_string)
        .unwrap_or_default();
    // The store first: what it holds now is older than the server's
    // answer, or the same.
    let mut watch = crate::sections::Watch::new(&device, in_store().await);
    let tell = |told: crate::sections::Told| {
        if told.ends_waiting_write() {
            waiting.send_if_modified(|waiting| waiting.take().is_some());
        }
        let _ = tx.send(Update::Sections(told));
    };
    match client.account().fetch_account_data(kind.clone()).await {
        Ok(raw) => tell(watch.fetched(json(raw))),
        Err(_) => {
            if let Some(said) = watch.not_fetched() {
                tell(said);
            }
        }
    }
    let mut tick = tokio::time::interval(Duration::from_secs(4));
    // While the account has not spoken (no answer and an empty store),
    // the server is asked again, further and further apart.
    let (mut ticks, mut ask_at) = (0u32, 2u32);
    loop {
        tick.tick().await;
        if tx.is_closed() {
            break;
        }
        if let Some(told) = watch.look(in_store().await) {
            tell(told);
        }
        if watch.has_told() {
            continue;
        }
        ticks += 1;
        if ticks >= ask_at {
            ticks = 0;
            ask_at = (ask_at * 2).min(75);
            if let Ok(raw) = client.account().fetch_account_data(kind.clone()).await {
                tell(watch.fetched(json(raw)));
            }
        }
    }
}

/// How long to wait before a write of the sections that failed `tries`
/// times is tried once more: further apart, and then every quarter of an
/// hour for as long as mothrix runs. Giving up would leave the change on
/// this device alone, with nothing to send it again.
fn sections_retry_after(tries: usize) -> Duration {
    const WAITS: [u64; 5] = [5, 15, 60, 300, 900];
    Duration::from_secs(WAITS[tries.min(WAITS.len() - 1)])
}

/// After how many failed tries the user is told (once per change).
const SECTIONS_TRIES_SAID: usize = 5;

/// Write the account's sections into its account data, one write at a
/// time and always the newest asked for: two changes in quick succession
/// cannot reach the server the wrong way round, and one that did not get
/// there (no network) is tried again, further and further apart, until it
/// is there, a newer one takes its place, or it is given up
/// ([`watch_sections`] empties `wanted`).
async fn keep_sections(
    client: Client,
    mut wanted: tokio::sync::watch::Receiver<SectionsToKeep>,
    tx: mpsc::UnboundedSender<Update>,
) {
    while wanted.changed().await.is_ok() {
        let mut tries = 0;
        loop {
            let Some((write, sections)) = wanted.borrow_and_update().clone() else {
                break;
            };
            let Err(error) = store_sections(&client, sections, write).await else {
                break;
            };
            tries += 1;
            if tries == SECTIONS_TRIES_SAID {
                let _ = tx.send(Update::Notice(format!(
                    "Your sections are not kept in the account yet (mothrix goes on trying): \
                     {error:#}"
                )));
            }
            // Again after a while, or at once with something newer.
            tokio::select! {
                () = tokio::time::sleep(sections_retry_after(tries - 1)) => {}
                changed = wanted.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    tries = 0;
                }
            }
        }
    }
}

/// Keep `sections` (this account's share) in the account data, as this
/// device's write number `write`.
async fn store_sections(client: &Client, sections: Vec<Section>, write: u64) -> Result<()> {
    use matrix_sdk::ruma::events::GlobalAccountDataEventType;
    let by = client.device_id().map(|device| crate::sections::Written {
        device: device.to_string(),
        write,
    });
    let content = serde_json::value::to_raw_value(&crate::sections::Stored { sections, by })?;
    client
        .account()
        .set_account_data_raw(
            GlobalAccountDataEventType::from(crate::sections::EVENT_TYPE),
            Raw::from_json(content),
        )
        .await?;
    Ok(())
}

/// The profiles this process holds, each with its open, locked file.
static HELD: std::sync::Mutex<Vec<(PathBuf, std::fs::File)>> = std::sync::Mutex::new(Vec::new());

/// The lock's file in a profile's folder.
const LOCK: &str = "lock";

/// Delete what a profile keeps on this computer: session, store, log.
/// Its lock file stays (empty, in an otherwise empty folder): the lock is
/// on the file, and with the file gone this process would go on holding
/// nothing while another one locks a new file and opens the same account.
fn forget(paths: &Paths) -> std::io::Result<()> {
    if SHARED_AS.get().is_none() {
        crate::store_key::remove(&paths.data_dir);
    }
    let entries = match std::fs::read_dir(&paths.data_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name() == LOCK {
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Let go of a profile this process no longer uses (signed out, or the
/// sign-in failed), so that another mothrix can take it.
fn release(paths: &Paths) {
    let path = paths.data_dir.join(LOCK);
    HELD.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|(known, _)| *known != path);
}

/// Whether this process holds a profile ([`hold`]).
fn holds(paths: &Paths) -> bool {
    let path = paths.data_dir.join(LOCK);
    HELD.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .any(|(known, _)| *known == path)
}

/// Keep other mothrix processes away from a profile while this one uses
/// it: two at once on one store would mix up its encryption keys. Held
/// until the process ends. Not when stores are shared ([`share_stores`]).
fn hold(paths: &Paths) -> Result<()> {
    if SHARED_AS.get().is_some() {
        return Ok(());
    }
    let mut held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = paths.data_dir.join(LOCK);
    if held.iter().any(|(known, _)| *known == path) {
        return Ok(());
    }
    crate::folders::private_dir(&paths.data_dir)?;
    let file = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {
            held.push((path, file));
            Ok(())
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            bail!("another mothrix is running with this account; close it first")
        }
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

async fn build_client(paths: &Paths, homeserver: &str, passphrase: Option<&str>) -> Result<Client> {
    let store = paths.store_dir();
    let builder = Client::builder()
        .with_encryption_settings(EncryptionSettings {
            auto_enable_cross_signing: true,
            auto_enable_backups: true,
            // Fetch room keys from the server-side backup lazily, when a
            // message fails to decrypt (what Element X does).
            backup_download_strategy: BackupDownloadStrategy::AfterDecryptionFailure,
        })
        // In-thread replies stay out of the room's own conversation and
        // are read in a thread view.
        .with_threading_support(matrix_sdk::ThreadingSupport::Enabled {
            with_subscriptions: false,
        })
        .sqlite_store(&store, passphrase);
    let builder = match SHARED_AS.get() {
        Some(holder) => builder.cross_process_store_config(
            matrix_sdk::cross_process_lock::CrossProcessLockConfig::multi_process(holder.clone()),
        ),
        None => builder,
    };
    // Networks without a direct way out (school labs) have a proxy.
    let builder =
        builder.http_client(crate::config::Config::read(&paths.config_file)?.matrix_http_client()?);

    let homeserver = homeserver.trim();
    let builder = if homeserver.starts_with("http://") || homeserver.starts_with("https://") {
        builder.homeserver_url(homeserver)
    } else {
        builder.server_name_or_homeserver_url(homeserver)
    };

    builder
        .build()
        .await
        .with_context(|| format!("could not reach homeserver '{homeserver}'"))
}

/// With shared stores ([`share_stores`]): take turns on the encryption
/// store with the other process. Only once the client has a session: the
/// encryption machine does not exist before.
async fn share_encryption_store(client: &Client) -> Result<()> {
    if let Some(holder) = SHARED_AS.get() {
        client
            .encryption()
            .enable_cross_process_store_lock(holder.clone())
            .await
            .context("could not share the encryption store")?;
    }
    Ok(())
}

fn save_session(paths: &Paths, client: &Client, store_key: Option<String>) -> Result<()> {
    let session = client
        .matrix_auth()
        .session()
        .context("login produced no session")?;
    let stored = StoredSession {
        homeserver: client.homeserver().to_string(),
        session,
        store_key,
    };
    crate::folders::private_dir(&paths.data_dir)?;
    // The file holds the access token: owner-only.
    crate::folders::write_private(&paths.session_file(), &serde_json::to_vec_pretty(&stored)?)?;
    Ok(())
}

/// Restore the saved session, if there is one.
pub async fn restore(paths: &Paths) -> Result<Option<Client>> {
    let file = paths.session_file();
    if !file.exists() {
        return Ok(None);
    }
    let stored: StoredSession = serde_json::from_slice(&std::fs::read(&file)?)
        .with_context(|| format!("the session file ({}) cannot be read", file.display()))?;
    let held_before = holds(paths);
    hold(paths)?;
    let restored = restore_held(paths, stored).await;
    // An account that did not start is not in use here: another mothrix
    // (one that reaches the keyring, say) may take it.
    if restored.is_err() && !held_before {
        release(paths);
    }
    restored.map(Some)
}

/// [`restore`], with the profile's lock held.
async fn restore_held(paths: &Paths, stored: StoredSession) -> Result<Client> {
    let passphrase = match stored.store_key.as_deref() {
        Some(crate::store_key::IN_KEYRING) => {
            let data_dir = paths.data_dir.clone();
            // Asking the keyring waits for another program.
            Some(tokio::task::spawn_blocking(move || crate::store_key::fetch(&data_dir)).await??)
        }
        Some(crate::store_key::FROM_APP) => Some(
            crate::store_key::given()
                .context(
                    "this account's store is locked with the app's passphrase, which the \
                     Keychain did not give out",
                )?
                .to_owned(),
        ),
        Some(other) => {
            bail!("this account's store is locked in a way this mothrix does not know ({other})")
        }
        None => None,
    };
    let client = build_client(paths, &stored.homeserver, passphrase.as_deref()).await?;
    client
        .restore_session(stored.session)
        .await
        .context("could not restore the saved session")?;
    share_encryption_store(&client).await?;
    Ok(client)
}

/// Sign a running account out and delete what it kept on this computer.
pub async fn sign_out(client: Client, paths: Paths) {
    // Not for as long as an unreachable server lets us wait: quitting
    // meanwhile would leave the account on the disk, back at the next
    // start.
    let _ = tokio::time::timeout(SIGN_OUT_WAIT, client.matrix_auth().logout()).await;
    // The client first: it has the store's files open.
    drop(client);
    let _ = forget(&paths);
    release(&paths);
}

/// Sign the device out on the server and remove the saved session and
/// local store. Not while another mothrix runs with the account: that one
/// would go on with files that are gone, still signed in.
///
/// The files go whatever the server says. When the device could not be
/// signed out there, `Some` says so, for the user: the device stays in
/// the account's list, and a phone's pusher goes on sending it
/// notifications, until its session is removed from another device.
pub async fn logout(paths: &Paths) -> Result<Option<String>> {
    if !paths.data_dir.exists() {
        return Ok(None);
    }
    hold(paths)?;
    let stored = std::fs::read(paths.session_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StoredSession>(&bytes).ok());
    let left = match stored {
        Some(stored) => sign_out_on_server(paths, stored).await.err(),
        // A session file that cannot be read has no token to go by.
        None if has_session(paths) => Some(anyhow!("its session file cannot be read")),
        None => None,
    };
    let forgotten = forget(paths);
    release(paths);
    forgotten?;
    Ok(left.map(|why| {
        format!(
            "The device is still signed in on the server ({why:#}): remove its session from \
             another device of the account."
        )
    }))
}

/// How long a sign-out waits for the server: quitting meanwhile would
/// leave the account on the disk, back at the next start.
const SIGN_OUT_WAIT: Duration = Duration::from_secs(15);

/// Sign the device of a saved session out on its server: through the SDK
/// where the store still opens, else with the session file's access token
/// alone. An account is mostly removed this way because its store does
/// not open (a passphrase the keyring or the Keychain lost, a damaged
/// file), and its device and pusher would else stay on the server.
async fn sign_out_on_server(paths: &Paths, stored: StoredSession) -> Result<()> {
    let homeserver = stored.homeserver.clone();
    let token = stored.session.tokens.access_token.clone();
    match restore_held(paths, stored).await {
        Ok(client) => {
            // (The client goes with this arm: it has the store's files
            // open, which are deleted next.)
            match tokio::time::timeout(SIGN_OUT_WAIT, client.matrix_auth().logout()).await {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(error)) => Err(anyhow!("the server did not take the sign-out: {error}")),
                Err(_) => bail!("the server did not answer"),
            }
        }
        Err(_) => {
            let client = crate::config::Config::http_client_at(&paths.config_file);
            let request = logout_request(&client, &homeserver, &token)?;
            // Without the request's address in the words: the user knows
            // the server, and nothing of a request belongs in a notice.
            let response = client.execute(request).await.map_err(|error| {
                anyhow!("the server could not be reached: {}", error.without_url())
            })?;
            let status = response.status().as_u16();
            logout_answer(status, &response.text().await.unwrap_or_default())
        }
    }
}

/// The request that signs a device out with its access token alone
/// (`POST /_matrix/client/v3/logout`; the server removes the device's
/// pushers with it), for the homeserver of a session file. With a time
/// limit, and for `client` to send: the one that goes through the
/// proxies ([`crate::config::Config::http_client_at`]). The token is in
/// its header only, never in the address, which errors and logs name.
fn logout_request(
    client: &matrix_sdk::reqwest::Client,
    homeserver: &str,
    access_token: &str,
) -> Result<matrix_sdk::reqwest::Request> {
    let homeserver = homeserver.trim().trim_end_matches('/');
    if !(homeserver.starts_with("https://") || homeserver.starts_with("http://")) {
        bail!("the session file names no homeserver address");
    }
    Ok(client
        .post(format!("{homeserver}/_matrix/client/v3/logout"))
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(SIGN_OUT_WAIT)
        .build()?)
}

/// What the server's answer to [`logout_request`] means. A token it no
/// longer knows is a device that is signed out already; one that only
/// ran out (`soft_logout`) is still a device of the account.
fn logout_answer(status: u16, body: &str) -> Result<()> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let body: Value = serde_json::from_str(body).unwrap_or_default();
    let gone = status == 401
        && body["errcode"] == "M_UNKNOWN_TOKEN"
        && body["soft_logout"].as_bool() != Some(true);
    if gone {
        return Ok(());
    }
    bail!("the server answered {status}")
}

// ---------------------------------------------------------------------------
// Login (plain terminal prompts, before the TUI starts)
// ---------------------------------------------------------------------------

/// Hand a file or web address to the desktop's default application.
fn open_with_desktop(target: &std::ffi::OsStr) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        // Not `cmd /C start`: the shell would read `&`, `|` and `%NAME%`
        // in an address as its own, and a link in a message could then run
        // a program. This hands the target to the desktop as one string.
        let mut command = std::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        std::process::Command::new("xdg-open")
    };
    command
        .arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

fn prompt(label: &str) -> Result<String> {
    use std::io::Write;
    print!("{label}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

/// The server part of a Matrix ID, or the input itself when it is not one.
pub fn server_from_input(input: &str) -> &str {
    let input = input.trim();
    match input.strip_prefix('@') {
        Some(rest) => rest
            .split_once(':')
            .map(|(_, server)| server)
            .unwrap_or(rest),
        None => input,
    }
}

pub struct LoginOptions {
    /// Homeserver name/URL or a full Matrix ID. Prompted for when `None`.
    pub server: Option<String>,
    pub user: Option<String>,
    /// Force password login even when the server offers browser login.
    pub password_login: bool,
}

pub async fn login(paths: &Paths, options: LoginOptions) -> Result<Client> {
    let input = match options.server {
        Some(server) => server,
        None => prompt("Homeserver or Matrix ID (e.g. matrix.org or @you:matrix.org): ")?,
    };
    let default_user = input.starts_with('@').then(|| input.clone());
    let user = options.user.or(default_user);
    sign_in(
        paths,
        &input,
        options.password_login,
        move || {
            let user = match user {
                Some(user) => user,
                None => prompt("Username or Matrix ID: ")?,
            };
            let password = match std::env::var("MOTHRIX_PASSWORD") {
                Ok(password) if !password.is_empty() => password,
                _ => rpassword::prompt_password("Password: ")?,
            };
            Ok((user, password))
        },
        // A line may hold the server's words (its address for signing in).
        |line| println!("{}", crate::message::for_terminal(&line)),
    )
    .await
}

/// Sign in to `server` (a homeserver name or URL, or a full Matrix ID) and
/// save the session under `paths`. Through the browser when the server
/// offers that, unless `password_login` asks for the password instead;
/// `credentials` is only called for a password sign-in. `say` gets the
/// lines the user should see, like the address to open.
pub async fn sign_in(
    paths: &Paths,
    server: &str,
    password_login: bool,
    credentials: impl FnOnce() -> Result<(String, String)>,
    say: impl Fn(String) + Clone + Send + Sync + 'static,
) -> Result<Client> {
    if server.trim().is_empty() {
        bail!("no homeserver given");
    }
    refuse_over_an_account(paths)?;
    // Before anything is touched: another mothrix may be signing in here.
    let held_before = holds(paths);
    hold(paths)?;
    // Again, now that nobody else can be at it.
    let signed_in = match refuse_over_an_account(paths) {
        Ok(()) => sign_in_held(paths, server, password_login, credentials, say).await,
        Err(error) => {
            if !held_before {
                release(paths);
            }
            return Err(error);
        }
    };
    if signed_in.is_err() {
        // Nothing of a sign-in that failed stays, and the profile is free.
        let _ = forget(paths);
        release(paths);
    }
    signed_in
}

/// A sign-in starts with an empty folder and leaves one when it fails:
/// where there is a session file there is an account, whose store must
/// not go that way. Also when the file cannot be read: it says nothing
/// of whether the keys beside it are still wanted.
fn refuse_over_an_account(paths: &Paths) -> Result<()> {
    if has_session(paths) {
        bail!(
            "there is already an account in {} (if its session file is damaged, remove \
             the account first: `{}`)",
            paths.data_dir.display(),
            logout_command(paths)
        );
    }
    Ok(())
}

/// What to type to remove the account of these paths: its profile is
/// the folder's name, and only "default" goes without saying.
fn logout_command(paths: &Paths) -> String {
    match paths.data_dir.file_name().and_then(|name| name.to_str()) {
        Some(profile) if profile != "default" => format!("mothrix logout --profile {profile}"),
        _ => "mothrix logout".to_owned(),
    }
}

async fn sign_in_held(
    paths: &Paths,
    server: &str,
    password_login: bool,
    credentials: impl FnOnce() -> Result<(String, String)>,
    say: impl Fn(String) + Clone + Send + Sync + 'static,
) -> Result<Client> {
    // A previous half-finished login would leave a crypto store belonging to
    // another device behind; start clean.
    if paths.store_dir().exists() {
        std::fs::remove_dir_all(paths.store_dir())?;
    }
    // The store's passphrase: the one the app hands over (the iPhone's,
    // from its Keychain, where its extensions read it too), else one made
    // here and put into the system keyring, where there is one.
    let from_app = crate::store_key::given().map(str::to_owned);
    let passphrase = if from_app.is_some() {
        from_app.clone()
    } else if SHARED_AS.get().is_some() {
        // The iPhone app, whose Keychain gave no passphrase (it is
        // locked away, or could not be written): said, as it is said
        // of a desktop without a keyring.
        tracing::warn!("no passphrase from the app for the store");
        say(
            "The Keychain gave no passphrase: this account's encryption keys are kept \
             without one, protected by the phone's own file protection only."
                .to_owned(),
        );
        None
    } else {
        let data_dir = paths.data_dir.clone();
        match tokio::task::spawn_blocking(move || crate::store_key::create(&data_dir)).await? {
            Ok(passphrase) => Some(passphrase),
            Err(error) => {
                tracing::warn!(%error, "no keyring for the store's passphrase");
                say(
                    "No system keyring could be reached: this account's encryption keys are \
                     kept without a passphrase, protected by the file permissions only."
                        .to_owned(),
                );
                None
            }
        }
    };
    let client = build_client(paths, server_from_input(server), passphrase.as_deref()).await?;
    let flows = client
        .matrix_auth()
        .get_login_types()
        .await
        .context("the homeserver did not answer the login query")?
        .flows;
    let has_sso = flows.iter().any(|flow| matches!(flow, LoginType::Sso(_)));
    let has_password = flows
        .iter()
        .any(|flow| matches!(flow, LoginType::Password(_)));

    if has_sso && !(password_login && has_password) {
        say("This server signs you in through your browser (single sign-on).".to_owned());
        client
            .matrix_auth()
            .login_sso(move |url| async move {
                say(format!(
                    "Open this address in a browser on this computer and sign in: {url}"
                ));
                // Best effort; the address shown is the fallback.
                let _ = open_with_desktop(url.as_ref());
                say(format!(
                    "Waiting for the browser sign-in to finish… If no page opened: {url}"
                ));
                Ok(())
            })
            .initial_device_display_name("mothrix")
            .await
            .context("browser sign-in failed")?;
    } else if has_password {
        let (user, password) = credentials()?;
        client
            .matrix_auth()
            .login_username(&user, &password)
            .initial_device_display_name("mothrix")
            .await
            .context("password sign-in failed")?;
    } else {
        bail!(
            "this server offers neither browser (SSO) nor password login; \
             it probably requires the newer OAuth login, which mothrix does not support yet"
        );
    }

    let store_key = passphrase.map(|_| {
        if from_app.is_some() {
            crate::store_key::FROM_APP.to_owned()
        } else {
            crate::store_key::IN_KEYRING.to_owned()
        }
    });
    // The server has a new device now. If it cannot be kept here (a full
    // disk, the store), it is signed out again: nothing on this computer
    // would know it, and it would stay in the account's list for good.
    let kept = match save_session(paths, &client, store_key) {
        Ok(()) => share_encryption_store(&client).await,
        Err(error) => Err(error),
    };
    if let Err(error) = kept {
        let _ = tokio::time::timeout(Duration::from_secs(15), client.matrix_auth().logout()).await;
        return Err(error);
    }
    // Signing in kicks off key uploads and cross-signing setup in the
    // background. Let them finish, so that quitting right after `mothrix login`
    // does not leave the new device half set up.
    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;
    Ok(client)
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Replace every string, number and list item in `value` by its type, but
/// keep the structure (keys) and a handful of fields that describe the
/// event's format rather than its text. Lets people share what a room's
/// events look like without sharing what was said.
pub fn shape_of(value: &Value) -> Value {
    const KEEP: [&str; 8] = [
        "msgtype",
        "rel_type",
        "kind",
        "max_selections",
        "membership",
        "format",
        "algorithm",
        "mimetype",
    ];
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, inner)| {
                    let kept =
                        KEEP.contains(&key.as_str()) && !inner.is_object() && !inner.is_array();
                    (
                        key.clone(),
                        if kept { inner.clone() } else { shape_of(inner) },
                    )
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().take(3).map(shape_of).collect()),
        Value::String(_) => Value::String("<text>".to_owned()),
        Value::Number(_) => Value::String("<number>".to_owned()),
        other => other.clone(),
    }
}

/// Print the most recent events of a room: type, sender, and the shape of
/// the content (no message text).
pub async fn print_events(client: &Client, room_name: Option<&str>, limit: u32) -> Result<()> {
    let wanted = room_name.map(str::to_lowercase);
    let mut chosen = None;
    for room in client.joined_rooms() {
        let name = room
            .display_name()
            .await
            .map(|name| name.to_string())
            .unwrap_or_default();
        let matches = match &wanted {
            Some(wanted) => {
                name.to_lowercase().contains(wanted) || room.room_id().as_str() == wanted
            }
            None => true,
        };
        if matches {
            chosen = Some((room, name));
            break;
        }
    }
    let (room, name) = chosen.context(
        "no such room (start mothrix once so it knows your rooms, then pass part of the room's name)",
    )?;
    // The name, and below the types and senders, are other people's
    // text: nothing of it is to be obeyed by the terminal.
    println!(
        "# {}  ({limit} events, newest first; text is masked)",
        crate::message::for_terminal(&name)
    );

    let mut options = matrix_sdk::room::MessagesOptions::backward();
    options.limit = UInt::from(limit);
    let messages = room
        .messages(options)
        .await
        .context("could not read the room's history")?;
    for event in messages.chunk {
        let Ok(value) = serde_json::from_str::<Value>(event.raw().json().get()) else {
            continue;
        };
        let sender = value["sender"].as_str().unwrap_or("?");
        let line = format!(
            "{}  from {}\n    {}",
            value["type"].as_str().unwrap_or("?"),
            convert::localpart(sender),
            shape_of(&value["content"]),
        );
        println!("{}", crate::message::for_terminal(&line));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Background task
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum SyncStatus {
    Connecting,
    Online,
    Offline,
    Error(String),
}

#[derive(Debug)]
pub enum Update {
    /// Our own display name.
    OwnName(String),
    /// A picture from the clipboard, saved to this file, ready to send.
    PastedImage(String),
    /// Text from the clipboard.
    PastedText(String),
    Rooms(Vec<RoomEntry>),
    Timeline {
        room_id: String,
        items: Vec<Item>,
    },
    /// The messages of a thread of the open room, first message included.
    Thread {
        room_id: String,
        root: String,
        items: Vec<Item>,
    },
    /// The thread (or the pinned messages) asked for could not be opened:
    /// the room's conversation is followed again.
    ThreadFailed {
        room_id: String,
        root: String,
    },
    /// What the homeserver found behind a web address; empty when it
    /// found nothing.
    LinkPreview {
        url: String,
        title: String,
        description: String,
    },
    /// A new recovery key, to show once so that it can be written down.
    RecoveryKey(String),
    /// The answer to [`Command::RoomDetails`].
    RoomDetails(RoomDetails),
    /// Show this room (one just made or joined) as soon as it is listed.
    GoTo(String),
    /// Who is writing in the open room right now (display names).
    Typing {
        room_id: String,
        names: Vec<String>,
    },
    /// Joined members of the open room, for @-mention completion.
    Members {
        room_id: String,
        members: Vec<Member>,
    },
    /// A downloaded picture, ready to show under its message.
    Preview {
        event_id: String,
        preview: Preview,
    },
    Sync(SyncStatus),
    /// Results for the search that asked for `query`.
    SearchResults {
        query: String,
        hits: Vec<SearchHit>,
    },
    /// The message asked for with [`Command::Reveal`] is now loaded.
    Reveal(String),
    /// What the account stores of the user's own sections
    /// ([`crate::sections`]).
    Sections(crate::sections::Told),
    /// An attachment asked for with [`Command::OpenMedia`] is on disk, for
    /// the app to show ([`hand_media_over`]).
    MediaFile {
        event_id: String,
        path: String,
    },
    /// A message ([`Command::Send`]) was refused, since the conversation
    /// it was written in is not the one followed (any more): its text,
    /// for the front end to give back to the user.
    NotSent {
        room_id: String,
        thread: Option<String>,
        text: String,
    },
    /// An attachment asked for with [`Command::OpenMedia`] is on disk and
    /// was not opened: it is of a kind mothrix opens by nothing (a web
    /// page, a program), or not the sound its name says. `name`: what
    /// its sender called it. The terminal says where it lies; an app,
    /// whose folders its user does not get to, offers to pass it on.
    MediaKept {
        event_id: String,
        path: String,
        name: String,
    },
    /// Progress of a device verification; `None` closes the dialog.
    Verify(Option<VerifyState>),
    /// Whether this device is verified (trusted by our other devices).
    Verified(bool),
    /// One-line message for the status bar.
    Notice(String),
    /// A message read for [`Command::Forward`], for `Accounts` to hand to
    /// the account of `to` (a room key) as [`Command::SendForwarded`].
    ToForward {
        to: String,
        content: String,
        encrypted_file: bool,
    },
}

impl Command {
    /// The room a command is about, where it names one: its key in the
    /// UI, its id once `Accounts::send` has routed it.
    pub fn room_id_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Open(room_id)
            | Self::Send { room_id, .. }
            | Self::SendFile { room_id, .. }
            | Self::SendVoice { room_id, .. }
            | Self::PasteImage { room_id, .. }
            | Self::CreatePoll { room_id, .. }
            | Self::SetFavourite { room_id, .. }
            | Self::SetLowPriority { room_id, .. }
            | Self::SetNotify { room_id, .. }
            | Self::RoomDetails { room_id }
            | Self::Invite { room_id, .. }
            | Self::Kick { room_id, .. }
            | Self::Ban { room_id, .. }
            | Self::SetRoomName { room_id, .. }
            | Self::SetTopic { room_id, .. }
            | Self::SendForwarded { room_id, .. }
            | Self::Leave { room_id }
            | Self::PreviewLink { room_id, .. }
            | Self::MarkRead { room_id } => Some(room_id),
            Self::Search { room_id, .. } => room_id.as_mut(),
            // About the open room, an account as a whole, or the desktop.
            // (Forward: to the open room's account; its room stays a key.)
            Self::Close
            | Self::Forward { .. }
            | Self::Typing(_)
            | Self::OpenThread { .. }
            | Self::CloseThread
            | Self::Reply { .. }
            | Self::Edit { .. }
            | Self::ClipboardPaste
            | Self::Reveal { .. }
            | Self::VerifyStart
            | Self::VerifyUser { .. }
            | Self::Verify(_)
            | Self::Delete { .. }
            | Self::React { .. }
            | Self::Paginate
            | Self::Vote { .. }
            | Self::EndPoll { .. }
            | Self::TogglePin { .. }
            | Self::StopAudio
            | Self::OpenLink(_)
            | Self::OpenMedia { .. }
            | Self::StartDm { .. }
            | Self::CreateRoom { .. }
            | Self::Join { .. }
            | Self::Recover(_)
            | Self::SetSections { .. }
            | Self::SetPusher(_)
            | Self::RemovePusher { .. }
            | Self::Backup { .. }
            | Self::ResetIdentity { .. } => None,
        }
    }
}

impl Update {
    /// Every room id the update carries, for `Accounts::deliver` to turn
    /// into keys.
    pub fn room_ids_mut(&mut self) -> Vec<&mut String> {
        match self {
            Self::Rooms(rooms) => rooms.iter_mut().map(|room| &mut room.id).collect(),
            Self::Timeline { room_id, .. }
            | Self::Thread { room_id, .. }
            | Self::ThreadFailed { room_id, .. }
            | Self::GoTo(room_id)
            | Self::NotSent { room_id, .. }
            | Self::Typing { room_id, .. }
            | Self::Members { room_id, .. } => vec![room_id],
            Self::RoomDetails(details) => vec![&mut details.room_id],
            Self::SearchResults { hits, .. } => {
                hits.iter_mut().map(|hit| &mut hit.room_id).collect()
            }
            Self::OwnName(_)
            | Self::PastedImage(_)
            | Self::PastedText(_)
            | Self::LinkPreview { .. }
            | Self::RecoveryKey(_)
            | Self::Preview { .. }
            | Self::Sync(_)
            | Self::Reveal(_)
            | Self::MediaFile { .. }
            | Self::MediaKept { .. }
            | Self::Sections(_)
            | Self::Verify(_)
            | Self::Verified(_)
            | Self::Notice(_)
            // A key already.
            | Self::ToForward { .. } => Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum Command {
    Open(String),
    /// Stop following the open room (another account's room is shown).
    Close,
    /// Tell the open room that we are writing, or stopped.
    Typing(bool),
    /// Show the thread that starts at this message of the open room.
    OpenThread {
        event_id: String,
    },
    /// Back from the thread to the room's conversation.
    CloseThread,
    /// Text is sent as Markdown; `mentions` are the completed @-mentions.
    ///
    /// Like everything that writes something new into the conversation
    /// (this, [`Command::SendFile`], [`Command::SendVoice`],
    /// [`Command::CreatePoll`], [`Command::PasteImage`]), it names the
    /// room it was written in and the `thread` there (its first message;
    /// none for the room's own conversation), and is refused when that
    /// is not the conversation followed ([`written_into`]): what the
    /// front end shows and what the backend follows differ for a moment
    /// after every change, and for good after a thread failed to open.
    /// Of a refused message the text comes back ([`Update::NotSent`]).
    Send {
        room_id: String,
        thread: Option<String>,
        text: String,
        mentions: Vec<Member>,
    },
    Reply {
        event_id: String,
        text: String,
        mentions: Vec<Member>,
    },
    Edit {
        event_id: String,
        text: String,
        mentions: Vec<Member>,
    },
    /// Upload a local file into the conversation named.
    SendFile {
        room_id: String,
        thread: Option<String>,
        path: String,
        /// Text to show with it.
        caption: Option<String>,
    },
    /// Upload a recorded voice message into the conversation named
    /// (MSC3245): its length and loudness over time (values 0 to 1).
    SendVoice {
        room_id: String,
        thread: Option<String>,
        path: String,
        duration_ms: u64,
        waveform: Vec<f32>,
    },
    /// Send the picture that is on the system clipboard into the
    /// conversation named.
    PasteImage {
        room_id: String,
        thread: Option<String>,
    },
    /// Ctrl+V: hand the clipboard's picture (as a file) or text to the UI.
    ClipboardPaste,
    /// Search messages in one room (by id) or, with `None`, everywhere.
    Search {
        query: String,
        room_id: Option<String>,
    },
    /// Load history of the open room until this message is on hand.
    Reveal {
        event_id: String,
    },
    /// Ask our other devices to verify this one.
    VerifyStart,
    /// Ask another person to verify with us.
    VerifyUser {
        user_id: String,
    },
    Verify(VerifyInput),
    Delete {
        event_id: String,
    },
    /// Add the reaction, or take it back if we already gave it.
    React {
        event_id: String,
        key: String,
    },
    /// Load older messages of the open room.
    Paginate,
    /// A new poll, in the conversation named.
    CreatePoll {
        room_id: String,
        thread: Option<String>,
        question: String,
        answers: Vec<String>,
        multiple: bool,
        hidden: bool,
    },
    /// `answers` is the complete new selection.
    Vote {
        poll_event_id: String,
        answers: Vec<String>,
        format: PollFormat,
    },
    EndPoll {
        poll_event_id: String,
        format: PollFormat,
    },
    /// Pin a message of the open room for everyone, or take the pin off.
    TogglePin {
        event_id: String,
    },
    /// Send a copy of a message of the open room into another room, of
    /// this account or another one. `Accounts` sends it to the account
    /// whose room is open, with `room_id` left a key: that account reads
    /// the message and answers with [`Update::ToForward`].
    Forward {
        event_id: String,
        room_id: String,
    },
    /// Send what an account read for [`Command::Forward`]: `content` is the
    /// message's type as JSON; an encrypted file is not sent into an
    /// unencrypted room.
    SendForwarded {
        room_id: String,
        content: String,
        encrypted_file: bool,
    },
    /// Ask the homeserver what is behind a web address (title, summary)
    /// that stands in a message of this room. The room says which
    /// account's homeserver is asked, and lets the backend refuse for a
    /// room it does not show or that is encrypted.
    PreviewLink {
        room_id: String,
        url: String,
    },
    /// Stop the audio that is playing.
    StopAudio,
    /// Hand a web address from a message to the browser.
    OpenLink(String),
    /// Download an image or file and hand it to the desktop's default app.
    OpenMedia {
        event_id: String,
    },
    SetFavourite {
        room_id: String,
        value: bool,
    },
    SetLowPriority {
        room_id: String,
        value: bool,
    },
    SetNotify {
        room_id: String,
        mode: Notify,
    },
    /// Ask for what the room info dialog shows.
    RoomDetails {
        room_id: String,
    },
    Invite {
        room_id: String,
        user_id: String,
    },
    Kick {
        room_id: String,
        user_id: String,
    },
    Ban {
        room_id: String,
        user_id: String,
    },
    SetRoomName {
        room_id: String,
        name: String,
    },
    SetTopic {
        room_id: String,
        topic: String,
    },
    Leave {
        room_id: String,
    },
    /// Read up to the newest message of a room that is not open (a swipe
    /// in the list, a notification's action).
    MarkRead {
        room_id: String,
    },
    /// Start (or find) the direct chat with someone.
    StartDm {
        user_id: String,
    },
    /// Make a new private room.
    CreateRoom {
        name: String,
    },
    /// Join a room by its address or id.
    Join {
        target: String,
    },
    Recover(String),
    /// Store the user's own sections (this account's share) in its account
    /// data ([`crate::sections`]), as this device's write of this number
    /// ([`crate::sections::Sync::to_send`] makes it).
    SetSections {
        write: u64,
        sections: Vec<Section>,
    },
    /// Have the homeserver send push notifications for this account to a
    /// phone ([`crate::push`]); or stop it.
    SetPusher(crate::push::Target),
    RemovePusher {
        app_id: String,
        pushkey: String,
    },
    /// Say how key backup stands; set it up if there is none. With
    /// `renew`, make a new recovery key (the old one stops working).
    Backup {
        renew: bool,
    },
    /// Throw away our cross-signing identity and make a new one. Asks
    /// the server with the account password.
    ResetIdentity {
        password: String,
    },
}

pub struct Backend {
    pub updates: mpsc::UnboundedReceiver<Update>,
    pub commands: mpsc::UnboundedSender<Command>,
    pub user_id: String,
}

pub fn spawn(client: Client) -> Backend {
    let (update_tx, updates) = mpsc::unbounded_channel();
    let (commands, command_rx) = mpsc::unbounded_channel();
    let user_id = client
        .user_id()
        .map(ToString::to_string)
        .unwrap_or_default();
    tokio::spawn(async move {
        let failures = update_tx.clone();
        if let Err(error) = run(client, update_tx, command_rx).await {
            tracing::error!(%error, "backend stopped");
            let _ = failures.send(Update::Sync(SyncStatus::Error(format!("{error:#}"))));
        }
    });
    Backend {
        updates,
        commands,
        user_id,
    }
}

/// A task that ends when this is dropped.
struct EndsWith(JoinHandle<()>);

impl Drop for EndsWith {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct OpenRoom {
    room_id: String,
    /// The thread followed (its first message, or [`PINNED_VIEW`]); none
    /// for the room's own conversation.
    thread: Option<String>,
    room: Room,
    timeline: Arc<Timeline>,
    task: JoinHandle<()>,
    /// Tell the timeline task about something it cannot see itself.
    nudge: mpsc::UnboundedSender<Nudge>,
}

/// News for the timeline task of the open room.
enum Nudge {
    /// A poll got a new answer or was ended (its event id).
    PollChanged(String),
    /// Something shown changed outside the timeline (a pin): show it anew.
    Republish,
}

async fn room_entry(room: &Room) -> RoomEntry {
    let name = match room.cached_display_name() {
        Some(name) => name.to_string(),
        None => room
            .display_name()
            .await
            .map(|name| name.to_string())
            .unwrap_or_default(),
    };
    let server = room.unread_notification_counts();
    let (unread, mentions) = unread_counts(
        (room.num_unread_notifications(), room.num_unread_mentions()),
        (server.notification_count, server.highlight_count),
        room.is_marked_unread(),
    );
    RoomEntry {
        account: 0,
        id: room.room_id().to_string(),
        name: if name.is_empty() {
            room.room_id().to_string()
        } else {
            name
        },
        topic: room.topic().unwrap_or_default(),
        is_dm: room.is_direct().await.unwrap_or(false),
        is_space: room.is_space(),
        space: String::new(),
        is_invite: room.state() == RoomState::Invited,
        notify: Notify::Default,
        favourite: room.is_favourite(),
        low_priority: room.is_low_priority(),
        unread,
        mentions,
        marked_unread: room.is_marked_unread()
            && room.num_unread_notifications() == 0
            && server.notification_count == 0,
    }
}

async fn run(
    client: Client,
    tx: mpsc::UnboundedSender<Update>,
    mut rx: mpsc::UnboundedReceiver<Command>,
) -> Result<()> {
    let _ = tx.send(Update::Sync(SyncStatus::Connecting));

    // The status bar shows our name rather than the id.
    {
        let (client, tx) = (client.clone(), tx.clone());
        tokio::spawn(async move {
            if let Ok(Some(name)) = client.account().get_display_name().await
                && !name.trim().is_empty()
            {
                let _ = tx.send(Update::OwnName(name));
            }
        });
    }

    // The user's own sections, as the account stores them: at the start
    // from the server, then whenever a sync brings a change.
    // (Ended with this backend: it holds a sender of the updates, and
    // whoever forwards them waits for the last sender to go, so a
    // signed-out account would else keep both tasks for good.)
    let (sections, sections_to_keep) = tokio::sync::watch::channel(None);
    let _watching = EndsWith(tokio::spawn(watch_sections(
        client.clone(),
        sections.clone(),
        tx.clone(),
    )));

    let sync = SyncService::builder(client.clone())
        .with_offline_mode()
        .build()
        .await
        .context("could not set up sync (the server may lack sliding sync support)")?;
    let room_list_service = sync.room_list_service();
    let room_list = room_list_service.all_rooms().await?;
    let (entries, controller) = room_list.entries_with_dynamic_adapters(10_000);
    controller.set_filter(Box::new(new_filter_non_left()));
    let mut entries = Box::pin(entries);
    let mut sync_state = sync.state();
    sync.start().await;

    let own_user_id: Option<OwnedUserId> = client.user_id().map(ToOwned::to_owned);
    let notifications = client.notification_settings().await;
    let mut rooms = Vector::new();
    // Unread counters can change without the list itself changing order.
    let mut refresh = tokio::time::interval(Duration::from_secs(4));

    // Verification requests from our other devices arrive as to-device
    // events; hand them to this loop, which runs one verification at a time.
    let (incoming_tx, mut incoming) = mpsc::unbounded_channel::<VerificationRequest>();
    let incoming_tx_rooms = incoming_tx.clone();
    client.add_event_handler(
        move |event: ToDeviceKeyVerificationRequestEvent, client: Client| {
            let incoming_tx = incoming_tx.clone();
            async move {
                let request = client
                    .encryption()
                    .get_verification_request(&event.sender, &event.content.transaction_id)
                    .await;
                if let Some(request) = request {
                    let _ = incoming_tx.send(request);
                }
            }
        },
    );
    // Other people ask in the direct chat with us, as a message.
    {
        let incoming_tx = incoming_tx_rooms;
        client.add_event_handler(move |event: OriginalSyncRoomMessageEvent, client: Client| {
            let incoming_tx = incoming_tx.clone();
            async move {
                if !matches!(event.content.msgtype, MessageType::VerificationRequest(_))
                    || Some(event.sender.as_ref()) == client.user_id()
                {
                    return;
                }
                // Requests run out after ten minutes; older ones in the
                // history are of no use.
                let age = MilliSecondsSinceUnixEpoch::now()
                    .get()
                    .saturating_sub(event.origin_server_ts.get());
                if u64::from(age) > 10 * 60 * 1000 {
                    return;
                }
                // The crypto machine takes the request in while it
                // handles the same sync; give it a moment.
                for _ in 0..10 {
                    let request = client
                        .encryption()
                        .get_verification_request(&event.sender, &event.event_id)
                        .await;
                    if let Some(request) = request {
                        let _ = incoming_tx.send(request);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
        });
    }
    let mut verified: Option<bool> = None;

    // Responses to standard-format polls are not aggregated by the SDK;
    // note which poll changed so the open timeline can re-read it.
    let (poll_tx, mut poll_events) = mpsc::unbounded_channel::<(String, String)>();
    // Listen to raw events: the SDK cannot parse the responses some
    // clients send, so a typed handler would never see them.
    client.add_event_handler(move |raw: Raw<AnySyncMessageLikeEvent>, room: Room| {
        let poll_tx = poll_tx.clone();
        async move {
            let Ok(event) = serde_json::from_str::<Value>(raw.json().get()) else {
                return;
            };
            let about_a_poll = event["type"]
                .as_str()
                .is_some_and(stable_polls::is_poll_update);
            if let (true, Some(poll_id)) = (
                about_a_poll,
                event["content"]["m.relates_to"]["event_id"].as_str(),
            ) {
                let _ = poll_tx.send((room.room_id().to_string(), poll_id.to_owned()));
            }
        }
    });

    let (account, account_commands) = mpsc::unbounded_channel();
    tokio::spawn(account_lane(client.clone(), account_commands, tx.clone()));
    let (beside, beside_events) = mpsc::unbounded_channel();
    tokio::spawn(beside_lane(beside_events, tx.clone()));
    let _keeping = EndsWith(tokio::spawn(keep_sections(
        client.clone(),
        sections_to_keep,
        tx.clone(),
    )));
    let mut session = Session {
        client: client.clone(),
        sync,
        own_user_id,
        open: None,
        verifying: None,
        tx: tx.clone(),
        account,
        beside,
        sections,
    };

    let mut gate = EmptyListGate::default();
    loop {
        tokio::select! {
            diffs = entries.next() => {
                let Some(diffs) = diffs else { bail!("room list stream ended") };
                for diff in diffs {
                    diff.apply(&mut rooms);
                }
                if gate.show(rooms.is_empty(), Instant::now()) {
                    publish_rooms(&rooms, &notifications, &tx).await;
                }
            }
            _ = refresh.tick() => {
                if gate.show(rooms.is_empty(), Instant::now()) {
                    publish_rooms(&rooms, &notifications, &tx).await;
                }
                let now = match client.encryption().get_own_device().await {
                    // Signed by our own identity, i.e. trusted by our other
                    // devices. (`is_verified` is always true for the device
                    // we are running on.)
                    Ok(Some(device)) => Some(device.is_cross_signed_by_owner()),
                    _ => None,
                };
                if now.is_some() && now != verified {
                    verified = now;
                    let _ = tx.send(Update::Verified(now == Some(true)));
                }
            }
            Some((room_id, poll_id)) = poll_events.recv() => {
                if let Some(current) = session.open.as_ref().filter(|current| current.room_id == room_id) {
                    let _ = current.nudge.send(Nudge::PollChanged(poll_id));
                }
            }
            request = incoming.recv() => {
                // A request nobody answered yet makes way for a newer one
                // (what is left over from an earlier run comes first).
                let busy = session
                    .verifying
                    .as_ref()
                    .is_some_and(|running| !running.is_closed() && !running.is_unanswered());
                // From another of our devices, or from another person.
                if let Some(request) = request
                    && !busy
                {
                    session.verifying = Some(spawn_verification(&client, request, true, &tx));
                }
            }
            state = sync_state.next() => {
                let status = match state {
                    Some(SyncState::Running) => SyncStatus::Online,
                    Some(SyncState::Offline) => SyncStatus::Offline,
                    Some(SyncState::Error(error)) => SyncStatus::Error(error.to_string()),
                    Some(SyncState::Idle | SyncState::Terminated) => SyncStatus::Connecting,
                    None => bail!("sync state stream ended"),
                };
                let _ = tx.send(Update::Sync(status));
            }
            command = rx.recv() => {
                // The UI is gone: shut down.
                let Some(command) = command else { break };
                if let Err(error) = session.handle(command).await {
                    let _ = tx.send(Update::Notice(format!("{error:#}")));
                }
            }
        }
    }

    if let Some(open) = session.open {
        open.task.abort();
    }
    session.sync.stop().await;
    Ok(())
}

/// A local file made ready to upload: its name, type, and the details
/// other clients want (a picture's size and small version).
struct Attachment {
    path: PathBuf,
    name: String,
    mime: mime::Mime,
    config: AttachmentConfig,
}

impl Attachment {
    async fn prepare(path: &str, caption: Option<String>) -> Result<Self> {
        let path = crate::folders::expand_home(path);
        let metadata =
            std::fs::metadata(&path).with_context(|| format!("cannot read {}", path.display()))?;
        if !metadata.is_file() {
            bail!("{} is not a file", path.display());
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mime = mime_guess::from_path(&path).first_or_octet_stream();
        let mut config = AttachmentConfig {
            caption: caption.map(TextMessageEventContent::markdown),
            ..AttachmentConfig::default()
        };
        // Pictures go with their size and a small version, so that other
        // clients can lay them out and preview them.
        if mime.type_() == mime::IMAGE {
            let file = path.clone();
            let size = metadata.len();
            if let Ok(Some((info, thumbnail))) =
                tokio::task::spawn_blocking(move || picture_details(&file, size)).await
            {
                config.info = Some(AttachmentInfo::Image(info));
                config.thumbnail = thumbnail;
            }
        }
        Ok(Self {
            path,
            name,
            mime,
            config,
        })
    }

    /// Send it as a voice message: an audio clip marked as voice, with its
    /// length and waveform.
    fn make_voice(&mut self, duration_ms: u64, waveform: Vec<f32>) {
        // AAC in an MP4 box (".m4a"), as players know it.
        if self.mime.type_() != mime::AUDIO || self.mime.subtype() == "m4a" {
            self.mime = "audio/mp4".parse().expect("a valid type");
        }
        let size = std::fs::metadata(&self.path).ok().map(|m| m.len());
        self.config.info = Some(AttachmentInfo::Voice(BaseAudioInfo {
            duration: Some(Duration::from_millis(duration_ms)),
            size: size.and_then(UInt::new),
            waveform: Some(
                waveform
                    .into_iter()
                    .map(|level| level.clamp(0.0, 1.0))
                    .collect(),
            ),
        }));
    }
}

/// For a short-lived process (the share extension, a notification's
/// action) that has a signed-in client but no sync running: send `text`
/// into a room.
pub async fn send_text(
    client: &Client,
    room_id: &str,
    text: &str,
    thread: Option<&str>,
) -> Result<()> {
    use matrix_sdk::ruma::events::room::message::Relation;
    let room = room_of(client, room_id)?;
    let mut content = MessageParts::new(text, &[]).into_content();
    // Into the thread the message answered is in (its root standing in
    // for the latest message there, which only the timeline would know).
    if let Some(root) = thread {
        let root = EventId::parse(root)?;
        content.relates_to = Some(Relation::Thread(
            matrix_sdk::ruma::events::relation::Thread::plain(root.clone(), root),
        ));
    }
    room.send(content)
        .await
        .context("could not send the message")?;
    Ok(())
}

/// Like [`send_text`], for files: each uploaded, the caption with the
/// first.
pub async fn send_files(
    client: &Client,
    room_id: &str,
    paths: &[String],
    mut caption: Option<String>,
) -> Result<()> {
    let room = room_of(client, room_id)?;
    for path in paths {
        let file = Attachment::prepare(path, caption.take()).await?;
        let data = tokio::fs::read(&file.path).await?;
        let config = matrix_sdk::attachment::AttachmentConfig {
            info: file.config.info,
            thumbnail: file.config.thumbnail,
            caption: file.config.caption,
            ..Default::default()
        };
        room.send_attachment(file.name.clone(), &file.mime, data, config)
            .await
            .with_context(|| format!("could not send {}", file.name))?;
    }
    Ok(())
}

/// Read a room up to its newest message, without opening it.
pub async fn read_newest(room: &Room) {
    let mut options = matrix_sdk::room::MessagesOptions::backward();
    options.limit = 1u32.into();
    if let Ok(page) = room.messages(options).await
        && let Some(newest) = page.chunk.first().and_then(|event| event.event_id())
    {
        let _ = room
            .send_single_receipt(
                ReceiptType::Read,
                ReceiptThread::Unthreaded,
                newest.to_owned(),
            )
            .await;
    }
    if room.is_marked_unread() {
        let _ = room.set_unread_flag(false).await;
    }
}

/// A room's unread messages and mentions, from the SDK's own counts and
/// the server's (each: messages, mentions). The SDK counts from the events
/// it saw, and loses them when a room it does not follow got several at
/// once (a gap); the server cannot see mentions inside encrypted messages.
/// The larger of the two is right in both cases. A room marked unread
/// counts at least one.
fn unread_counts(sdk: (u64, u64), server: (u64, u64), marked: bool) -> (u64, u64) {
    let unread = sdk.0.max(server.0).max(u64::from(marked));
    (unread, sdk.1.max(server.1))
}

/// A room's notification setting from the user's own rule for it (`None`:
/// none, the account's default holds).
fn notify_of(mode: Option<RoomNotificationMode>) -> Notify {
    match mode {
        Some(RoomNotificationMode::AllMessages) => Notify::All,
        Some(RoomNotificationMode::MentionsAndKeywordsOnly) => Notify::Mentions,
        Some(RoomNotificationMode::Mute) => Notify::Mute,
        None => Notify::Default,
    }
}

/// The rule [`notify_of`] reads back as `notify`; `None`: the room's own
/// rules go.
fn mode_of(notify: Notify) -> Option<RoomNotificationMode> {
    match notify {
        Notify::Default => None,
        Notify::All => Some(RoomNotificationMode::AllMessages),
        Notify::Mentions => Some(RoomNotificationMode::MentionsAndKeywordsOnly),
        Notify::Mute => Some(RoomNotificationMode::Mute),
    }
}

/// Whether a message's attachment is encrypted: its key travels in the
/// message.
fn holds_encrypted_file(msgtype: &MessageType) -> bool {
    // Wherever it stands: the file itself, its thumbnail, or in a kind of
    // message this build has no name for.
    fn has_key(value: &Value) -> bool {
        match value {
            Value::Object(fields) => {
                (fields.contains_key("key") && fields.contains_key("iv"))
                    || fields.values().any(has_key)
            }
            Value::Array(items) => items.iter().any(has_key),
            _ => false,
        }
    }
    // What cannot be looked at is not passed on as harmless.
    serde_json::to_value(msgtype).map_or(true, |content| has_key(&content))
}

fn room_of(client: &Client, room_id: &str) -> Result<Room> {
    client
        .get_room(&RoomId::parse(room_id)?)
        .ok_or_else(|| anyhow!("unknown room"))
}

/// A Matrix id as typed: `@name:server`, with the `@` optional.
fn parse_user(typed: &str) -> Result<OwnedUserId> {
    let typed = typed.trim();
    let full = if typed.starts_with('@') {
        typed.to_owned()
    } else {
        format!("@{typed}")
    };
    UserId::parse(&full).map_err(|_| anyhow!("'{typed}' is not a Matrix ID (@name:server)"))
}

/// Send fresh room details once the server had a moment to apply a change.
fn refresh_details(room: Room, tx: &mpsc::UnboundedSender<Update>) {
    let tx = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        // The local member list does not always learn of the change (one
        // removed while invited stayed listed): ask the server again.
        room.mark_members_missing();
        let _ = tx.send(Update::RoomDetails(room_details(&room).await));
    });
}

async fn room_details(room: &Room) -> RoomDetails {
    let shown = RoomMemberships::JOIN | RoomMemberships::INVITE;
    // From the server when it answers, else what is known locally.
    let listed = match room.members(shown).await {
        Ok(members) => members,
        Err(_) => room.members_no_sync(shown).await.unwrap_or_default(),
    };
    let own = listed
        .iter()
        .find(|member| member.user_id() == room.own_user_id());
    let rank = |role: &str| match role {
        "admin" => 0,
        "moderator" => 1,
        _ => 2,
    };
    let mut members: Vec<RoomMember> = listed
        .iter()
        .map(|member| RoomMember {
            user_id: member.user_id().to_string(),
            name: member.name().to_owned(),
            role: match member.suggested_role_for_power_level() {
                RoomMemberRole::Creator | RoomMemberRole::Administrator => "admin",
                RoomMemberRole::Moderator => "moderator",
                RoomMemberRole::User => "",
            }
            .to_owned(),
            invited: *member.membership() == MembershipState::Invite,
        })
        .collect();
    members.sort_by_key(|member| {
        (
            member.invited,
            rank(&member.role),
            member.name.to_lowercase(),
        )
    });
    RoomDetails {
        room_id: room.room_id().to_string(),
        name: room
            .cached_display_name()
            .map(|name| name.to_string())
            .unwrap_or_default(),
        topic: room.topic().unwrap_or_default(),
        alias: room
            .canonical_alias()
            .map(|alias| alias.to_string())
            .unwrap_or_default(),
        encrypted: room.encryption_state().is_encrypted(),
        members,
        can_invite: own.is_some_and(|own| own.can_invite()),
        can_kick: own.is_some_and(|own| own.can_kick()),
        can_ban: own.is_some_and(|own| own.can_ban()),
        can_rename: own.is_some_and(|own| own.can_send_state(StateEventType::RoomName)),
        can_set_topic: own.is_some_and(|own| own.can_send_state(StateEventType::RoomTopic)),
    }
}

/// The rooms a space lists as its children (by room id).
async fn space_children(space: &Room) -> Vec<String> {
    let Ok(events) = space
        .get_state_events_static::<SpaceChildEventContent>()
        .await
    else {
        return Vec::new();
    };
    events
        .into_iter()
        .filter_map(|raw| match raw.deserialize().ok()? {
            // An entry without servers to join through was taken back.
            SyncOrStrippedState::Sync(SyncStateEvent::Original(event))
                if !event.content.via.is_empty() =>
            {
                Some(event.state_key.to_string())
            }
            _ => None,
        })
        .collect()
}

/// Section names for the rooms that are in a space: the space's name, or
/// `Space/Subspace` one level down. `spaces` is (id, name, children).
pub fn space_sections(spaces: &[(String, String, Vec<String>)]) -> HashMap<String, String> {
    // A slash would read as a subsection.
    let clean = |name: &str| name.replace('/', "-").trim().to_owned();
    let parent_of = |id: &str| {
        spaces
            .iter()
            .find(|(_, _, children)| children.iter().any(|child| child == id))
    };
    let mut sections = HashMap::new();
    for (id, name, children) in spaces {
        let section = match parent_of(id) {
            Some((_, parent, _)) => format!("{}/{}", clean(parent), clean(name)),
            None => clean(name),
        };
        for child in children {
            // A room in a subspace is listed there, not again in the
            // space above.
            let deeper = sections
                .get(child)
                .is_some_and(|known: &String| known.contains('/'));
            if !deeper {
                sections.insert(child.clone(), section.clone());
            }
        }
    }
    sections
}

/// The SDK's room list can be empty for a few seconds while it starts
/// over (seen soon after starting). Showing that empties the sidebar of
/// an account; so once rooms were shown, an empty list counts only after
/// it stayed empty for a while.
#[derive(Default)]
struct EmptyListGate {
    shown_rooms: bool,
    empty_since: Option<Instant>,
}

const EMPTY_LIST_GRACE: Duration = Duration::from_secs(10);

impl EmptyListGate {
    /// Whether to show the list as it is now.
    fn show(&mut self, empty: bool, now: Instant) -> bool {
        if !empty {
            self.shown_rooms = true;
            self.empty_since = None;
            return true;
        }
        if !self.shown_rooms {
            return true;
        }
        let since = *self.empty_since.get_or_insert(now);
        now.duration_since(since) >= EMPTY_LIST_GRACE
    }
}

async fn publish_rooms<R>(
    rooms: &Vector<R>,
    notifications: &NotificationSettings,
    tx: &mpsc::UnboundedSender<Update>,
) where
    R: std::ops::Deref<Target = Room> + Clone,
{
    let mut out = Vec::with_capacity(rooms.len());
    let mut spaces = Vec::new();
    for room in rooms.iter() {
        let mut entry = room_entry(room).await;
        entry.notify = notify_of(
            notifications
                .get_user_defined_room_notification_mode(room.room_id())
                .await,
        );
        if entry.is_space {
            spaces.push((
                entry.id.clone(),
                entry.name.clone(),
                space_children(room).await,
            ));
        }
        out.push(entry);
    }
    let sections = space_sections(&spaces);
    for entry in &mut out {
        if !entry.is_space
            && let Some(section) = sections.get(&entry.id)
        {
            entry.space = section.clone();
        }
    }
    let _ = tx.send(Update::Rooms(out));
}

/// What the backend of one account keeps while it runs.
struct Session {
    client: Client,
    sync: SyncService,
    own_user_id: Option<OwnedUserId>,
    /// The conversation followed: a room, and its timeline, a thread's or
    /// its pinned messages'.
    open: Option<OpenRoom>,
    verifying: Option<Verifying>,
    tx: mpsc::UnboundedSender<Update>,
    /// Commands for [`account_lane`].
    account: mpsc::UnboundedSender<Command>,
    /// Events for [`beside_lane`].
    beside: mpsc::UnboundedSender<Beside>,
    /// The sections to keep in the account, for [`keep_sections`].
    sections: tokio::sync::watch::Sender<SectionsToKeep>,
}

impl Session {
    /// Carry out a command, or pass it on. Three kinds, by what they wait
    /// for:
    ///
    /// - What uses or changes the conversation followed (open, send,
    ///   react, …) is done here, in the order it was asked for. Messages
    ///   go through the SDK's send queue, so this does not wait for the
    ///   server, except to open a room (and to accept its invitation).
    /// - What changes something on the server for a room by its id or for
    ///   the account (invite, rename, join, key backup, …) goes to
    ///   [`account_lane`]: in order among themselves, and however long the
    ///   server takes, the room list and the conversation go on.
    /// - What only looks something up (search, room details, a link's
    ///   preview, a file, older messages) gets a task of its own.
    ///
    /// (Events the timeline's queue cannot send, a vote in a format the
    /// SDK has no type for, leave through [`beside_lane`].)
    ///
    /// What writes something new into the conversation says which one it
    /// was written for, and is refused when another is followed
    /// ([`written_into`]).
    ///
    /// An error becomes a notice, whichever way.
    async fn handle(&mut self, command: Command) -> Result<()> {
        let Self {
            client,
            sync,
            own_user_id,
            open,
            verifying,
            tx,
            account,
            beside,
            sections: sections_to_keep,
        } = self;
        let (client, sync, tx) = (&*client, &*sync, &*tx);
        let own_user_id = own_user_id.as_deref();
        match command {
            Command::Close => {
                if let Some(previous) = open.take() {
                    previous.task.abort();
                }
            }
            Command::Open(room_id) => {
                if open
                    .as_ref()
                    .is_some_and(|current| current.room_id == room_id)
                {
                    return Ok(());
                }
                if let Some(previous) = open.take() {
                    previous.task.abort();
                }
                let parsed = RoomId::parse(&room_id)?;
                let room = client
                    .get_room(&parsed)
                    .ok_or_else(|| anyhow!("unknown room"))?;
                if room.state() == RoomState::Invited {
                    room.join()
                        .await
                        .context("could not accept the invitation")?;
                }
                // Ask the server for this room's full timeline, not just the
                // latest event the room list needs.
                sync.room_list_service()
                    .set_room_subscriptions(&[&parsed])
                    .await;
                *open = Some(open_timeline(room, View::Live, own_user_id, tx).await?);
            }
            Command::Typing(typing) => {
                if let Some(current) = open.as_ref() {
                    // Beside the loop: a server that is slow to take the
                    // notice must not hold up the message sent after it.
                    // Not worth a message in the status bar if it fails.
                    let room = current.room.clone();
                    tokio::spawn(async move {
                        let _ = room.typing_notice(typing).await;
                    });
                }
            }
            Command::OpenThread { event_id } => {
                let room_id = open.as_ref().map(|current| current.room_id.clone());
                let room_id = room_id.ok_or_else(|| anyhow!("open a room first"))?;
                let view = if event_id == PINNED_VIEW {
                    View::Pinned
                } else {
                    // Before the room is let go of: with an id that is
                    // none, the room stays followed, and the UI is told
                    // to go back to it.
                    match EventId::parse(&event_id) {
                        Ok(root) => View::Thread(root),
                        Err(error) => {
                            let _ = tx.send(Update::ThreadFailed {
                                room_id,
                                root: event_id,
                            });
                            return Err(anyhow!(error).context("could not open the thread"));
                        }
                    }
                };
                let Some(current) = open.take() else {
                    return Ok(());
                };
                current.task.abort();
                let room = current.room.clone();
                *open = Some(match open_timeline(room, view, own_user_id, tx).await {
                    Ok(thread) => thread,
                    // Back to the room rather than to nothing.
                    Err(error) => {
                        let _ = tx.send(Update::ThreadFailed {
                            room_id: current.room_id.clone(),
                            root: event_id,
                        });
                        *open =
                            Some(open_timeline(current.room, View::Live, own_user_id, tx).await?);
                        return Err(error.context("could not open the thread"));
                    }
                });
            }
            Command::CloseThread => {
                if let Some(current) = open.take() {
                    current.task.abort();
                    *open = Some(open_timeline(current.room, View::Live, own_user_id, tx).await?);
                }
            }
            Command::Send {
                room_id,
                thread,
                text,
                mentions,
            } => {
                // Refused, the text goes back to whoever wrote it.
                let Ok(timeline) = written_into(open, &room_id, &thread) else {
                    let _ = tx.send(Update::NotSent {
                        room_id,
                        thread,
                        text,
                    });
                    return Ok(());
                };
                let message = MessageParts::new(&text, &mentions);
                timeline
                    .send(message.into_content().into())
                    .await
                    .context("could not send the message")?;
            }
            Command::SendFile {
                room_id,
                thread,
                path,
                caption,
            } => {
                let timeline = written_into(open, &room_id, &thread)?;
                let file = Attachment::prepare(&path, caption).await?;
                let _ = tx.send(Update::Notice(format!("Sending {}…", file.name)));
                timeline
                    .send_attachment(AttachmentSource::File(file.path), file.mime, file.config)
                    .use_send_queue()
                    .await
                    .with_context(|| format!("could not send {}", file.name))?;
            }
            Command::SendVoice {
                room_id,
                thread,
                path,
                duration_ms,
                waveform,
            } => {
                let timeline = written_into(open, &room_id, &thread)?;
                let mut file = Attachment::prepare(&path, None).await?;
                file.make_voice(duration_ms, waveform);
                timeline
                    .send_attachment(AttachmentSource::File(file.path), file.mime, file.config)
                    .use_send_queue()
                    .await
                    .context("could not send the voice message")?;
            }
            Command::Search { query, room_id } => {
                let client = client.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let room = room_id.as_deref().and_then(|id| RoomId::parse(id).ok());
                    let hits = crate::search::search(&client, &query, room.as_deref()).await;
                    let _ = tx.send(Update::SearchResults { query, hits });
                });
            }
            Command::Reveal { event_id } => {
                let timeline = Arc::clone(timeline_of(open)?);
                let tx = tx.clone();
                tokio::spawn(async move {
                    let Ok(id) = EventId::parse(&event_id) else {
                        return;
                    };
                    // Page back through history until the message turns up.
                    for _ in 0..60 {
                        if timeline.item_by_event_id(&id).await.is_some() {
                            let _ = tx.send(Update::Reveal(event_id));
                            return;
                        }
                        match timeline.paginate_backwards(100).await {
                            Ok(false) => {}
                            Ok(true) | Err(_) => break,
                        }
                    }
                    let update = if timeline.item_by_event_id(&id).await.is_some() {
                        Update::Reveal(event_id)
                    } else {
                        Update::Notice("That message could not be loaded here.".to_owned())
                    };
                    let _ = tx.send(update);
                });
            }
            Command::PasteImage { room_id, thread } => {
                // Beside the loop, like the two below: a clipboard tool
                // waits for the program that owns the clipboard, which
                // may never answer, and a verification's start waits for
                // the server. The picture goes into the conversation that
                // was followed when it was asked for.
                let timeline = Arc::clone(written_into(open, &room_id, &thread)?);
                let tx = tx.clone();
                tokio::spawn(async move {
                    let pasted = async {
                        let (bytes, mime_type) = from_clipboard(read_clipboard_image).await?;
                        let extension = mime_type.rsplit('/').next().unwrap_or("png");
                        let filename = format!("pasted-{}.{extension}", seconds_now());
                        let mime: mime::Mime = mime_type.parse().unwrap_or(mime::IMAGE_PNG);
                        timeline
                            .send_attachment(
                                AttachmentSource::Data { bytes, filename },
                                mime,
                                AttachmentConfig::default(),
                            )
                            .use_send_queue()
                            .await
                            .context("could not send the picture")?;
                        anyhow::Ok(())
                    };
                    if let Err(error) = pasted.await {
                        let _ = tx.send(Update::Notice(format!("{error:#}")));
                    }
                });
            }
            Command::ClipboardPaste => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let pasted = async {
                        let clip = from_clipboard(|| match read_clipboard_image() {
                            Ok(image) => Ok(Ok(image)),
                            Err(no_image) => read_clipboard_text().map(Err).ok_or(no_image),
                        })
                        .await?;
                        Ok(match clip {
                            Ok((bytes, mime_type)) => {
                                let extension = mime_type.rsplit('/').next().unwrap_or("png");
                                let dir = crate::folders::cache()
                                    .context("no cache directory on this system")?
                                    .join("pasted");
                                crate::folders::private_dir(&dir)?;
                                let file =
                                    dir.join(format!("pasted-{}.{extension}", seconds_now()));
                                crate::folders::write_private(&file, &bytes)?;
                                Update::PastedImage(file.display().to_string())
                            }
                            Err(text) => Update::PastedText(text),
                        })
                    };
                    let update = pasted.await.unwrap_or_else(|error: anyhow::Error| {
                        Update::Notice(format!("{error:#}"))
                    });
                    let _ = tx.send(update);
                });
            }
            Command::VerifyStart => {
                if verifying.as_ref().is_some_and(|inputs| !inputs.is_closed()) {
                    bail!("a verification is already running");
                }
                let asking = client.clone();
                *verifying = Some(start_verification(client, tx, async move {
                    verify::request(&asking).await
                }));
            }
            Command::VerifyUser { user_id } => {
                if verifying.as_ref().is_some_and(|inputs| !inputs.is_closed()) {
                    bail!("a verification is already running");
                }
                let user = parse_user(&user_id)?;
                let asking = client.clone();
                *verifying = Some(start_verification(client, tx, async move {
                    verify::request_user(&asking, &user).await
                }));
            }
            Command::Verify(input) => {
                let delivered = verifying
                    .as_ref()
                    .is_some_and(|inputs| inputs.send(input).is_ok());
                // Nothing is running any more (finished or failed): this just
                // dismisses the dialog.
                if !delivered {
                    *verifying = None;
                    let _ = tx.send(Update::Verify(None));
                }
            }
            Command::Paginate => {
                if let Some(current) = open.as_ref() {
                    let timeline = Arc::clone(&current.timeline);
                    tokio::spawn(async move {
                        let _ = timeline.paginate_backwards(40).await;
                    });
                }
            }
            Command::Vote {
                poll_event_id,
                answers,
                format,
            } => {
                let timeline = timeline_of(open)?;
                // Answer in the format the poll was asked in.
                match format {
                    PollFormat::Unstable => {
                        let content = UnstablePollResponseEventContent::new(
                            answers,
                            EventId::parse(&poll_event_id)?,
                        );
                        timeline
                            .send(AnyMessageLikeEventContent::UnstablePollResponse(content))
                            .await
                            .context("could not send the vote")?;
                    }
                    // These go out as raw JSON: the SDK's types cannot express
                    // the layouts other clients use.
                    PollFormat::Standard | PollFormat::Mixed => {
                        let (event_type, content) =
                            stable_polls::response_event(format, &poll_event_id, &answers);
                        let room = timeline.room().clone();
                        let _ = beside.send((room, event_type.into(), content, "the vote"));
                    }
                }
            }
            Command::Reply {
                event_id,
                text,
                mentions,
            } => {
                let content = MessageParts::new(&text, &mentions).into_content_without_relation();
                timeline_of(open)?
                    .send_reply(content, EventId::parse(&event_id)?)
                    .await
                    .context("could not send the reply")?;
            }
            Command::Edit {
                event_id,
                text,
                mentions,
            } => {
                let content = MessageParts::new(&text, &mentions).into_content_without_relation();
                timeline_of(open)?
                    .edit(&item_id(&event_id)?, EditedContent::RoomMessage(content))
                    .await
                    .context("could not edit the message")?;
            }
            Command::Delete { event_id } => {
                timeline_of(open)?
                    .redact(&item_id(&event_id)?, None)
                    .await
                    .context("could not delete the message")?;
            }
            Command::React { event_id, key } => {
                timeline_of(open)?
                    .toggle_reaction(&item_id(&event_id)?, &key)
                    .await
                    .context("could not send the reaction")?;
            }
            Command::CreatePoll {
                room_id,
                thread,
                question,
                answers,
                multiple,
                hidden,
            } => {
                let content = build_poll_start(&question, &answers, multiple, hidden)?;
                written_into(open, &room_id, &thread)?
                    .send(AnyMessageLikeEventContent::UnstablePollStart(
                        UnstablePollStartEventContent::New(content),
                    ))
                    .await
                    .context("could not create the poll")?;
            }
            Command::EndPoll {
                poll_event_id,
                format,
            } => {
                let timeline = timeline_of(open)?;
                let text = "The poll has ended.";
                match format {
                    PollFormat::Unstable => {
                        let content =
                            UnstablePollEndEventContent::new(text, EventId::parse(&poll_event_id)?);
                        timeline
                            .send(AnyMessageLikeEventContent::UnstablePollEnd(content))
                            .await
                            .context("could not end the poll")?;
                    }
                    PollFormat::Standard | PollFormat::Mixed => {
                        let (event_type, content) =
                            stable_polls::end_event(format, &poll_event_id, text);
                        let room = timeline.room().clone();
                        let _ = beside.send((room, event_type.into(), content, "the poll's end"));
                    }
                }
            }
            Command::TogglePin { event_id } => {
                let current = open.as_ref().ok_or_else(|| anyhow!("open a room first"))?;
                let id = EventId::parse(&event_id)?;
                let pinned = current
                    .room
                    .pinned_event_ids()
                    .is_some_and(|ids| ids.contains(&id));
                let (room, tx, nudge) = (current.room.clone(), tx.clone(), current.nudge.clone());
                // On its own, with a time limit: a server that never answers
                // (seen with IServ) must not hold up the account's other commands.
                tokio::spawn(async move {
                    let change = async {
                        if pinned {
                            room.unpin_event(&id).await
                        } else {
                            room.pin_event(&id).await
                        }
                    };
                    let notice = match tokio::time::timeout(Duration::from_secs(20), change).await {
                        Ok(Ok(_)) if pinned => "Unpinned.".to_owned(),
                        Ok(Ok(_)) => "Pinned for everyone in the room.".to_owned(),
                        Ok(Err(error)) => {
                            format!(
                                "Could not change the pin (you may not be allowed to here): {error}"
                            )
                        }
                        Err(_) => "The server did not answer the pin in 20 seconds.".to_owned(),
                    };
                    let _ = tx.send(Update::Notice(notice));
                    // Show it once the server has confirmed.
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    let _ = nudge.send(Nudge::Republish);
                });
            }
            Command::Forward { event_id, room_id } => {
                let timeline = timeline_of(open)?;
                let item = timeline
                    .item_by_event_id(&EventId::parse(&event_id)?)
                    .await
                    .ok_or_else(|| anyhow!("that message is no longer loaded"))?;
                let message = item
                    .content()
                    .as_message()
                    .ok_or_else(|| anyhow!("only messages and files can be forwarded"))?;
                // The same content (for files: the same upload, unless the
                // copy goes out through another homeserver), without the
                // reply or thread it belonged to, and saying who wrote it.
                let msgtype = message.msgtype();
                let from = match item.sender_profile() {
                    TimelineDetails::Ready(profile) => profile.display_name.clone(),
                    _ => None,
                }
                .unwrap_or_else(|| item.sender().to_string());
                let content = crate::forward::attributed(serde_json::to_value(msgtype)?, &from);
                let _ = tx.send(Update::ToForward {
                    to: room_id,
                    content: content.to_string(),
                    encrypted_file: holds_encrypted_file(msgtype),
                });
            }
            Command::PreviewLink { room_id, url } => {
                let shown = open.as_ref().map(|current| {
                    let plain = matches!(
                        current.room.encryption_state(),
                        matrix_sdk::EncryptionState::NotEncrypted
                    );
                    (current.room_id.as_str(), plain)
                });
                if !previews_links(shown, &room_id) {
                    return Ok(());
                }
                let client = client.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let data = client
                        .media()
                        .get_media_preview(&url, None)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|raw| serde_json::from_str::<Value>(raw.get()).ok())
                        .unwrap_or_default();
                    let text = |key: &str| {
                        data[key]
                            .as_str()
                            .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                            .unwrap_or_default()
                    };
                    let _ = tx.send(Update::LinkPreview {
                        title: text("og:title"),
                        description: text("og:description"),
                        url,
                    });
                });
            }
            Command::StopAudio => {
                let notice = if stop_audio() {
                    "Stopped."
                } else {
                    "Nothing is playing."
                };
                let _ = tx.send(Update::Notice(notice.to_owned()));
            }
            Command::OpenLink(address) => {
                // Only what a browser or mail program is meant to get.
                if !desktop_opens(&address) {
                    bail!("mothrix does not open this kind of address");
                }
                open_with_desktop(address.as_ref()).context("could not start the browser")?;
                let _ = tx.send(Update::Notice(format!("Opened {address}")));
            }
            Command::OpenMedia { event_id } => {
                let timeline = Arc::clone(timeline_of(open)?);
                let client = client.clone();
                let tx = tx.clone();
                // Downloads can take a while; do not hold up other commands.
                tokio::spawn(async move {
                    let update = match open_media(&client, &timeline, &event_id).await {
                        Ok((path, Opened::Handed)) => Update::MediaFile {
                            event_id,
                            path: path.display().to_string(),
                        },
                        Ok((path, Opened::Kept)) => Update::MediaKept {
                            event_id,
                            name: unmarked(&path.file_name().unwrap_or_default().to_string_lossy())
                                .to_owned(),
                            path: path.display().to_string(),
                        },
                        Ok((_, Opened::Playing)) => {
                            Update::Notice("Playing; /stop ends it.".to_owned())
                        }
                        Ok((path, Opened::Shown)) => {
                            Update::Notice(format!("Opened {}", path.display()))
                        }
                        Err(error) => Update::Notice(format!("{error:#}")),
                    };
                    let _ = tx.send(update);
                });
            }
            Command::RoomDetails { room_id } => {
                let room = room_of(client, &room_id)?;
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(Update::RoomDetails(room_details(&room).await));
                });
            }
            Command::MarkRead { room_id } => {
                let room = room_of(client, &room_id)?;
                tokio::spawn(async move { read_newest(&room).await });
            }
            Command::SetSections { write, sections } => {
                let _ = sections_to_keep.send(Some((write, sections)));
            }
            Command::Leave { room_id } => {
                if open
                    .as_ref()
                    .is_some_and(|current| current.room_id == room_id)
                    && let Some(current) = open.take()
                {
                    current.task.abort();
                }
                let _ = account.send(Command::Leave { room_id });
            }
            command @ (Command::SendForwarded { .. }
            | Command::SetFavourite { .. }
            | Command::SetLowPriority { .. }
            | Command::SetNotify { .. }
            | Command::Invite { .. }
            | Command::Kick { .. }
            | Command::Ban { .. }
            | Command::SetRoomName { .. }
            | Command::SetTopic { .. }
            | Command::StartDm { .. }
            | Command::CreateRoom { .. }
            | Command::Join { .. }
            | Command::Backup { .. }
            | Command::ResetIdentity { .. }
            | Command::SetPusher(_)
            | Command::RemovePusher { .. }
            | Command::Recover(_)) => {
                let _ = account.send(command);
            }
        }
        Ok(())
    }
}

/// Carries out, one after the other, the commands that change something
/// on the server for a room by its id or for the account
/// ([`Session::handle`] says which). A server that takes its time (or a
/// reset waiting to be approved in the browser) holds up only these.
async fn account_lane(
    client: Client,
    mut commands: mpsc::UnboundedReceiver<Command>,
    tx: mpsc::UnboundedSender<Update>,
) {
    while let Some(command) = commands.recv().await {
        if let Err(error) = account_command(&client, command, &tx).await {
            let _ = tx.send(Update::Notice(format!("{error:#}")));
        }
    }
}

async fn account_command(
    client: &Client,
    command: Command,
    tx: &mpsc::UnboundedSender<Update>,
) -> Result<()> {
    match command {
        Command::SendForwarded {
            room_id,
            content,
            encrypted_file,
        } => {
            let target = room_of(client, &room_id)?;
            if encrypted_file && !target.latest_encryption_state().await?.is_encrypted() {
                bail!(
                    "an encrypted file is not forwarded into an unencrypted room: \
                     the server could read it there"
                );
            }
            let msgtype: MessageType = serde_json::from_str(&content)?;
            target
                .send(RoomMessageEventContent::new(msgtype))
                .await
                .context("could not forward the message")?;
            let name = target
                .cached_display_name()
                .map(|name| name.to_string())
                .unwrap_or_default();
            let _ = tx.send(Update::Notice(format!("Forwarded to {name}.")));
        }
        Command::SetFavourite { room_id, value } => {
            let room = client
                .get_room(&RoomId::parse(&room_id)?)
                .ok_or_else(|| anyhow!("unknown room"))?;
            room.set_is_favourite(value, None)
                .await
                .context("could not change the pin")?;
        }
        Command::SetLowPriority { room_id, value } => {
            let room = client
                .get_room(&RoomId::parse(&room_id)?)
                .ok_or_else(|| anyhow!("unknown room"))?;
            room.set_is_low_priority(value, None)
                .await
                .context("could not change the priority")?;
        }
        Command::SetNotify { room_id, mode } => {
            let room_id = RoomId::parse(&room_id)?;
            let settings = client.notification_settings().await;
            match mode_of(mode) {
                None => settings.delete_user_defined_room_rules(&room_id).await,
                Some(mode) => settings.set_room_notification_mode(&room_id, mode).await,
            }
            .context("could not change the notifications")?;
        }
        Command::Invite { room_id, user_id } => {
            let room = room_of(client, &room_id)?;
            let user = parse_user(&user_id)?;
            room.invite_user_by_id(&user)
                .await
                .with_context(|| format!("could not invite {user}"))?;
            let _ = tx.send(Update::Notice(format!("Invited {user}.")));
            refresh_details(room, tx);
        }
        Command::Kick { room_id, user_id } => {
            let room = room_of(client, &room_id)?;
            let user = parse_user(&user_id)?;
            room.kick_user(&user, None)
                .await
                .with_context(|| format!("could not remove {user}"))?;
            let _ = tx.send(Update::Notice(format!("Removed {user} from the room.")));
            refresh_details(room, tx);
        }
        Command::Ban { room_id, user_id } => {
            let room = room_of(client, &room_id)?;
            let user = parse_user(&user_id)?;
            room.ban_user(&user, None)
                .await
                .with_context(|| format!("could not ban {user}"))?;
            let _ = tx.send(Update::Notice(format!("Banned {user}.")));
            refresh_details(room, tx);
        }
        Command::SetRoomName { room_id, name } => {
            let room = room_of(client, &room_id)?;
            room.set_name(name)
                .await
                .context("could not rename the room")?;
            refresh_details(room, tx);
        }
        Command::SetTopic { room_id, topic } => {
            let room = room_of(client, &room_id)?;
            room.set_room_topic(&topic)
                .await
                .context("could not change the topic")?;
            refresh_details(room, tx);
        }
        Command::StartDm { user_id } => {
            let user = parse_user(&user_id)?;
            let room = match client.get_dm_room(&user) {
                Some(room) => room,
                None => client
                    .create_dm(&user)
                    .await
                    .with_context(|| format!("could not start a chat with {user}"))?,
            };
            let _ = tx.send(Update::GoTo(room.room_id().to_string()));
        }
        Command::CreateRoom { name } => {
            let mut request = create_room::v3::Request::new();
            request.name = Some(name);
            request.preset = Some(create_room::v3::RoomPreset::PrivateChat);
            // Private rooms are encrypted from the start, as other clients
            // make them.
            request.initial_state = vec![
                InitialStateEvent::with_empty_state_key(
                    RoomEncryptionEventContent::with_recommended_defaults(),
                )
                .to_raw_any(),
            ];
            let room = client
                .create_room(request)
                .await
                .context("could not make the room")?;
            let _ = tx.send(Update::GoTo(room.room_id().to_string()));
        }
        Command::Join { target } => {
            let target = <&RoomOrAliasId>::try_from(target.trim())
                .map_err(|_| anyhow!("'{target}' is not a room address (#name:server) or id"))?;
            let room = client
                .join_room_by_id_or_alias(target, &[])
                .await
                .with_context(|| format!("could not join {target}"))?;
            let _ = tx.send(Update::GoTo(room.room_id().to_string()));
        }
        Command::Backup { renew } => {
            let recovery = client.encryption().recovery();
            match backup_step(recovery.state(), renew) {
                BackupStep::Say(text) => {
                    let _ = tx.send(Update::Notice(text.to_owned()));
                }
                BackupStep::NewKey => {
                    let key = recovery
                        .reset_key()
                        .await
                        .context("could not make a new recovery key")?;
                    let _ = tx.send(Update::RecoveryKey(key));
                }
                BackupStep::SetUp => {
                    let _ = tx.send(Update::Notice("Setting up key backup…".to_owned()));
                    let key = recovery
                        .enable()
                        .await
                        .context("could not set up key backup")?;
                    let _ = tx.send(Update::RecoveryKey(key));
                }
            }
        }
        Command::ResetIdentity { password } => {
            let recovery = client.encryption().recovery();
            let handle = recovery
                .reset_identity()
                .await
                .context("could not start the reset")?;
            if let Some(handle) = handle {
                match handle.auth_type() {
                    CrossSigningResetAuthType::Uiaa(info) => {
                        let user = client
                            .user_id()
                            .ok_or_else(|| anyhow!("not signed in"))?
                            .to_owned();
                        let mut auth = uiaa::Password::new(
                            uiaa::UserIdentifier::Matrix(uiaa::MatrixUserIdentifier::new(
                                user.to_string(),
                            )),
                            password,
                        );
                        auth.session = info.session.clone();
                        handle
                            .reset(Some(uiaa::AuthData::Password(auth)))
                            .await
                            .context(
                                "the server did not accept the password; key backup is off \
                                 until /reset-identity is run again and goes through",
                            )?;
                    }
                    CrossSigningResetAuthType::OAuth(info) => {
                        let url = info.approval_url.to_string();
                        // Any other address is only shown.
                        if approval_opens(&url) {
                            let _ = open_with_desktop(url.as_ref());
                        }
                        let _ = tx.send(Update::Notice(format!(
                            "Approve the reset in your browser: {url}"
                        )));
                        // Not for ever: everything else that changes
                        // the account waits behind this.
                        tokio::time::timeout(Duration::from_secs(10 * 60), handle.reset(None))
                            .await
                            .context(
                                "the reset was not approved in ten minutes; nothing was changed",
                            )?
                            .context("the reset was not approved")?;
                    }
                }
            }
            // The old backup went with the old identity.
            let key = recovery
                .enable()
                .await
                .context("identity reset, but key backup could not be set up again")?;
            let _ = tx.send(Update::Notice(
                "Identity reset. Verify your other devices again.".to_owned(),
            ));
            let _ = tx.send(Update::RecoveryKey(key));
        }
        Command::SetPusher(target) => {
            if let Err(error) = crate::push::add(client, &target).await {
                let _ = tx.send(Update::Notice(format!(
                    "Push notifications could not be set up: {error:#}"
                )));
            }
        }
        Command::RemovePusher { app_id, pushkey } => {
            if let Err(error) = crate::push::remove(client, &app_id, &pushkey).await {
                tracing::warn!(%error, "could not remove a pusher");
            }
        }
        Command::Recover(key) => {
            client
                .encryption()
                .recovery()
                .recover(key.trim())
                .await
                .context("the recovery key was not accepted")?;
            let _ = tx.send(Update::Notice(
                "Encryption recovery set up; old messages will decrypt as they load.".to_owned(),
            ));
        }
        Command::Leave { room_id } => {
            let room = room_of(client, &room_id)?;
            room.leave().await.context("could not leave the room")?;
            let _ = tx.send(Update::Notice("Left the room.".to_owned()));
        }
        // `Session::handle` keeps everything else.
        other => bail!("not a command for the account's lane: {other:?}"),
    }
    Ok(())
}

/// Whether the homeserver is asked what is behind a link of the room
/// `asked`. It makes the preview, and would learn of a link from a room
/// it cannot read otherwise: only for the room open here (`shown`: its
/// id, and whether it is known not to be encrypted), so that the answer
/// is this account's own and about a room whose state it has.
fn previews_links(shown: Option<(&str, bool)>, asked: &str) -> bool {
    shown.is_some_and(|(room_id, plain)| plain && room_id == asked)
}

/// A user id as it stands in a matrix.to address. Ids from the early
/// days may hold nearly anything: a `)` would end the Markdown link the
/// id is put into, and what follows it would be read as Markdown or HTML
/// of ours. Whatever is not usual in an id is percent-encoded.
fn in_address(user_id: &str) -> String {
    let mut out = String::with_capacity(user_id.len());
    for byte in user_id.bytes() {
        if byte.is_ascii_alphanumeric() || b"@:._=-/+".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Whether an address is one a browser or mail program is meant to get:
/// the same ones a message shows as links ([`crate::inline::site`]).
fn desktop_opens(address: &str) -> bool {
    crate::inline::site(address).is_some()
}

/// Whether the address a homeserver names for approving an identity
/// reset is opened in the browser. The server may name anything: only a
/// web page, and only an encrypted one.
fn approval_opens(address: &str) -> bool {
    address.starts_with("https://") && desktop_opens(address)
}

/// What `/backup` (`renew`: `/backup new`) does about key backup.
#[derive(Debug, PartialEq, Eq)]
enum BackupStep {
    /// Nothing; say this.
    Say(&'static str),
    /// Make a new recovery key for the backup there is.
    NewKey,
    /// Set key backup up, with its recovery key.
    SetUp,
}

fn backup_step(state: RecoveryState, renew: bool) -> BackupStep {
    match (state, renew) {
        (RecoveryState::Enabled, false) => {
            BackupStep::Say("Key backup is on. `/backup new` makes a new recovery key.")
        }
        (RecoveryState::Incomplete, false) => BackupStep::Say(
            "Key backup exists, but this device lacks its key: /recover <recovery key>, or /verify.",
        ),
        (RecoveryState::Enabled | RecoveryState::Incomplete, true) => BackupStep::NewKey,
        (RecoveryState::Disabled | RecoveryState::Unknown, _) => BackupStep::SetUp,
    }
}

/// The verification that is running.
struct Verifying {
    inputs: mpsc::UnboundedSender<VerifyInput>,
    /// An incoming request the user has not accepted yet.
    unanswered: Arc<std::sync::atomic::AtomicBool>,
}

impl Verifying {
    fn is_closed(&self) -> bool {
        self.inputs.is_closed()
    }

    fn send(&self, input: VerifyInput) -> Result<(), mpsc::error::SendError<VerifyInput>> {
        self.inputs.send(input)
    }

    fn is_unanswered(&self) -> bool {
        self.unanswered.load(std::sync::atomic::Ordering::Relaxed)
    }
}

fn spawn_verification(
    client: &Client,
    request: VerificationRequest,
    incoming: bool,
    tx: &mpsc::UnboundedSender<Update>,
) -> Verifying {
    let (inputs_tx, inputs) = mpsc::unbounded_channel();
    let unanswered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    tokio::spawn(verify::run(
        client.clone(),
        request,
        incoming,
        tx.clone(),
        inputs,
        Arc::clone(&unanswered),
    ));
    Verifying {
        inputs: inputs_tx,
        unanswered,
    }
}

/// Start a verification of our asking: `ask` sends the request (to our
/// other devices, or to another person), in the verification's own task,
/// since the server may take its time over it. Esc meanwhile ends it
/// there and then ([`unless_given_up`]).
fn start_verification(
    client: &Client,
    tx: &mpsc::UnboundedSender<Update>,
    ask: impl Future<Output = Result<VerificationRequest>> + Send + 'static,
) -> Verifying {
    let (inputs_tx, mut inputs) = mpsc::unbounded_channel();
    let unanswered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (client, tx, waits) = (client.clone(), tx.clone(), Arc::clone(&unanswered));
    let _ = tx.send(Update::Verify(Some(VerifyState::Waiting)));
    tokio::spawn(async move {
        let asking = tokio::time::timeout(Duration::from_secs(60), ask);
        let Some(asked) = unless_given_up(asking, &mut inputs).await else {
            // Without a word, but for closing the dialog should it
            // still be there.
            let _ = tx.send(Update::Verify(None));
            return;
        };
        let failed = match asked {
            Ok(Ok(request)) => {
                return verify::run(client, request, false, tx, inputs, waits).await;
            }
            Ok(Err(error)) => format!("{error:#}"),
            Err(_) => "The server did not answer the request.".to_owned(),
        };
        let _ = tx.send(Update::Verify(Some(VerifyState::Failed(failed))));
    });
    Verifying {
        inputs: inputs_tx,
        unanswered,
    }
}

/// What `asking` (a verification's request on its way to the server)
/// comes to; none when the user gave it up meanwhile (Esc), or nobody
/// waits for it any more. Left to wait for the request, the Esc was
/// taken only once that was out, up to a minute later: the dialog, long
/// closed, came back to say "Cancelled".
async fn unless_given_up<T>(
    asking: impl Future<Output = T>,
    inputs: &mut mpsc::UnboundedReceiver<VerifyInput>,
) -> Option<T> {
    tokio::pin!(asking);
    loop {
        tokio::select! {
            asked = &mut asking => return Some(asked),
            input = inputs.recv() => match input {
                Some(VerifyInput::Cancel) | None => return None,
                // Nothing to accept or confirm yet.
                Some(VerifyInput::Accept | VerifyInput::Confirm) => {}
            },
        }
    }
}

/// Read the clipboard with `read` (which runs a clipboard tool and waits
/// for it), off the async threads and not for ever.
async fn from_clipboard<T: Send + 'static>(
    read: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match tokio::time::timeout(Duration::from_secs(10), tokio::task::spawn_blocking(read)).await {
        Ok(read) => read.context("could not read the clipboard")?,
        Err(_) => bail!("the clipboard did not answer"),
    }
}

/// The time in seconds, for the name of a pasted picture.
fn seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Picture formats looked for on the clipboard, best first.
const CLIPBOARD_IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// From the formats a clipboard tool lists, pick the picture to take.
pub fn pick_clipboard_image_type(listed: &str) -> Option<&'static str> {
    CLIPBOARD_IMAGE_TYPES
        .into_iter()
        .find(|wanted| listed.lines().any(|offered| offered.trim() == *wanted))
}

/// Read a picture from the system clipboard with `wl-paste` (Wayland) or
/// `xclip` (X11). Returns the bytes and their MIME type.
fn read_clipboard_image() -> Result<(Vec<u8>, String)> {
    use std::process::Command as Process;

    let tools: [(&str, &[&str], &[&str]); 2] = [
        ("wl-paste", &["--list-types"], &["--type"]),
        (
            "xclip",
            &["-selection", "clipboard", "-t", "TARGETS", "-o"],
            &["-selection", "clipboard", "-o", "-t"],
        ),
    ];
    let mut tool_found = false;
    for (tool, list_args, read_args) in tools {
        let Ok(listed) = Process::new(tool).args(list_args).output() else {
            continue;
        };
        tool_found = true;
        let Some(mime_type) = pick_clipboard_image_type(&String::from_utf8_lossy(&listed.stdout))
        else {
            continue;
        };
        let data = Process::new(tool).args(read_args).arg(mime_type).output()?;
        if data.status.success() && !data.stdout.is_empty() {
            return Ok((data.stdout, mime_type.to_owned()));
        }
    }
    // macOS has no tool of its own for pictures; pngpaste is the usual one.
    if let Ok(data) = Process::new("pngpaste").arg("-").output() {
        if data.status.success() && !data.stdout.is_empty() {
            return Ok((data.stdout, "image/png".to_owned()));
        }
        tool_found = true;
    }
    if tool_found {
        bail!("there is no picture on the clipboard");
    }
    bail!(
        "reading a picture from the clipboard needs wl-paste (Wayland), xclip (X11) or pngpaste (macOS)"
    )
}

/// Text on the system clipboard, if a clipboard tool is there and has any.
fn read_clipboard_text() -> Option<String> {
    use std::process::Command as Process;

    let tools: [(&str, &[&str]); 4] = [
        ("wl-paste", &["--no-newline", "--type", "text"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        // macOS and Windows.
        ("pbpaste", &[]),
        (
            "powershell",
            &["-NoProfile", "-Command", "Get-Clipboard -Raw"],
        ),
    ];
    for (tool, args) in tools {
        if let Ok(output) = Process::new(tool).args(args).output()
            && output.status.success()
            && !output.stdout.is_empty()
        {
            return Some(String::from_utf8_lossy(&output.stdout).into_owned());
        }
    }
    None
}

/// The longest side of the small version sent along with a picture.
const THUMBNAIL_SIDE: u32 = 800;

/// Size of a picture file and, when it is large, a smaller JPEG of it.
/// `None` when the file cannot be read as a picture.
fn picture_details(path: &Path, bytes: u64) -> Option<(BaseImageInfo, Option<Thumbnail>)> {
    let picture = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let info = BaseImageInfo {
        width: UInt::new(u64::from(picture.width())),
        height: UInt::new(u64::from(picture.height())),
        size: UInt::new(bytes),
        ..BaseImageInfo::default()
    };
    if picture.width().max(picture.height()) <= THUMBNAIL_SIDE {
        return Some((info, None));
    }
    let small = picture.thumbnail(THUMBNAIL_SIDE, THUMBNAIL_SIDE).to_rgb8();
    let mut data = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, 80)
        .encode_image(&small)
        .ok()?;
    let thumbnail = Thumbnail {
        content_type: mime::IMAGE_JPEG,
        width: UInt::new(u64::from(small.width()))?,
        height: UInt::new(u64::from(small.height()))?,
        size: UInt::new(data.len() as u64)?,
        data,
    };
    Some((info, Some(thumbnail)))
}

/// A message ready to send: plain text, optional HTML, and who is mentioned.
pub struct MessageParts {
    pub plain: String,
    pub html: Option<String>,
    pub mentioned: Vec<OwnedUserId>,
    pub emote: bool,
}

impl MessageParts {
    /// `text` is Markdown as typed. Every `@Display Name` that was completed
    /// from the member list becomes a proper mention: a link in the HTML,
    /// the bare name in the plain text, and an entry in `m.mentions` (which
    /// is what makes the other person's client notify them).
    pub fn new(text: &str, mentions: &[Member]) -> Self {
        let (text, emote) = match text.strip_prefix("/me ") {
            Some(action) => (action, true),
            None => (text, false),
        };
        let mut plain = String::with_capacity(text.len());
        let mut source = String::with_capacity(text.len());
        let mut mentioned: Vec<OwnedUserId> = Vec::new();

        // Longest names first, so "@Amy Winter" is not cut short by "@Amy".
        let mut by_length: Vec<(String, &Member, OwnedUserId)> = mentions
            .iter()
            .filter(|member| !member.name.is_empty())
            .filter_map(|member| {
                let user_id = UserId::parse(&member.user_id).ok()?;
                Some((format!("@{}", member.name), member, user_id))
            })
            .collect();
        by_length.sort_by_key(|(tag, ..)| std::cmp::Reverse(tag.len()));
        // One pass over the text as typed: each name where it stands as a
        // word. Replacing name after name in the whole text would also
        // find a short name inside the link just made for a longer one,
        // or inside an e-mail address.
        let mut rest = text;
        let mut before: Option<char> = None;
        while let Some(c) = rest.chars().next() {
            let found = (c == '@' && !before.is_some_and(char::is_alphanumeric))
                .then(|| {
                    by_length.iter().find(|(tag, ..)| {
                        rest.strip_prefix(tag.as_str()).is_some_and(|after| {
                            !after.chars().next().is_some_and(char::is_alphanumeric)
                        })
                    })
                })
                .flatten();
            match found {
                Some((tag, member, user_id)) => {
                    // The name is another person's text: as Markdown it
                    // must stay a name, whatever marks or HTML it holds.
                    let label: String = member
                        .name
                        .chars()
                        .flat_map(|c| {
                            c.is_ascii_punctuation()
                                .then_some('\\')
                                .into_iter()
                                .chain([c])
                        })
                        .collect();
                    source.push_str(&format!(
                        "[{label}](https://matrix.to/#/{})",
                        in_address(&member.user_id)
                    ));
                    plain.push_str(&member.name);
                    if !mentioned.contains(user_id) {
                        mentioned.push(user_id.clone());
                    }
                    before = tag.chars().last();
                    rest = &rest[tag.len()..];
                }
                None => {
                    plain.push(c);
                    source.push(c);
                    before = Some(c);
                    rest = &rest[c.len_utf8()..];
                }
            }
        }

        // `None` when the text has no formatting at all: then plain is enough.
        let html = FormattedBody::markdown(&source).map(|formatted| formatted.body);
        Self {
            plain,
            html,
            mentioned,
            emote,
        }
    }

    pub fn into_content(self) -> RoomMessageEventContent {
        let content = match (self.emote, self.html) {
            (false, Some(html)) => RoomMessageEventContent::text_html(self.plain, html),
            (false, None) => RoomMessageEventContent::text_plain(self.plain),
            (true, Some(html)) => RoomMessageEventContent::emote_html(self.plain, html),
            (true, None) => RoomMessageEventContent::emote_plain(self.plain),
        };
        if self.mentioned.is_empty() {
            content
        } else {
            content.add_mentions(Mentions::with_user_ids(self.mentioned))
        }
    }

    pub fn into_content_without_relation(self) -> RoomMessageEventContentWithoutRelation {
        let content = match (self.emote, self.html) {
            (false, Some(html)) => {
                RoomMessageEventContentWithoutRelation::text_html(self.plain, html)
            }
            (false, None) => RoomMessageEventContentWithoutRelation::text_plain(self.plain),
            (true, Some(html)) => {
                RoomMessageEventContentWithoutRelation::emote_html(self.plain, html)
            }
            (true, None) => RoomMessageEventContentWithoutRelation::emote_plain(self.plain),
        };
        if self.mentioned.is_empty() {
            content
        } else {
            content.add_mentions(Mentions::with_user_ids(self.mentioned))
        }
    }
}

/// An event that does not go through the timeline's queue (a vote or a
/// poll's end in a format the SDK's types cannot say): the room, its
/// type and content, and what to call it when it could not be sent.
type Beside = (Room, String, Value, &'static str);

/// Send what does not go through the timeline's queue, beside the
/// backend's loop: the server may take its time (or ask to wait and try
/// again), and what the user does next must not wait for it. One after
/// the other, in the order they were asked for: of two votes in one poll
/// the later one counts, and sent each on its own the first, tried
/// again, could arrive last.
async fn beside_lane(
    mut events: mpsc::UnboundedReceiver<Beside>,
    tx: mpsc::UnboundedSender<Update>,
) {
    while let Some((room, event_type, content, what)) = events.recv().await {
        if let Err(error) = room.send_raw(&event_type, content).await {
            let _ = tx.send(Update::Notice(format!("Could not send {what}: {error}")));
        }
    }
}

/// The conversation followed, for writing something new into it that was
/// written for `room_id` and the `thread` there (none: the room's own
/// conversation). Refused when that is not the one followed: a message
/// written in a thread that then could not be opened must not go into
/// the room's own conversation, which is followed in its place, nor a
/// file picked in one room into another opened meanwhile.
fn written_into<'a>(
    open: &'a Option<OpenRoom>,
    room_id: &str,
    thread: &Option<String>,
) -> Result<&'a Arc<Timeline>> {
    open.as_ref()
        .filter(|current| writes_into(&current.room_id, &current.thread, room_id, thread))
        .map(|current| &current.timeline)
        .ok_or_else(|| anyhow!("Not sent: that conversation is no longer open."))
}

/// Whether what was written for `room_id` and `thread` goes into the
/// conversation followed (`open_room`, `open_thread`).
fn writes_into(
    open_room: &str,
    open_thread: &Option<String>,
    room_id: &str,
    thread: &Option<String>,
) -> bool {
    open_room == room_id && open_thread == thread
}

fn timeline_of(open: &Option<OpenRoom>) -> Result<&Arc<Timeline>> {
    open.as_ref()
        .map(|current| &current.timeline)
        .ok_or_else(|| anyhow!("no room is open"))
}

fn item_id(event_id: &str) -> Result<TimelineEventItemId> {
    Ok(TimelineEventItemId::EventId(EventId::parse(event_id)?))
}

/// Build a poll in the unstable MSC3381 format (what Element sends and the
/// SDK timeline aggregates).
pub fn build_poll_start(
    question: &str,
    answers: &[String],
    multiple: bool,
    hidden: bool,
) -> Result<NewUnstablePollStartEventContent> {
    let question = question.trim();
    if question.is_empty() {
        bail!("a poll needs a question");
    }
    let texts: Vec<&str> = answers
        .iter()
        .map(|answer| answer.trim())
        .filter(|answer| !answer.is_empty())
        .collect();
    if texts.len() < 2 {
        bail!("a poll needs at least two answers");
    }
    if texts.len() > crate::message::MAX_POLL_ANSWERS {
        bail!(
            "a poll cannot have more than {} answers",
            crate::message::MAX_POLL_ANSWERS
        );
    }

    let poll_answers: Vec<UnstablePollAnswer> = texts
        .iter()
        .enumerate()
        .map(|(index, text)| UnstablePollAnswer::new(format!("answer-{}", index + 1), *text))
        .collect();
    let poll_answers = UnstablePollAnswers::try_from(poll_answers)
        .map_err(|error| anyhow!("invalid poll answers: {error}"))?;

    let mut block = UnstablePollStartContentBlock::new(question, poll_answers);
    block.kind = if hidden {
        PollKind::Undisclosed
    } else {
        PollKind::Disclosed
    };
    block.max_selections = UInt::from(if multiple { texts.len() as u32 } else { 1 });

    // Plain-text fallback for clients that cannot show polls.
    let mut fallback = question.to_owned();
    for (index, text) in texts.iter().enumerate() {
        fallback.push_str(&format!("\n{}. {}", index + 1, text));
    }
    Ok(NewUnstablePollStartEventContent::plain_text(
        fallback, block,
    ))
}

/// Download the attachment of a message into the cache directory and open
/// it with the desktop's default application.
/// The audio player started for a voice message or audio file, if any.
static AUDIO: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

/// Stop the audio that is playing; whether there was any.
fn stop_audio() -> bool {
    let Ok(mut playing) = AUDIO.lock() else {
        return false;
    };
    match playing.take() {
        Some(mut child) => {
            let was_running = matches!(child.try_wait(), Ok(None));
            let _ = child.kill();
            let _ = child.wait();
            was_running
        }
        None => false,
    }
}

/// Play an audio file in the background with mpv, if that is installed.
fn play_audio(path: &Path) -> bool {
    stop_audio();
    let child = std::process::Command::new("mpv")
        .args([
            "--no-video",
            "--really-quiet",
            "--no-terminal",
            // Nothing but this file: no fetching through yt-dlp, and
            // should mpv take it for a list after all, nothing the list
            // names that is not a plain file beside it.
            "--no-ytdl",
            "--load-unsafe-playlists=no",
        ])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match (child, AUDIO.lock()) {
        (Ok(child), Ok(mut playing)) => {
            *playing = Some(child);
            true
        }
        _ => false,
    }
}

/// A name for a downloaded file that stays inside the folder it is put
/// in, whatever the sender called it.
fn safe_file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = match safe.trim_matches(|c| c == '.' || c == ' ') {
        "" => "attachment".to_owned(),
        safe => shortened(safe),
    };
    // Windows keeps these names for devices, with any ending.
    let stem = safe.split('.').next().unwrap_or_default().to_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if device { format!("_{safe}") } else { safe }
}

/// The longest name a downloaded file gets, in bytes. File systems take
/// 255; the name is saved with a mark in front, perhaps an ending added,
/// and under a longer name first while it is written.
const MAX_FILE_NAME: usize = 150;

/// `name` cut down to [`MAX_FILE_NAME`], keeping its ending: that is what
/// says how the file is opened.
fn shortened(name: &str) -> String {
    if name.len() <= MAX_FILE_NAME {
        return name.to_owned();
    }
    // An "ending" longer than any real one is part of the name.
    let ending = match name.rsplit_once('.') {
        Some((_, ending)) if ending.len() <= 16 => format!(".{ending}"),
        _ => String::new(),
    };
    let mut cut = MAX_FILE_NAME - ending.len();
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    let stem = name[..cut].trim_end_matches(['.', ' ']);
    format!("{stem}{ending}")
}

/// What became of a downloaded attachment.
#[derive(Debug, PartialEq, Eq)]
enum Opened {
    /// Handed to the desktop's program for it.
    Shown,
    /// Played here, with mpv.
    Playing,
    /// Saved only.
    Kept,
    /// Saved for the app to show ([`hand_media_over`]).
    Handed,
}

/// The kinds of file the desktop is asked to open, by the ending of the
/// name: pictures, sound, video and documents. The name comes from
/// whoever sent the file, and "open" would run a program, a script or a
/// shortcut called `holiday.bat` as readily as it shows a picture.
///
/// No `svg`: where no viewer is set for it the browser opens it, and to
/// a browser it is a page with scripts.
const OPENED_BY_ITSELF: [&str; 33] = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "heic", "mp3", "ogg", "oga", "opus", "m4a", "aac",
    "flac", "wav", "mp4", "webm", "mkv", "mov", "avi", "pdf", "txt", "md", "csv", "odt", "ods",
    "odp", "docx", "xlsx", "pptx", "rtf", "epub", "ics",
];

/// The endings mpv is given: by a name like `voice.m3u` or `voice.txt` it
/// would take a file for a list of addresses to play, whatever the file
/// begins with.
const SOUND_ENDINGS: [&str; 9] = [
    "mp3", "ogg", "oga", "opus", "m4a", "aac", "flac", "wav", "webm",
];

fn ending_in(name: &str, endings: &[&str]) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ending)| endings.contains(&ending.to_lowercase().as_str()))
}

fn opened_by_itself(name: &str) -> bool {
    ending_in(name, &OPENED_BY_ITSELF)
}

fn named_as_sound(name: &str) -> bool {
    ending_in(name, &SOUND_ENDINGS)
}

/// The ending a picture's own bytes call for, whatever its name says.
fn picture_ending(bytes: &[u8]) -> Option<&'static str> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => Some("png"),
        image::ImageFormat::Jpeg => Some("jpg"),
        image::ImageFormat::Gif => Some("gif"),
        image::ImageFormat::WebP => Some("webp"),
        image::ImageFormat::Bmp => Some("bmp"),
        _ => None,
    }
}

/// The ending a sound file's own bytes call for; none for what does not
/// begin like sound. mpv would also take a list of addresses to play
/// (called `voice.ogg`), and fetch them.
fn sound_ending(bytes: &[u8]) -> Option<&'static str> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    if starts(b"OggS") {
        Some("ogg")
    } else if starts(b"ID3") || matches!(bytes, [0xff, second, ..] if second & 0xe0 == 0xe0) {
        Some("mp3")
    } else if starts(b"fLaC") {
        Some("flac")
    } else if starts(b"\x1a\x45\xdf\xa3") {
        Some("webm")
    } else if starts(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        Some("wav")
    } else if bytes.get(4..8) == Some(b"ftyp") {
        Some("m4a")
    } else {
        None
    }
}

/// The name an attachment is saved under: the sender's, made harmless,
/// with the ending its bytes call for (`ending`: of a picture or, with
/// `sound`, of an audio message) unless it has a fitting one, and a mark
/// of its message in front so that two files of one name do not overwrite
/// each other.
fn saved_name(name: &str, event_id: &str, ending: Option<&'static str>, sound: bool) -> String {
    let mut name = safe_file_name(name);
    let fits = if sound {
        named_as_sound(&name)
    } else {
        opened_by_itself(&name)
    };
    if let Some(ending) = ending
        && !fits
    {
        name = format!("{name}.{ending}");
    }
    let mark = hex::encode(&<sha2::Sha256 as sha2::Digest>::digest(event_id.as_bytes())[..3]);
    format!("{mark}-{name}")
}

/// A saved attachment's name without the mark [`saved_name`] put in
/// front: what its sender called it, for telling the user about it.
fn unmarked(saved: &str) -> &str {
    saved.split_once('-').map_or(saved, |(_, name)| name)
}

/// What is done with a downloaded attachment saved as `name`. `audio`:
/// it came as an audio message; `sound`: it begins like sound
/// ([`sound_ending`]); `hand_over`: an app shows files itself.
fn way_to_open(name: &str, audio: bool, sound: bool, hand_over: bool) -> Opened {
    // What came as sound or is called like sound, and does not begin
    // like it, is something else under that name (a list of addresses a
    // player would fetch): nothing gets to open it, whichever kind of
    // message it was sent as.
    let not_the_sound_it_says = (audio || named_as_sound(name)) && !sound;
    if not_the_sound_it_says || !opened_by_itself(name) {
        Opened::Kept
    } else if hand_over {
        Opened::Handed
    } else if audio && named_as_sound(name) {
        Opened::Playing
    } else {
        Opened::Shown
    }
}

/// Download an attachment and open it. Returns where it was saved, and
/// whether it is being played here (audio, with mpv) rather than handed
/// to the desktop.
async fn open_media(
    client: &Client,
    timeline: &Timeline,
    event_id: &str,
) -> Result<(PathBuf, Opened)> {
    let event = timeline
        .item_by_event_id(&EventId::parse(event_id)?)
        .await
        .ok_or_else(|| anyhow!("that message is no longer loaded"))?;
    let message = event
        .content()
        .as_message()
        .ok_or_else(|| anyhow!("nothing to open here"))?;
    let (source, name) = match message.msgtype() {
        MessageType::Image(content) => (content.source.clone(), content.filename().to_owned()),
        MessageType::File(content) => (content.source.clone(), content.filename().to_owned()),
        MessageType::Audio(content) => (content.source.clone(), content.filename().to_owned()),
        MessageType::Video(content) => (content.source.clone(), content.filename().to_owned()),
        _ => bail!("this message has no attachment"),
    };

    let bytes = client
        .media()
        .get_media_content(
            &MediaRequestParameters {
                source,
                format: MediaFormat::File,
            },
            true,
        )
        .await
        .context("could not download the attachment")?;

    let dir = crate::folders::cache()
        .context("no cache directory")?
        .join("media");
    // What an encrypted room sent lies here decrypted.
    crate::folders::private_dir(&dir)?;
    let audio = matches!(message.msgtype(), MessageType::Audio(_));
    let ending = match message.msgtype() {
        MessageType::Image(_) => picture_ending(&bytes),
        MessageType::Audio(_) => sound_ending(&bytes),
        _ => None,
    };
    let path = dir.join(saved_name(&name, event_id, ending, audio));
    crate::folders::write_private(&path, &bytes)?;

    let hand_over = HAND_MEDIA_OVER.load(std::sync::atomic::Ordering::Relaxed);
    let way = way_to_open(
        &path.to_string_lossy(),
        audio,
        sound_ending(&bytes).is_some(),
        hand_over,
    );
    match way {
        Opened::Kept | Opened::Handed => return Ok((path, way)),
        // Without mpv the desktop's player gets it.
        Opened::Playing if play_audio(&path) => return Ok((path, way)),
        Opened::Playing | Opened::Shown => {}
    }
    open_with_desktop(path.as_os_str())
        .with_context(|| format!("saved to {}, but could not start a viewer", path.display()))?;
    Ok((path, Opened::Shown))
}

/// What [`Command::OpenThread`] is given to show the pinned messages
/// instead of a thread.
pub const PINNED_VIEW: &str = "pinned";

/// What of a room the conversation view shows.
enum View {
    Live,
    Thread(OwnedEventId),
    Pinned,
}

/// Start following the conversation of a room, or one thread in it.
async fn open_timeline(
    room: Room,
    view: View,
    own_user_id: Option<&UserId>,
    tx: &mpsc::UnboundedSender<Update>,
) -> Result<OpenRoom> {
    let focus = match &view {
        View::Thread(root) => TimelineFocus::Thread {
            root_event_id: root.clone(),
        },
        View::Pinned => TimelineFocus::PinnedEvents,
        View::Live => TimelineFocus::Live {
            hide_threaded_events: true,
        },
    };
    let timeline = Arc::new(
        room.timeline_builder()
            .with_focus(focus)
            .track_read_marker_and_receipts(TimelineReadReceiptTracking::MessageLikeEvents)
            // Also keep poll starts in the standard format, which
            // the SDK would otherwise drop (see `stable_polls`).
            .event_filter(|event, rules| {
                default_event_filter(event, rules) || is_standard_poll_start(event)
            })
            .build()
            .await?,
    );
    let thread = match view {
        View::Thread(root) => Some(root.to_string()),
        View::Pinned => Some(PINNED_VIEW.to_owned()),
        View::Live => None,
    };
    let (nudge, nudges) = mpsc::unbounded_channel();
    let task = tokio::spawn(timeline_loop(
        room.clone(),
        thread.clone(),
        Arc::clone(&timeline),
        own_user_id.map(ToOwned::to_owned),
        tx.clone(),
        nudges,
    ));
    Ok(OpenRoom {
        room_id: room.room_id().to_string(),
        thread,
        room,
        timeline,
        task,
        nudge,
    })
}

/// Follow one room's timeline and send a fresh snapshot on every change.
async fn timeline_loop(
    room: Room,
    thread: Option<String>,
    timeline: Arc<Timeline>,
    own_user_id: Option<OwnedUserId>,
    tx: mpsc::UnboundedSender<Update>,
    mut nudges: mpsc::UnboundedReceiver<Nudge>,
) {
    let room_id = room.room_id().to_string();
    let mut watch = RoomWatch::new(match thread.as_deref() {
        None => crate::room_watch::Shown::Room,
        Some(PINNED_VIEW) => crate::room_watch::Shown::Pinned,
        Some(_) => crate::room_watch::Shown::Thread,
    });
    let (poll_loaded_tx, mut poll_loaded) = mpsc::unbounded_channel::<PollLoaded>();
    let (receipt_failed_tx, mut receipt_failed) = mpsc::unbounded_channel::<bool>();
    let (mut items, stream) = timeline.subscribe().await;
    let mut stream = Box::pin(stream);
    let mut names = Names::new();
    let downloads = Arc::new(tokio::sync::Semaphore::new(3));
    let (members_loaded_tx, mut members_loaded) = mpsc::unbounded_channel::<()>();

    // Load the member list so senders and poll voters show their display
    // names (rooms are lazy-loaded), and fill the screen on first open.
    {
        let timeline = Arc::clone(&timeline);
        let short = items.len() < 30;
        let room = room.clone();
        let room_id = room_id.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            timeline.fetch_members().await;
            if let Ok(joined) = room.members_no_sync(RoomMemberships::JOIN).await {
                let members = joined
                    .iter()
                    .map(|member| Member {
                        user_id: member.user_id().to_string(),
                        name: member.name().to_owned(),
                    })
                    .collect();
                let _ = tx.send(Update::Members { room_id, members });
            }
            let _ = members_loaded_tx.send(());
            if short {
                let _ = timeline.paginate_backwards(40).await;
            }
        });
    }

    let (_typing_guard, mut typing) = room.subscribe_to_typing_notifications();

    loop {
        while let Ok(with_threads) = receipt_failed.try_recv() {
            if with_threads {
                watch.read_with_threads_failed();
            } else {
                watch.read_failed();
            }
        }
        let (seen, pictures) = seen_in(&items, &watch);
        let todo = watch.see(seen);
        for poll in todo.load_polls {
            tokio::spawn(load_poll_events(room.clone(), poll, poll_loaded_tx.clone()));
        }

        publish_timeline(
            &room,
            &room_id,
            thread.as_deref(),
            &items,
            own_user_id.as_deref(),
            &mut names,
            watch.polls(),
            &tx,
        )
        .await;

        for event_id in todo.fetch_pictures {
            if let Some(source) = pictures.get(&event_id) {
                tokio::spawn(fetch_preview(
                    room.client(),
                    source.clone(),
                    event_id,
                    Arc::clone(&downloads),
                    tx.clone(),
                ));
            }
        }
        // Receipts go out beside the loop: a slow or rate-limiting server
        // must not hold up showing what arrives. One that did not go out
        // is sent again with the next snapshot (the loop comes round with
        // every change in the room).
        let newest = todo
            .read_with_threads
            .and_then(|id| OwnedEventId::try_from(id).ok());
        if todo.mark_read || newest.is_some() || todo.clear_unread_flag {
            let (timeline, room, failed) = (
                Arc::clone(&timeline),
                room.clone(),
                receipt_failed_tx.clone(),
            );
            let (mark_read, clear_flag) = (todo.mark_read, todo.clear_unread_flag);
            tokio::spawn(async move {
                if mark_read && timeline.mark_as_read(ReceiptType::Read).await.is_err() {
                    let _ = failed.send(false);
                }
                if let Some(newest) = newest
                    && room
                        .send_single_receipt(ReceiptType::Read, ReceiptThread::Unthreaded, newest)
                        .await
                        .is_err()
                {
                    let _ = failed.send(true);
                }
                if clear_flag && room.is_marked_unread() {
                    let _ = room.set_unread_flag(false).await;
                }
            });
        }

        tokio::select! {
            diffs = stream.next() => {
                let Some(diffs) = diffs else { break };
                for diff in diffs {
                    apply(diff, &mut items);
                }
            }
            Some((poll_id, events)) = poll_loaded.recv() => {
                tracing::info!(poll_id, related_events = events.len(), "poll answers read");
                // It changed while it was read: once more.
                if watch.poll_loaded(&poll_id, &events) {
                    tokio::spawn(load_poll_events(room.clone(), poll_id, poll_loaded_tx.clone()));
                }
            }
            Ok(users) = typing.recv() => {
                learn_names(&room, users.iter().map(ToString::to_string), &mut names).await;
                let names = users
                    .iter()
                    .map(|user| convert::name_for(user.as_str(), &names))
                    .collect();
                let _ = tx.send(Update::Typing { room_id: room_id.clone(), names });
            }
            // The member list is in: names not found before may be now.
            Some(()) = members_loaded.recv() => {}
            Some(nudge) = nudges.recv() => match nudge {
                Nudge::PollChanged(poll_id) if watch.poll_changed(&poll_id) => {
                    tokio::spawn(load_poll_events(room.clone(), poll_id, poll_loaded_tx.clone()));
                }
                // The loop goes round and publishes again.
                Nudge::PollChanged(_) | Nudge::Republish => {}
            },
        }
    }
}

/// Read what [`RoomWatch`] decides on out of the SDK's timeline items,
/// and where each picture is to be fetched from.
fn seen_in(
    items: &Vector<Arc<TimelineItem>>,
    watch: &RoomWatch,
) -> (Seen, HashMap<String, MediaSource>) {
    let mut seen = Seen {
        now: MilliSecondsSinceUnixEpoch::now().get().into(),
        ..Seen::default()
    };
    let mut pictures = HashMap::new();
    for event in items.iter().filter_map(|item| item.as_event()) {
        let event_id = event.event_id().map(ToString::to_string);
        if let Some(id) = &event_id {
            seen.newest = Some(id.clone());
            seen.times.push(event.timestamp().get().into());
        }
        // The newest reply of a thread that starts here.
        if let TimelineItemContent::MsgLike(content) = event.content()
            && let Some(summary) = &content.thread_summary
            && let TimelineDetails::Ready(reply) = &summary.latest_event
            && let TimelineEventItemId::EventId(id) = &reply.identifier
        {
            seen.replies
                .push((reply.timestamp.get().into(), id.to_string()));
        }
        let Some(id) = event_id else {
            continue;
        };
        match event.content() {
            // A poll the SDK tallies itself (the older format).
            TimelineItemContent::MsgLike(content) => match &content.kind {
                MsgLikeKind::Poll(state) => seen.sdk_polls.push((
                    id,
                    convert::poll_mark(state),
                    convert::own_tally_for(event.sender().as_str(), state),
                )),
                MsgLikeKind::Message(message) => {
                    if let MessageType::Image(content) = message.msgtype() {
                        let size = content
                            .info
                            .as_ref()
                            .and_then(|info| info.size)
                            .map(u64::from);
                        pictures.insert(id.clone(), content.source.clone());
                        seen.pictures.push((id, size));
                    }
                }
                // A standard-format poll the SDK knows but does not show.
                MsgLikeKind::Other(other) => {
                    let event_type = other.event_type().to_string();
                    if let Some(poll) = standard_poll_start(event, &event_type, &id, watch) {
                        seen.new_standard_polls.push((id, poll));
                    }
                }
                _ => {}
            },
            // One whose content the SDK could not parse at all (IServ's
            // layout).
            TimelineItemContent::FailedToParseMessageLike { event_type, .. } => {
                let event_type = event_type.to_string();
                if let Some(poll) = standard_poll_start(event, &event_type, &id, watch) {
                    seen.new_standard_polls.push((id, poll));
                }
            }
            _ => {}
        }
    }
    (seen, pictures)
}

/// The poll an event starts, in one of the standard formats, if it is one
/// not followed yet.
fn standard_poll_start(
    event: &EventTimelineItem,
    event_type: &str,
    id: &str,
    watch: &RoomWatch,
) -> Option<StablePoll> {
    if !stable_polls::START_TYPES.contains(&event_type) || watch.tracks(id) {
        return None;
    }
    let content = event
        .original_json()
        .and_then(|raw| serde_json::from_str::<Value>(raw.json().get()).ok())
        .map(|mut value| value["content"].take())?;
    stable_polls::parse_start(event.sender().as_str(), &content)
}

fn is_standard_poll_start(event: &AnySyncTimelineEvent) -> bool {
    match event {
        AnySyncTimelineEvent::MessageLike(message) => {
            message.original_content().is_some()
                && stable_polls::START_TYPES.contains(&message.event_type().to_string().as_str())
        }
        AnySyncTimelineEvent::State(_) => false,
    }
}

/// Read every event that refers to a poll (its responses and end) from the
/// server and hand them to the timeline task as raw JSON. A reading that
/// failed on the way (no network, a server that says to wait) is no
/// reading: it is made again after a pause, and again, until one went
/// through or the room is no longer followed.
/// [`RoomWatch`] sees to it that one poll has one of these at a time.
async fn load_poll_events(room: Room, poll_id: String, done: mpsc::UnboundedSender<PollLoaded>) {
    let Ok(event_id) = EventId::parse(&poll_id) else {
        return;
    };
    let mut tries = 0;
    loop {
        if let Some(events) = read_poll_events(&room, &event_id).await {
            let _ = done.send((poll_id, events));
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(poll_retry_after(tries)) => {}
            () = done.closed() => return,
        }
        tries += 1;
    }
}

/// How long to wait before a poll's answers are read once more, after
/// `tries` readings that failed.
fn poll_retry_after(tries: usize) -> Duration {
    const WAITS: [u64; 4] = [5, 15, 60, 300];
    Duration::from_secs(WAITS[tries.min(WAITS.len() - 1)])
}

/// One reading of a poll's answers; none when it failed on the way.
/// Half an answer is none: counting only what came before the network
/// failed would show a poll with votes missing, and the reading is made
/// again.
async fn read_poll_events(room: &Room, event_id: &EventId) -> Option<Vec<Value>> {
    let mut read = PollPages::default();
    let mut from = None;
    loop {
        let options = RelationsOptions {
            from: from.take(),
            include_relations: IncludeRelations::RelationsOfType(RelationType::Reference),
            // Not the server's own page size, which can be a handful.
            limit: Some(100u32.into()),
            ..Default::default()
        };
        let page = room.relations(event_id.to_owned(), options).await.ok()?;
        let events = page
            .chunk
            .iter()
            .filter_map(|event| serde_json::from_str::<Value>(event.raw().json().get()).ok());
        match read.take(events, page.next_batch_token) {
            Some(next) => from = Some(next),
            None => break,
        }
    }
    if read.cut_off {
        tracing::warn!(
            poll_id = %event_id,
            "a poll has more answers than are read: it is counted from the first {}",
            read.events.len()
        );
    }
    Some(read.events)
}

/// How many pages of a poll's answers are read.
const POLL_PAGES: usize = 40;

/// A poll's answers as they are read from the server, page by page.
#[derive(Default)]
struct PollPages {
    events: Vec<Value>,
    pages: usize,
    /// There were more pages than are read.
    cut_off: bool,
}

impl PollPages {
    /// A page was read: its events, and where the next one begins if
    /// there is one. Returns where to read on; none when that was the
    /// last page, or it is enough ([`POLL_PAGES`]). What was read by
    /// then is the reading: calling it a failure had a poll with
    /// thousands of answers read again for ever, and shown with no votes
    /// at all.
    fn take(
        &mut self,
        events: impl Iterator<Item = Value>,
        next: Option<String>,
    ) -> Option<String> {
        self.events.extend(events);
        self.pages += 1;
        self.cut_off = next.is_some() && self.pages >= POLL_PAGES;
        next.filter(|_| !self.cut_off)
    }
}

/// A poll's answers as read from the server, all of them.
type PollLoaded = (String, Vec<Value>);

async fn fetch_preview(
    client: Client,
    source: MediaSource,
    event_id: String,
    downloads: Arc<tokio::sync::Semaphore>,
    tx: mpsc::UnboundedSender<Update>,
) {
    let Ok(_permit) = downloads.acquire().await else {
        return;
    };
    let request = MediaRequestParameters {
        source,
        format: MediaFormat::File,
    };
    let Ok(bytes) = client.media().get_media_content(&request, true).await else {
        return;
    };
    // What the message said of its size was the sender's word.
    if bytes.len() as u64 > crate::room_watch::MAX_PREVIEW_BYTES {
        return;
    }
    // Decoding and scaling is CPU work; keep it off the async threads.
    let scaled = tokio::task::spawn_blocking(move || preview::make_preview(&bytes)).await;
    if let Ok(Some(preview)) = scaled {
        let _ = tx.send(Update::Preview { event_id, preview });
    }
}

fn apply(diff: VectorDiff<Arc<TimelineItem>>, items: &mut Vector<Arc<TimelineItem>>) {
    diff.apply(items);
}

/// Look up the display names of `people` not known yet, in the local
/// member store (never the network). Someone not found is tried again
/// next time, since members load lazily. On some servers (IServ) user ids
/// are random, so without this a reader shows as a long run of letters.
async fn learn_names(room: &Room, people: impl Iterator<Item = String>, names: &mut Names) {
    let unknown: HashSet<String> = people.filter(|id| !names.contains_key(id)).collect();
    for id in unknown {
        if let Ok(user_id) = UserId::parse(&id)
            && let Ok(Some(member)) = room.get_member_no_sync(&user_id).await
            && let Some(name) = member.display_name()
        {
            names.insert(id, name.to_owned());
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn publish_timeline(
    room: &Room,
    room_id: &str,
    thread: Option<&str>,
    items: &Vector<Arc<TimelineItem>>,
    own_user_id: Option<&UserId>,
    names: &mut Names,
    stable_polls: &StablePolls,
    tx: &mpsc::UnboundedSender<Update>,
) {
    // Poll voters and readers may never have written a message.
    let people = items
        .iter()
        .flat_map(|item| convert::people_shown(item))
        .chain(
            stable_polls
                .values()
                .flat_map(|poll| poll.voter_ids())
                .map(ToOwned::to_owned),
        );
    learn_names(room, people, names).await;

    // Where we moderate, other people's messages can be deleted too.
    let moderator = match (own_user_id, room.power_levels().await) {
        (Some(own), Ok(levels)) => levels.user_can_redact_event_of_other(own),
        _ => false,
    };
    let pinned: Vec<String> = room
        .pinned_event_ids()
        .unwrap_or_default()
        .iter()
        .map(ToString::to_string)
        .collect();
    let converted = convert::without_repeats(
        items
            .iter()
            .filter_map(|item| convert::convert(item, own_user_id, names, stable_polls))
            .map(|mut item| {
                item.can_delete |= moderator && !item.event_id.is_empty();
                item.pinned = !item.event_id.is_empty() && pinned.contains(&item.event_id);
                item
            })
            .collect(),
    );
    let _ = tx.send(match thread {
        Some(root) => Update::Thread {
            room_id: room_id.to_owned(),
            root: root.to_owned(),
            items: converted,
        },
        None => Update::Timeline {
            room_id: room_id.to_owned(),
            items: converted,
        },
    });
}

#[cfg(test)]
mod tests {
    use super::{
        BackupStep, EMPTY_LIST_GRACE, EmptyListGate, MessageParts, MessageType, Notify,
        RecoveryState, backup_step, build_poll_start, desktop_opens, holds_encrypted_file, mode_of,
        notify_of, pick_clipboard_image_type, safe_file_name, server_from_input, shape_of,
        unread_counts,
    };

    #[test]
    fn a_profile_s_lock_outlives_what_is_deleted_and_is_let_go_when_asked() {
        let dir = std::env::temp_dir().join(format!("mothrix-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = super::Paths {
            config_file: dir.join("config.toml"),
            data_dir: dir.join("work"),
        };
        let lock = paths.data_dir.join(super::LOCK);
        let taken = |path: &std::path::Path| {
            let file = std::fs::File::options().write(true).open(path).unwrap();
            matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
        };
        super::hold(&paths).unwrap();
        std::fs::write(paths.session_file(), "{}").unwrap();
        std::fs::create_dir_all(paths.store_dir().join("deep")).unwrap();
        // A failed sign-in or a sign-out deletes the account's files: the
        // lock's file is not among them, so the profile is still held.
        super::forget(&paths).unwrap();
        assert!(!paths.session_file().exists() && !paths.store_dir().exists());
        assert!(taken(&lock), "a second mothrix is still kept out");
        super::hold(&paths).unwrap();
        assert!(taken(&lock));
        super::release(&paths);
        assert!(!taken(&lock), "free for another mothrix");
        // A profile that never was: nothing to delete.
        let none = super::Paths {
            config_file: dir.join("config.toml"),
            data_dir: dir.join("none"),
        };
        super::forget(&none).unwrap();
        assert!(!none.data_dir.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unread_counts_take_the_larger_and_a_marked_room_counts_one() {
        // A gap the SDK did not see: the server's count.
        assert_eq!(unread_counts((1, 0), (5, 0), false), (5, 0));
        // A mention inside an encrypted message: only the SDK sees it.
        assert_eq!(unread_counts((2, 1), (2, 0), false), (2, 1));
        assert_eq!(unread_counts((0, 0), (0, 0), true), (1, 0));
        assert_eq!(unread_counts((3, 0), (0, 0), true), (3, 0));
        assert_eq!(unread_counts((0, 0), (0, 0), false), (0, 0));
    }

    #[test]
    fn a_notification_setting_reads_back_as_it_was_set() {
        for notify in [Notify::Default, Notify::All, Notify::Mentions, Notify::Mute] {
            assert_eq!(notify_of(mode_of(notify)), notify);
        }
    }

    #[test]
    fn a_forwarded_message_travels_as_json_and_its_encrypted_file_is_known() {
        let text: MessageType =
            serde_json::from_str(r#"{"msgtype":"m.text","body":"hi"}"#).unwrap();
        let json = serde_json::to_string(&text).unwrap();
        let back: MessageType = serde_json::from_str(&json).unwrap();
        assert_eq!(back.body(), "hi");
        assert!(!holds_encrypted_file(&text));

        let plain: MessageType =
            serde_json::from_str(r#"{"msgtype":"m.image","body":"a.png","url":"mxc://x.org/a"}"#)
                .unwrap();
        assert!(!holds_encrypted_file(&plain));
        let encrypted: MessageType = serde_json::from_str(
            r#"{"msgtype":"m.file","body":"a.pdf","file":{
                "url":"mxc://x.org/a","v":"v2","iv":"AAAAAAAAAAAAAAAAAAAAAA",
                "hashes":{"sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},
                "key":{"kty":"oct","key_ops":["encrypt","decrypt"],"alg":"A256CTR",
                       "k":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","ext":true}}}"#,
        )
        .unwrap();
        assert!(holds_encrypted_file(&encrypted));
        let json = serde_json::to_string(&encrypted).unwrap();
        assert!(holds_encrypted_file(
            &serde_json::from_str::<MessageType>(&json).unwrap()
        ));
    }

    #[test]
    fn a_room_list_that_empties_for_a_moment_is_not_shown_empty() {
        let start = std::time::Instant::now();
        let mut gate = EmptyListGate::default();
        // A new account without rooms shows its empty list.
        assert!(gate.show(true, start));
        assert!(gate.show(false, start));
        assert!(!gate.show(true, start));
        assert!(!gate.show(true, start + EMPTY_LIST_GRACE / 2));
        // Back before the grace is over: never shown empty.
        assert!(gate.show(false, start + EMPTY_LIST_GRACE / 2));
        assert!(!gate.show(true, start + EMPTY_LIST_GRACE));
        // Empty for good: the last room was left.
        assert!(gate.show(true, start + EMPTY_LIST_GRACE * 2));
    }

    #[test]
    fn large_pictures_get_a_small_version_and_small_ones_do_not() {
        let dir = std::env::temp_dir().join(format!("mothrix-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let large = dir.join("large.png");
        image::RgbImage::from_pixel(1600, 400, image::Rgb([200, 80, 40]))
            .save(&large)
            .unwrap();
        let (info, thumbnail) = super::picture_details(&large, 1234).unwrap();
        assert_eq!(info.width.map(u64::from), Some(1600));
        assert_eq!(info.height.map(u64::from), Some(400));
        let thumbnail = thumbnail.expect("a small version");
        assert_eq!(u64::from(thumbnail.width), 800);
        assert_eq!(u64::from(thumbnail.height), 200);
        assert_eq!(&thumbnail.data[..2], [0xFF, 0xD8], "a JPEG");

        let small = dir.join("small.png");
        image::RgbImage::new(64, 64).save(&small).unwrap();
        let (info, thumbnail) = super::picture_details(&small, 99).unwrap();
        assert_eq!(info.width.map(u64::from), Some(64));
        assert!(thumbnail.is_none());

        std::fs::write(dir.join("text.png"), "not a picture").unwrap();
        assert!(super::picture_details(&dir.join("text.png"), 13).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rooms_get_the_section_of_their_space_one_level_deep() {
        let id = |name: &str| name.to_owned();
        let spaces = vec![
            (
                id("!school"),
                id("School"),
                vec![id("!hall"), id("!7b"), id("!maths")],
            ),
            (id("!7b"), id("Class 7b / A"), vec![id("!maths")]),
        ];
        let sections = super::space_sections(&spaces);
        assert_eq!(sections["!hall"], "School");
        // Listed in both: the subspace is where it shows.
        assert_eq!(sections["!maths"], "School/Class 7b - A");
        assert_eq!(sections["!7b"], "School");
    }

    #[test]
    fn clipboard_pictures_are_picked_by_preference() {
        assert_eq!(
            pick_clipboard_image_type("text/plain\nimage/jpeg\nimage/png\n"),
            Some("image/png")
        );
        assert_eq!(
            pick_clipboard_image_type("TARGETS\nimage/webp\n"),
            Some("image/webp")
        );
        assert_eq!(pick_clipboard_image_type("text/plain\nUTF8_STRING\n"), None);
    }

    #[test]
    fn event_shapes_keep_structure_but_hide_text() {
        let shape = shape_of(&serde_json::json!({
            "msgtype": "m.text",
            "body": "secret words",
            "m.poll": {"kind": "m.disclosed", "max_selections": 2, "answers": [{"m.id": "a"}]},
            "m.relates_to": {"rel_type": "m.reference", "event_id": "$abc"},
        }));
        assert_eq!(shape["msgtype"], "m.text");
        assert_eq!(shape["body"], "<text>");
        assert_eq!(shape["m.poll"]["kind"], "m.disclosed");
        assert_eq!(shape["m.poll"]["max_selections"], 2);
        assert_eq!(shape["m.poll"]["answers"][0]["m.id"], "<text>");
        assert_eq!(shape["m.relates_to"]["rel_type"], "m.reference");
        assert_eq!(shape["m.relates_to"]["event_id"], "<text>");
    }
    use crate::model::Member;

    fn member(user_id: &str, name: &str) -> Member {
        Member {
            user_id: user_id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn plain_text_stays_plain() {
        let parts = MessageParts::new("just words", &[]);
        assert_eq!(parts.plain, "just words");
        assert!(parts.html.is_none());
        assert!(parts.mentioned.is_empty());
    }

    #[test]
    fn markdown_becomes_html() {
        let parts = MessageParts::new("this is **bold** and `code`", &[]);
        let html = parts.html.expect("formatted");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("<code>code</code>"), "{html}");
        assert_eq!(parts.plain, "this is **bold** and `code`");
    }

    #[test]
    fn a_mentioned_name_stays_a_name_and_stands_as_a_word() {
        let members = [
            member(
                "@mal:x.org",
                "Bob</a><a href=\"https://evil.example\">sign in",
            ),
            member("@a:x.org", "a"),
            member("@amyw:x.org", "Amy Winter"),
        ];
        let text =
            "@Bob</a><a href=\"https://evil.example\">sign in and @Amy Winter, mail me@abc.de, @a";
        let parts = MessageParts::new(text, &members);
        let html = parts.html.expect("mentions make html");
        assert!(!html.contains("<a href=\"https://evil.example\""), "{html}");
        assert_eq!(
            html.matches("<a href=").count(),
            3,
            "three mentions, no more: {html}"
        );
        // The short name is found neither in the address nor in the link
        // made for the long one.
        assert!(html.contains("me@abc.de"), "{html}");
        assert!(
            html.contains(r#"<a href="https://matrix.to/#/@amyw:x.org">Amy Winter</a>"#),
            "{html}"
        );
        assert_eq!(parts.mentioned.len(), 3);
    }

    #[test]
    fn only_harmless_kinds_of_file_are_opened_and_under_a_name_that_fits() {
        use super::{
            Opened, opened_by_itself, picture_ending, saved_name, sound_ending, way_to_open,
        };
        assert!(opened_by_itself("Holiday.PNG") && opened_by_itself("minutes.pdf"));
        for name in [
            "holiday.bat",
            "setup.exe",
            "page.html",
            "x.desktop",
            "run.sh",
            "noending",
            // A browser would run its scripts.
            "drawing.svg",
        ] {
            assert!(!opened_by_itself(name), "{name}");
        }
        // A picture is called what it is; anything else keeps its name.
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        assert_eq!(picture_ending(&png), Some("png"));
        assert!(
            saved_name("holiday.bat", "$1", picture_ending(&png), false)
                .ends_with("-holiday.bat.png")
        );
        assert!(
            saved_name("holiday.png", "$1", picture_ending(&png), false).ends_with("-holiday.png")
        );
        assert_ne!(
            saved_name("a.png", "$1", None, false),
            saved_name("a.png", "$2", None, false)
        );
        assert!(
            saved_name("../../CON.txt", "$1", None, false).ends_with("-_.._CON.txt")
                || saved_name("CON.txt", "$1", None, false).ends_with("-_CON.txt")
        );
        assert_eq!(super::safe_file_name("nul"), "_nul");
        assert_eq!(super::safe_file_name("com1.txt"), "_com1.txt");
        // Sound is sound; a list of addresses is not.
        assert_eq!(sound_ending(b"OggS\0\x02"), Some("ogg"));
        assert_eq!(sound_ending(b"ID3\x04"), Some("mp3"));
        assert_eq!(sound_ending(b"RIFF\0\0\0\0WAVEfmt "), Some("wav"));
        assert_eq!(sound_ending(b"#EXTM3U\nhttps://example.org/a.mp3"), None);
        assert_eq!(sound_ending(b"https://x"), None);

        // Hand a file to the app only when it is of a kind that is
        // opened. An audio message that begins like sound is played.
        assert_eq!(way_to_open("a.png", false, false, false), Opened::Shown);
        assert_eq!(way_to_open("voice.ogg", true, true, false), Opened::Playing);
        // A sound file sent as a file goes to the desktop's player.
        assert_eq!(way_to_open("song.mp3", false, true, false), Opened::Shown);
        for name in ["invoice.html", "x.svg", "page.webarchive", "model.usdz"] {
            assert_eq!(
                way_to_open(name, false, false, false),
                Opened::Kept,
                "{name}"
            );
            assert_eq!(
                way_to_open(name, false, false, true),
                Opened::Kept,
                "{name}"
            );
        }
        assert_eq!(way_to_open("a.png", false, false, true), Opened::Handed);
        assert_eq!(way_to_open("voice.ogg", true, true, true), Opened::Handed);
        // The user is told of a kept file by its sender's name.
        let saved = saved_name("in-voice 3.html", "$1", None, false);
        assert_eq!(super::unmarked(&saved), "in-voice 3.html");
    }

    #[test]
    fn what_is_called_like_sound_and_is_none_reaches_no_player_as_a_file_or_video_either() {
        use super::{Opened, saved_name, sound_ending, way_to_open};
        // A list of addresses called `voice.ogg`, sent as a file or as a
        // video: the desktop would hand it to its player by its name.
        let list = b"#EXTM3U\nhttps://example.org/a.mp3";
        let name = saved_name("voice.ogg", "$1", None, false);
        let sound = sound_ending(list).is_some();
        for hand_over in [false, true] {
            assert_eq!(way_to_open(&name, false, sound, hand_over), Opened::Kept);
        }
        // Real sound sent as a file opens as before, and so does what is
        // not called like sound.
        let sound = sound_ending(b"OggS\0\x02").is_some();
        assert_eq!(way_to_open(&name, false, sound, false), Opened::Shown);
        assert_eq!(way_to_open("film.mp4", false, false, false), Opened::Shown);
        assert_eq!(way_to_open("notes.pdf", false, false, true), Opened::Handed);
    }

    #[test]
    fn what_was_written_for_another_conversation_than_the_one_followed_is_refused() {
        use super::{Command, writes_into};
        let thread = Some("$root".to_owned());
        // The thread did not open, and the room is followed in its
        // place; or another room was opened meanwhile.
        assert!(!writes_into("!a", &None, "!a", &thread));
        assert!(!writes_into("!a", &thread, "!a", &None));
        assert!(!writes_into("!b", &None, "!a", &None));
        assert!(!writes_into("!a", &thread, "!a", &Some("$other".into())));
        assert!(writes_into("!a", &None, "!a", &None));
        assert!(writes_into("!a", &thread, "!a", &thread));
        // Everything that writes into the conversation names it: these
        // are routed by their room, like `Send`.
        let place = || ("!a".to_owned(), thread.clone());
        let (room_id, thread) = place();
        let mut commands = vec![
            Command::Send {
                room_id: room_id.clone(),
                thread: thread.clone(),
                text: String::new(),
                mentions: Vec::new(),
            },
            Command::SendFile {
                room_id: room_id.clone(),
                thread: thread.clone(),
                path: String::new(),
                caption: None,
            },
            Command::SendVoice {
                room_id: room_id.clone(),
                thread: thread.clone(),
                path: String::new(),
                duration_ms: 0,
                waveform: Vec::new(),
            },
            Command::CreatePoll {
                room_id: room_id.clone(),
                thread: thread.clone(),
                question: String::new(),
                answers: Vec::new(),
                multiple: false,
                hidden: false,
            },
            Command::PasteImage { room_id, thread },
        ];
        for command in &mut commands {
            let room = command.room_id_mut().map(|room| room.clone());
            assert_eq!(room.as_deref(), Some("!a"), "{command:?}");
        }
    }

    #[test]
    fn a_device_is_signed_out_with_its_token_alone_at_the_session_s_homeserver() {
        use super::{logout_answer, logout_request};
        let client = matrix_sdk::reqwest::Client::new();
        for homeserver in ["https://matrix.example.org/", " https://matrix.example.org"] {
            let request = logout_request(&client, homeserver, "syt_secret").unwrap();
            assert_eq!(request.method().as_str(), "POST");
            assert_eq!(
                request.url().as_str(),
                "https://matrix.example.org/_matrix/client/v3/logout"
            );
            assert_eq!(request.headers()["authorization"], "Bearer syt_secret");
            assert!(request.timeout().is_some());
            // The token is in the header alone.
            assert!(!request.url().as_str().contains("syt_secret"));
        }
        // A server under a path keeps it; what is no address is refused.
        let request = logout_request(&client, "http://127.0.0.1:16167/hs", "t").unwrap();
        assert_eq!(
            request.url().as_str(),
            "http://127.0.0.1:16167/hs/_matrix/client/v3/logout"
        );
        assert!(logout_request(&client, "matrix.example.org", "t").is_err());
        assert!(logout_request(&client, "", "t").is_err());

        assert!(logout_answer(200, "{}").is_ok());
        // Signed out already: that is what was wanted.
        assert!(logout_answer(401, r#"{"errcode":"M_UNKNOWN_TOKEN"}"#).is_ok());
        // A token that ran out still names a device of the account.
        let soft = r#"{"errcode":"M_UNKNOWN_TOKEN","soft_logout":true}"#;
        assert!(logout_answer(401, soft).is_err());
        assert!(logout_answer(401, r#"{"errcode":"M_MISSING_TOKEN"}"#).is_err());
        assert!(logout_answer(502, "<html>").is_err());
    }

    #[test]
    fn the_way_to_remove_an_account_names_its_profile() {
        let paths = |profile: &str| super::Paths {
            config_file: "/c/config.toml".into(),
            data_dir: std::path::Path::new("/d").join(profile),
        };
        assert_eq!(super::logout_command(&paths("default")), "mothrix logout");
        assert_eq!(
            super::logout_command(&paths("work")),
            "mothrix logout --profile work"
        );
    }

    #[test]
    fn a_poll_with_more_answers_than_are_read_is_counted_from_what_was_read() {
        use super::{POLL_PAGES, PollPages};
        let vote = || serde_json::json!({"type": "m.poll.response"});
        let votes = || [vote(), vote()].into_iter();
        // Pages without end: the reading stops, and is one.
        let mut read = PollPages::default();
        let mut pages = 0;
        while read.take(votes(), Some("next".to_owned())).is_some() {
            pages += 1;
            assert!(pages < 1000, "read for ever");
        }
        assert_eq!(read.pages, POLL_PAGES);
        assert_eq!(read.events.len(), 2 * POLL_PAGES);
        assert!(read.cut_off);
        // A poll of three pages is read whole, from token to token.
        let mut read = PollPages::default();
        assert_eq!(read.take(votes(), Some("b".into())).as_deref(), Some("b"));
        assert_eq!(read.take(votes(), Some("c".into())).as_deref(), Some("c"));
        assert_eq!(read.take(votes(), None), None);
        assert_eq!((read.events.len(), read.cut_off), (6, false));
        // One of exactly as many pages as are read is whole too.
        let mut read = PollPages::default();
        for _ in 1..POLL_PAGES {
            read.take(votes(), Some("next".into()));
        }
        assert_eq!(read.take(votes(), None), None);
        assert!(!read.cut_off);
    }

    #[tokio::test]
    async fn a_verification_given_up_while_its_request_is_on_the_way_ends_there() {
        use super::{VerifyInput, mpsc, unless_given_up};
        // The server takes its time over the request, and the user
        // presses Esc: over at once, not when the server answers.
        let (tx, mut inputs) = mpsc::unbounded_channel();
        tx.send(VerifyInput::Confirm).unwrap();
        tx.send(VerifyInput::Cancel).unwrap();
        let never = std::future::pending::<u8>();
        assert_eq!(unless_given_up(never, &mut inputs).await, None);
        // Nobody waits for it any more.
        let (tx, mut inputs) = mpsc::unbounded_channel::<VerifyInput>();
        drop(tx);
        let never = std::future::pending::<u8>();
        assert_eq!(unless_given_up(never, &mut inputs).await, None);
        // Left alone, the request goes out; a key that answers nothing
        // yet does not end it.
        let (tx, mut inputs) = mpsc::unbounded_channel();
        tx.send(VerifyInput::Accept).unwrap();
        let soon = async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            7
        };
        assert_eq!(unless_given_up(soon, &mut inputs).await, Some(7));
    }

    #[test]
    fn an_audio_message_that_is_not_sound_reaches_no_player() {
        use super::{Opened, saved_name, sound_ending, way_to_open};
        // A list of addresses called `voice.ogg`: neither mpv nor the
        // desktop's player (nor the app) gets it.
        let list = b"#EXTM3U\nhttps://example.org/a.mp3";
        let name = saved_name("voice.ogg", "$1", sound_ending(list), true);
        assert!(name.ends_with("-voice.ogg"), "{name}");
        for hand_over in [false, true] {
            assert_eq!(way_to_open(&name, true, false, hand_over), Opened::Kept);
        }
        // A list that begins like sound, under a list's name: saved as
        // the sound it begins like, so that mpv does not read it by its
        // name.
        let name = saved_name("voice.m3u", "$1", sound_ending(b"OggS\nhttps://x"), true);
        assert!(name.ends_with("-voice.m3u.ogg"), "{name}");
        assert_eq!(way_to_open(&name, true, true, false), Opened::Playing);
        // Also under a name that is opened, but not as sound.
        let name = saved_name("voice.txt", "$1", sound_ending(b"OggS\0"), true);
        assert!(name.ends_with("-voice.txt.ogg"), "{name}");
        // And never played under a name mpv could read as a list.
        assert_eq!(way_to_open("voice.txt", true, true, false), Opened::Shown);
        assert_eq!(way_to_open("voice.m3u", true, true, false), Opened::Kept);
        // A voice message without an ending still plays.
        let name = saved_name("Voice message", "$1", sound_ending(b"OggS\0"), true);
        assert_eq!(way_to_open(&name, true, true, false), Opened::Playing);
    }

    #[test]
    fn completed_mentions_become_links_and_m_mentions() {
        let members = [
            member("@amy:x.org", "Amy"),
            member("@amyw:x.org", "Amy Winter"),
        ];
        let parts = MessageParts::new("hi @Amy Winter and @Amy!", &members);
        assert_eq!(parts.plain, "hi Amy Winter and Amy!");
        let html = parts.html.clone().expect("mentions make html");
        assert!(
            html.contains(r#"<a href="https://matrix.to/#/@amyw:x.org">Amy Winter</a>"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<a href="https://matrix.to/#/@amy:x.org">Amy</a>"#),
            "{html}"
        );
        let ids: Vec<String> = parts.mentioned.iter().map(ToString::to_string).collect();
        assert_eq!(ids, ["@amyw:x.org", "@amy:x.org"]);

        let json = serde_json::to_value(parts.into_content()).expect("serializable");
        assert_eq!(
            json["m.mentions"]["user_ids"],
            serde_json::json!(["@amy:x.org", "@amyw:x.org"])
        );
        assert_eq!(json["format"], "org.matrix.custom.html");
    }

    #[test]
    fn a_user_id_cannot_put_a_link_or_html_into_a_mention() {
        assert_eq!(super::in_address("@amy:x.org"), "@amy:x.org");
        assert_eq!(
            super::in_address("@a)<b>*[ä%:x.org"),
            "@a%29%3Cb%3E%2A%5B%C3%A4%25:x.org"
        );
        let id = "@a)[pay-here](//evil.example/x)<img src=x>:evil.example";
        let parts = MessageParts::new("hi @Amy", &[member(id, "Amy")]);
        // The id is one the SDK takes, so the mention is made.
        assert_eq!(parts.mentioned.len(), 1, "the id must count as one");
        let html = parts.html.expect("mentions make html");
        assert_eq!(html.matches("<a ").count(), 1, "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(
            html.contains(
                "href=\"https://matrix.to/#/@a%29%5Bpay-here%5D%28//evil.example/x%29%3Cimg%20src=x%3E:evil.example\""
            ),
            "{html}"
        );
    }

    #[test]
    fn links_are_previewed_only_for_the_open_room_when_it_is_not_encrypted() {
        use super::previews_links;
        assert!(previews_links(Some(("!a:x", true)), "!a:x"));
        // Encrypted, or not known yet.
        assert!(!previews_links(Some(("!a:x", false)), "!a:x"));
        // Another account's room is open, and this one has none open or
        // another one: its homeserver is not asked.
        assert!(!previews_links(None, "!a:x"));
        assert!(!previews_links(Some(("!b:x", true)), "!a:x"));
    }

    #[test]
    fn a_mention_that_was_deleted_again_is_not_sent() {
        let parts = MessageParts::new("never mind", &[member("@amy:x.org", "Amy")]);
        assert!(parts.mentioned.is_empty());
        assert!(parts.html.is_none());
    }

    #[test]
    fn a_long_attachment_name_is_cut_to_one_that_can_be_saved() {
        // Longer than a file name may be (255 bytes), in letters of two
        // bytes each.
        let long = format!("{}.pdf", "ä".repeat(200));
        let saved = super::saved_name(&long, "$event", None, false);
        assert!(saved.len() < 200, "{}", saved.len());
        assert!(saved.ends_with(".pdf"));
        assert!(super::opened_by_itself(&saved));
        // A name without an ending, and one that fits, as it is.
        assert_eq!(safe_file_name(&"x".repeat(400)).len(), super::MAX_FILE_NAME);
        assert_eq!(safe_file_name("notes.txt"), "notes.txt");
    }

    #[test]
    fn a_downloaded_file_s_name_cannot_leave_its_folder() {
        assert_eq!(safe_file_name("cat picture-1.png"), "cat picture-1.png");
        assert_eq!(safe_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_file_name("a\\b:c"), "a_b_c");
        assert_eq!(safe_file_name(".bashrc"), "bashrc");
        assert_eq!(safe_file_name(" .. "), "attachment");
        assert_eq!(safe_file_name(""), "attachment");
        assert_eq!(safe_file_name("grüße.txt"), "grüße.txt");
    }

    #[test]
    fn only_web_and_mail_addresses_are_opened() {
        assert!(desktop_opens("https://example.org/a"));
        assert!(desktop_opens("http://example.org"));
        assert!(desktop_opens("mailto:amy@example.org"));
        assert!(!desktop_opens("file:///etc/passwd"));
        assert!(!desktop_opens("javascript:alert(1)"));
        assert!(!desktop_opens(" https://example.org"));
        assert!(!desktop_opens("/usr/bin/true"));
        // Nothing a shell or an opener could take for a second argument.
        assert!(!desktop_opens("https://example.org/ --flag"));
        assert!(!desktop_opens("https://example.org/\"&calc"));
        assert!(!desktop_opens("https://example.org/\u{1b}[2J"));
        // Nor what a browser reads differently from how it looks, or
        // what is no address.
        assert!(!desktop_opens("https://evil.example\\@bank.example/login"));
        assert!(!desktop_opens("https://"));
        assert!(!desktop_opens("mailto:"));
    }

    #[test]
    fn an_identity_reset_opens_only_a_web_page_the_server_names() {
        assert!(super::approval_opens(
            "https://account.example.org/reset?x=1"
        ));
        for address in [
            "file://host/share/x.exe",
            "smb://host/share",
            "ms-msdt:/id",
            "http://account.example.org/reset",
            "mailto:amy@example.org",
            "https://evil.example\\@account.example.org/",
        ] {
            assert!(!super::approval_opens(address), "{address}");
        }
    }

    #[test]
    fn backup_is_set_up_once_and_renewed_only_when_asked() {
        use RecoveryState::{Disabled, Enabled, Incomplete, Unknown};
        assert!(matches!(backup_step(Enabled, false), BackupStep::Say(_)));
        assert!(matches!(backup_step(Incomplete, false), BackupStep::Say(_)));
        assert_ne!(backup_step(Enabled, false), backup_step(Incomplete, false));
        assert_eq!(backup_step(Enabled, true), BackupStep::NewKey);
        assert_eq!(backup_step(Incomplete, true), BackupStep::NewKey);
        for renew in [false, true] {
            assert_eq!(backup_step(Disabled, renew), BackupStep::SetUp);
            assert_eq!(backup_step(Unknown, renew), BackupStep::SetUp);
        }
    }

    #[test]
    fn emotes_keep_their_type() {
        let json = serde_json::to_value(MessageParts::new("/me waves", &[]).into_content())
            .expect("serializable");
        assert_eq!(json["msgtype"], "m.emote");
        assert_eq!(json["body"], "waves");
    }

    #[test]
    fn poll_start_uses_the_unstable_msc3381_shape() {
        let answers = vec!["Pizza".to_owned(), " ".to_owned(), "Sushi ".to_owned()];
        let content = build_poll_start(" Lunch? ", &answers, true, true).expect("valid poll");
        let json = serde_json::to_value(&content).expect("serializable");
        let poll = &json["org.matrix.msc3381.poll.start"];
        assert_eq!(poll["question"]["org.matrix.msc1767.text"], "Lunch?");
        assert_eq!(poll["kind"], "org.matrix.msc3381.poll.undisclosed");
        assert_eq!(poll["max_selections"], 2);
        assert_eq!(poll["answers"].as_array().expect("answers").len(), 2);
        assert_eq!(
            json["org.matrix.msc1767.text"],
            "Lunch?\n1. Pizza\n2. Sushi"
        );

        assert!(build_poll_start("Q", &["only one".to_owned()], false, false).is_err());
        assert!(build_poll_start(" ", &answers, false, false).is_err());
    }

    #[test]
    fn server_is_taken_from_a_matrix_id() {
        assert_eq!(server_from_input("@amy:example.org"), "example.org");
        assert_eq!(server_from_input(" matrix.org "), "matrix.org");
        assert_eq!(
            server_from_input("https://m.example.org"),
            "https://m.example.org"
        );
        assert_eq!(server_from_input("@a:b:8448"), "b:8448");
    }
}

#[cfg(test)]
mod lock_tests {
    use super::{Paths, hold};

    #[test]
    fn a_profile_in_use_is_kept_from_other_processes() {
        let dir = std::env::temp_dir().join(format!("mothrix-lock-test-{}", std::process::id()));
        let paths = Paths {
            config_file: dir.join("config.toml"),
            data_dir: dir.join("work"),
        };
        hold(&paths).unwrap();
        // Again from the same process (signing in anew): fine.
        hold(&paths).unwrap();
        // Another process opens the file on its own and finds it taken.
        let other = std::fs::File::open(paths.data_dir.join("lock")).unwrap();
        assert!(matches!(
            other.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn profile(name: &str) -> (std::path::PathBuf, Paths) {
        let dir = std::env::temp_dir().join(format!("mothrix-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths {
            config_file: dir.join("config.toml"),
            data_dir: dir.join("work"),
        };
        std::fs::create_dir_all(paths.store_dir()).unwrap();
        (dir, paths)
    }

    #[tokio::test]
    async fn a_damaged_session_file_is_still_an_account_and_its_store_is_not_deleted() {
        let (dir, paths) = profile("damaged");
        // The file was cut short (a full disk, an editor); the store
        // beside it holds the device's keys.
        std::fs::write(paths.session_file(), r#"{"homeserver": "https://exa"#).unwrap();
        let keys = paths.store_dir().join("keys");
        std::fs::write(&keys, "secret").unwrap();
        assert!(super::has_session(&paths));
        assert_eq!(super::signed_in_as(&paths), None);
        // Starting says what is wrong, and signing in under that name is
        // refused: neither deletes anything.
        let error = super::restore(&paths).await.expect_err("not restored");
        assert!(format!("{error:#}").contains("session"), "{error:#}");
        let signed_in = super::sign_in(
            &paths,
            "example.invalid",
            true,
            || Ok(("amy".to_owned(), "pw".to_owned())),
            |_| {},
        )
        .await;
        let error = signed_in.expect_err("refused");
        assert!(error.to_string().contains("already an account"), "{error}");
        assert_eq!(std::fs::read(&keys).unwrap(), b"secret");
        assert!(paths.session_file().exists());
        assert!(!super::holds(&paths));
        // `mothrix logout` (and removing a left-out account in an app)
        // is how it goes.
        super::logout(&paths).await.unwrap();
        assert!(!paths.session_file().exists() && !paths.store_dir().exists());
        assert!(!super::holds(&paths));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_account_that_does_not_start_lets_go_of_its_profile() {
        let (dir, paths) = profile("unstarted");
        // Its store is locked in a way this mothrix cannot open (the
        // keyring's answer failing goes the same way).
        let session = serde_json::json!({
            "homeserver": "https://example.invalid",
            "session": {
                "user_id": "@amy:example.invalid",
                "device_id": "DEVICE",
                "access_token": "token",
            },
            "store_key": "elsewhere",
        });
        std::fs::write(paths.session_file(), session.to_string()).unwrap();
        let error = super::restore(&paths).await.expect_err("not restored");
        assert!(error.to_string().contains("does not know"), "{error:#}");
        // Another mothrix, which may be able to open it, is not kept out.
        assert!(!super::holds(&paths));
        let lock = std::fs::File::options()
            .write(true)
            .open(paths.data_dir.join(super::LOCK))
            .unwrap();
        assert!(lock.try_lock().is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
