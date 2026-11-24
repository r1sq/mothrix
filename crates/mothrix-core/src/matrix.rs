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

/// The Matrix id a profile is signed in as, read from its session file.
pub fn signed_in_as(paths: &Paths) -> Option<String> {
    let stored: StoredSession =
        serde_json::from_slice(&std::fs::read(paths.session_file()).ok()?).ok()?;
    Some(stored.session.meta.user_id.to_string())
}

#[derive(Serialize, Deserialize)]
struct StoredSession {
    homeserver: String,
    session: MatrixSession,
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

/// Tell the UI what the account stores of its sections: first as the
/// server has it (or, offline, the local store), then each change a sync
/// brings into the local store.
async fn watch_sections(client: Client, tx: mpsc::UnboundedSender<Update>) {
    use matrix_sdk::ruma::events::GlobalAccountDataEventType;
    let kind = GlobalAccountDataEventType::from(crate::sections::EVENT_TYPE);
    let json = |raw: Option<Raw<matrix_sdk::ruma::events::AnyGlobalAccountDataEventContent>>| {
        raw.map(|raw| raw.json().get().to_owned())
    };
    let parse = |json: &Option<String>| {
        json.as_deref()
            .and_then(|json| serde_json::from_str::<crate::sections::Stored>(json).ok())
            .map(|stored| stored.sections)
    };
    let first = match client.account().fetch_account_data(kind.clone()).await {
        Ok(raw) => json(raw),
        Err(_) => json(
            client
                .account()
                .account_data_raw(kind.clone())
                .await
                .ok()
                .flatten(),
        ),
    };
    let _ = tx.send(Update::Sections(parse(&first)));
    let mut last = first;
    let mut tick = tokio::time::interval(Duration::from_secs(4));
    loop {
        tick.tick().await;
        if tx.is_closed() {
            break;
        }
        let now = json(
            client
                .account()
                .account_data_raw(kind.clone())
                .await
                .ok()
                .flatten(),
        );
        if now.is_some() && now != last {
            let _ = tx.send(Update::Sections(parse(&now)));
            last = now;
        }
    }
}

/// Keep `sections` (this account's share) in the account data.
async fn store_sections(client: &Client, sections: Vec<Section>) -> Result<()> {
    use matrix_sdk::ruma::events::GlobalAccountDataEventType;
    let content = serde_json::value::to_raw_value(&crate::sections::Stored { sections })?;
    client
        .account()
        .set_account_data_raw(
            GlobalAccountDataEventType::from(crate::sections::EVENT_TYPE),
            Raw::from_json(content),
        )
        .await?;
    Ok(())
}

/// Keep other mothrix processes away from a profile while this one uses
/// it: two at once on one store would mix up its encryption keys. Held
/// until the process ends. Not when stores are shared ([`share_stores`]).
fn hold(paths: &Paths) -> Result<()> {
    if SHARED_AS.get().is_some() {
        return Ok(());
    }
    static HELD: std::sync::Mutex<Vec<(PathBuf, std::fs::File)>> =
        std::sync::Mutex::new(Vec::new());
    let mut held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = paths.data_dir.join("lock");
    if held.iter().any(|(known, _)| *known == path) {
        return Ok(());
    }
    std::fs::create_dir_all(&paths.data_dir)?;
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

async fn build_client(paths: &Paths, homeserver: &str) -> Result<Client> {
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
        .sqlite_store(&store, None);
    let builder = match SHARED_AS.get() {
        Some(holder) => builder.cross_process_store_config(
            matrix_sdk::cross_process_lock::CrossProcessLockConfig::multi_process(holder.clone()),
        ),
        None => builder,
    };
    // Networks without a direct way out (school labs) have a proxy.
    let builder = match crate::config::Config::load(&paths.config_file).proxy() {
        Some(proxy) => builder.proxy(proxy),
        None => builder,
    };

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

fn save_session(paths: &Paths, client: &Client) -> Result<()> {
    let session = client
        .matrix_auth()
        .session()
        .context("login produced no session")?;
    let stored = StoredSession {
        homeserver: client.homeserver().to_string(),
        session,
    };
    std::fs::create_dir_all(&paths.data_dir)?;
    let file = paths.session_file();
    std::fs::write(&file, serde_json::to_vec_pretty(&stored)?)?;
    // The file holds the access token: owner-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Restore the saved session, if there is one.
pub async fn restore(paths: &Paths) -> Result<Option<Client>> {
    let file = paths.session_file();
    if !file.exists() {
        return Ok(None);
    }
    let stored: StoredSession =
        serde_json::from_slice(&std::fs::read(&file)?).context("session file is corrupt")?;
    hold(paths)?;
    let client = build_client(paths, &stored.homeserver).await?;
    client
        .restore_session(stored.session)
        .await
        .context("could not restore the saved session")?;
    share_encryption_store(&client).await?;
    Ok(Some(client))
}

/// Sign a running account out and delete what it kept on this computer.
pub async fn sign_out(client: Client, paths: Paths) {
    let _ = client.matrix_auth().logout().await;
    let _ = std::fs::remove_dir_all(&paths.data_dir);
}

/// Remove the saved session and local store (does not sign the device out
/// server-side when the server is unreachable).
pub async fn logout(paths: &Paths) -> Result<()> {
    if let Ok(Some(client)) = restore(paths).await {
        let _ = client.matrix_auth().logout().await;
    }
    if paths.data_dir.exists() {
        std::fs::remove_dir_all(&paths.data_dir)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Login (plain terminal prompts, before the TUI starts)
// ---------------------------------------------------------------------------

/// Hand a file or web address to the desktop's default application.
fn open_with_desktop(target: &std::ffi::OsStr) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        // `start` is part of the shell; the empty string is the window title.
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
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
        |line| println!("{line}"),
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

    // A previous half-finished login would leave a crypto store belonging to
    // another device behind; start clean.
    if paths.store_dir().exists() {
        std::fs::remove_dir_all(paths.store_dir())?;
    }
    std::fs::create_dir_all(&paths.data_dir)?;

    hold(paths)?;
    let client = build_client(paths, server_from_input(server)).await?;
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

    save_session(paths, &client)?;
    share_encryption_store(&client).await?;
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
    println!("# {name}  ({} events, newest first; text is masked)", limit);

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
        println!(
            "{}  from {}\n    {}",
            value["type"].as_str().unwrap_or("?"),
            convert::localpart(sender),
            shape_of(&value["content"]),
        );
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
    /// What the account stores of the user's own sections ([`crate::sections`]);
    /// `None`: nothing yet.
    Sections(Option<Vec<Section>>),
    /// An attachment asked for with [`Command::OpenMedia`] is on disk, for
    /// the app to show ([`hand_media_over`]).
    MediaFile {
        event_id: String,
        path: String,
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
            | Self::SendFile { .. }
            | Self::SendVoice { .. }
            | Self::PasteImage
            | Self::ClipboardPaste
            | Self::Reveal { .. }
            | Self::VerifyStart
            | Self::VerifyUser { .. }
            | Self::Verify(_)
            | Self::Delete { .. }
            | Self::React { .. }
            | Self::Paginate
            | Self::CreatePoll { .. }
            | Self::Vote { .. }
            | Self::EndPoll { .. }
            | Self::TogglePin { .. }
            | Self::PreviewLink(_)
            | Self::StopAudio
            | Self::OpenLink(_)
            | Self::OpenMedia { .. }
            | Self::StartDm { .. }
            | Self::CreateRoom { .. }
            | Self::Join { .. }
            | Self::Recover(_)
            | Self::SetSections(_)
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
    Send {
        room_id: String,
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
    /// Upload a local file into the open room.
    SendFile {
        path: String,
        /// Text to show with it.
        caption: Option<String>,
    },
    /// Upload a recorded voice message into the open room (MSC3245):
    /// its length and loudness over time (values 0 to 1).
    SendVoice {
        path: String,
        duration_ms: u64,
        waveform: Vec<f32>,
    },
    /// Send the picture that is on the system clipboard.
    PasteImage,
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
    CreatePoll {
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
    /// Ask the homeserver what is behind a web address (title, summary).
    PreviewLink(String),
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
    /// data ([`crate::sections`]).
    SetSections(Vec<Section>),
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

struct OpenRoom {
    room_id: String,
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
    tokio::spawn(watch_sections(client.clone(), tx.clone()));

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
    let mut session = Session {
        client: client.clone(),
        sync,
        own_user_id,
        open: None,
        verifying: None,
        tx: tx.clone(),
        account,
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
        let path = expand_home(path);
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
pub async fn send_text(client: &Client, room_id: &str, text: &str) -> Result<()> {
    let room = room_of(client, room_id)?;
    let message = MessageParts::new(text, &[]);
    room.send(message.into_content())
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
    let source = match msgtype {
        MessageType::Image(content) => &content.source,
        MessageType::File(content) => &content.source,
        MessageType::Audio(content) => &content.source,
        MessageType::Video(content) => &content.source,
        _ => return false,
    };
    matches!(source, MediaSource::Encrypted(_))
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
                    // Not worth a message in the status bar if it fails.
                    let _ = current.room.typing_notice(typing).await;
                }
            }
            Command::OpenThread { event_id } => {
                let current = open.take().ok_or_else(|| anyhow!("open a room first"))?;
                current.task.abort();
                let view = if event_id == PINNED_VIEW {
                    View::Pinned
                } else {
                    View::Thread(EventId::parse(&event_id)?)
                };
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
                text,
                mentions,
            } => {
                let current = open.as_ref().filter(|current| current.room_id == room_id);
                let current = current.ok_or_else(|| anyhow!("that room is no longer open"))?;
                let message = MessageParts::new(&text, &mentions);
                current
                    .timeline
                    .send(message.into_content().into())
                    .await
                    .context("could not send the message")?;
            }
            Command::SendFile { path, caption } => {
                let timeline = timeline_of(open)?;
                let file = Attachment::prepare(&path, caption).await?;
                let _ = tx.send(Update::Notice(format!("Sending {}…", file.name)));
                timeline
                    .send_attachment(AttachmentSource::File(file.path), file.mime, file.config)
                    .use_send_queue()
                    .await
                    .with_context(|| format!("could not send {}", file.name))?;
            }
            Command::SendVoice {
                path,
                duration_ms,
                waveform,
            } => {
                let timeline = timeline_of(open)?;
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
            Command::PasteImage => {
                let timeline = timeline_of(open)?;
                let (bytes, mime_type) = tokio::task::spawn_blocking(read_clipboard_image)
                    .await
                    .context("could not read the clipboard")??;
                let extension = mime_type.rsplit('/').next().unwrap_or("png");
                let seconds = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or_default();
                let filename = format!("pasted-{seconds}.{extension}");
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
            }
            Command::ClipboardPaste => {
                let clip = tokio::task::spawn_blocking(|| match read_clipboard_image() {
                    Ok(image) => Ok(Ok(image)),
                    Err(no_image) => read_clipboard_text().map(Err).ok_or(no_image),
                })
                .await
                .context("could not read the clipboard")??;
                match clip {
                    Ok((bytes, mime_type)) => {
                        let extension = mime_type.rsplit('/').next().unwrap_or("png");
                        let seconds = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|elapsed| elapsed.as_secs())
                            .unwrap_or_default();
                        let dir = crate::folders::cache()
                            .context("no cache directory on this system")?
                            .join("pasted");
                        std::fs::create_dir_all(&dir)?;
                        let file = dir.join(format!("pasted-{seconds}.{extension}"));
                        std::fs::write(&file, bytes)?;
                        let _ = tx.send(Update::PastedImage(file.display().to_string()));
                    }
                    Err(text) => {
                        let _ = tx.send(Update::PastedText(text));
                    }
                }
            }
            Command::VerifyStart => {
                if verifying.as_ref().is_some_and(|inputs| !inputs.is_closed()) {
                    bail!("a verification is already running");
                }
                let _ = tx.send(Update::Verify(Some(VerifyState::Waiting)));
                match verify::request(client).await {
                    Ok(request) => {
                        *verifying = Some(spawn_verification(client, request, false, tx))
                    }
                    Err(error) => {
                        let _ = tx.send(Update::Verify(Some(VerifyState::Failed(format!(
                            "{error:#}"
                        )))));
                    }
                }
            }
            Command::VerifyUser { user_id } => {
                if verifying.as_ref().is_some_and(|inputs| !inputs.is_closed()) {
                    bail!("a verification is already running");
                }
                let user = parse_user(&user_id)?;
                let _ = tx.send(Update::Verify(Some(VerifyState::Waiting)));
                match verify::request_user(client, &user).await {
                    Ok(request) => {
                        *verifying = Some(spawn_verification(client, request, false, tx))
                    }
                    Err(error) => {
                        let _ = tx.send(Update::Verify(Some(VerifyState::Failed(format!(
                            "{error:#}"
                        )))));
                    }
                }
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
                        timeline
                            .room()
                            .send_raw(event_type, content)
                            .await
                            .context("could not send the vote")?;
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
                question,
                answers,
                multiple,
                hidden,
            } => {
                let content = build_poll_start(&question, &answers, multiple, hidden)?;
                timeline_of(open)?
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
                        timeline
                            .room()
                            .send_raw(event_type, content)
                            .await
                            .context("could not end the poll")?;
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
                // The same content (for files: the same upload), without the
                // reply or thread it belonged to.
                let msgtype = message.msgtype();
                let _ = tx.send(Update::ToForward {
                    to: room_id,
                    content: serde_json::to_string(msgtype)?,
                    encrypted_file: holds_encrypted_file(msgtype),
                });
            }
            Command::PreviewLink(url) => {
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
                        Ok((path, _))
                            if HAND_MEDIA_OVER.load(std::sync::atomic::Ordering::Relaxed) =>
                        {
                            Update::MediaFile {
                                event_id,
                                path: path.display().to_string(),
                            }
                        }
                        Ok((_, true)) => Update::Notice("Playing; /stop ends it.".to_owned()),
                        Ok((path, false)) => Update::Notice(format!("Opened {}", path.display())),
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
            Command::SetSections(sections) => {
                let client = client.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = store_sections(&client, sections).await {
                        let _ = tx.send(Update::Notice(format!(
                            "Your sections could not be kept in the account: {error:#}"
                        )));
                    }
                });
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
                        let _ = open_with_desktop(url.as_ref());
                        let _ = tx.send(Update::Notice(format!(
                            "Approve the reset in your browser: {url}"
                        )));
                        handle
                            .reset(None)
                            .await
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

/// Whether an address is one a browser or mail program is meant to get.
fn desktop_opens(address: &str) -> bool {
    ["https://", "http://", "mailto:"]
        .iter()
        .any(|scheme| address.starts_with(scheme))
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

fn expand_home(path: &str) -> PathBuf {
    let path = path.trim();
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| path.into()),
        None => path.into(),
    }
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
        let mut plain = text.to_owned();
        let mut source = text.to_owned();
        let mut mentioned = Vec::new();

        // Longest names first, so "@Amy Winter" is not cut short by "@Amy".
        let mut by_length: Vec<&Member> = mentions.iter().collect();
        by_length.sort_by_key(|member| std::cmp::Reverse(member.name.len()));
        for member in by_length {
            let tag = format!("@{}", member.name);
            let Ok(user_id) = UserId::parse(&member.user_id) else {
                continue;
            };
            if !source.contains(&tag) || mentioned.contains(&user_id) {
                continue;
            }
            let label = member.name.replace('[', "\\[").replace(']', "\\]");
            source = source.replace(
                &tag,
                &format!("[{label}](https://matrix.to/#/{})", member.user_id),
            );
            plain = plain.replace(&tag, &member.name);
            mentioned.push(user_id);
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
        .args(["--no-video", "--really-quiet", "--no-terminal"])
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

/// Download an attachment and open it. Returns where it was saved, and
/// whether it is being played here (audio, with mpv) rather than handed
/// to the desktop.
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
    match safe.trim_matches(|c| c == '.' || c == ' ') {
        "" => "attachment".to_owned(),
        safe => safe.to_owned(),
    }
}

async fn open_media(
    client: &Client,
    timeline: &Timeline,
    event_id: &str,
) -> Result<(PathBuf, bool)> {
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
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(safe_file_name(&name));
    std::fs::write(&path, bytes)?;

    if HAND_MEDIA_OVER.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok((path, false));
    }
    if matches!(message.msgtype(), MessageType::Audio(_)) && play_audio(&path) {
        return Ok((path, true));
    }
    open_with_desktop(path.as_os_str())
        .with_context(|| format!("saved to {}, but could not start a viewer", path.display()))?;
    Ok((path, false))
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
    let mut watch = RoomWatch::new(thread.is_none());
    let (poll_loaded_tx, mut poll_loaded) = mpsc::unbounded_channel::<(String, Vec<Value>)>();
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
        // A receipt that did not go out is sent again with the next
        // snapshot (the loop comes round with every change in the room).
        if todo.mark_read && timeline.mark_as_read(ReceiptType::Read).await.is_err() {
            watch.read_failed();
        }
        if let Some(newest) = todo
            .read_with_threads
            .and_then(|id| OwnedEventId::try_from(id).ok())
            && room
                .send_single_receipt(ReceiptType::Read, ReceiptThread::Unthreaded, newest)
                .await
                .is_err()
        {
            watch.read_with_threads_failed();
        }
        if todo.clear_unread_flag && room.is_marked_unread() {
            let _ = room.set_unread_flag(false).await;
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
                watch.poll_loaded(&poll_id, &events);
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
                Nudge::PollChanged(poll_id) if watch.tracks(&poll_id) => {
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
    let mut seen = Seen::default();
    let mut pictures = HashMap::new();
    for event in items.iter().filter_map(|item| item.as_event()) {
        let event_id = event.event_id().map(ToString::to_string);
        if let Some(id) = &event_id {
            seen.newest = Some(id.clone());
            seen.with_threads
                .push((event.timestamp().get().into(), id.clone()));
        }
        // The newest reply of a thread that starts here.
        if let TimelineItemContent::MsgLike(content) = event.content()
            && let Some(summary) = &content.thread_summary
            && let TimelineDetails::Ready(reply) = &summary.latest_event
            && let TimelineEventItemId::EventId(id) = &reply.identifier
        {
            seen.with_threads
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
/// server and hand them to the timeline task as raw JSON.
async fn load_poll_events(
    room: Room,
    poll_id: String,
    done: mpsc::UnboundedSender<(String, Vec<Value>)>,
) {
    let Ok(event_id) = EventId::parse(&poll_id) else {
        return;
    };
    let mut events = Vec::new();
    let mut from = None;
    // Polls with thousands of votes are not a thing; cap the paging anyway.
    for _ in 0..40 {
        let options = RelationsOptions {
            from: from.clone(),
            include_relations: IncludeRelations::RelationsOfType(RelationType::Reference),
            ..Default::default()
        };
        let Ok(page) = room.relations(event_id.clone(), options).await else {
            break;
        };
        for event in &page.chunk {
            if let Ok(value) = serde_json::from_str::<Value>(event.raw().json().get()) {
                events.push(value);
            }
        }
        match page.next_batch_token {
            Some(token) => from = Some(token),
            None => break,
        }
    }
    let _ = done.send((poll_id, events));
}

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
    fn a_mention_that_was_deleted_again_is_not_sent() {
        let parts = MessageParts::new("never mind", &[member("@amy:x.org", "Amy")]);
        assert!(parts.mentioned.is_empty());
        assert!(parts.html.is_none());
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
}
