// SPDX-License-Identifier: MPL-2.0

//! mothrix-core for the iOS app, through UniFFI. The interface is small on
//! purpose: the app starts a [`Mothrix`] on its folder, sends what the user
//! does as JSON actions ([`view::Action`]) and gets what changed as JSON
//! events ([`view::Event`], plus `snapshot` and `notify`) through its
//! [`Listener`]. All state is kept here, in [`view::View`], and tested
//! on plain data; the Swift side draws and asks.
//!
//! The notification extension calls [`notification`]: a process of its
//! own that shares the app's store (`matrix::share_stores`).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::Engine;
use mothrix_core::{
    accounts::{self, Accounts},
    config::Config,
    folders,
    matrix::{self, Command, Paths},
};
use tokio::sync::mpsc;
use view::{Action, Effect, View};

mod rich;
mod view;

uniffi::setup_scaffolding!();

/// Where the app hears what changed: one JSON object per call, with its
/// kind in `"type"`.
#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    fn on_event(&self, json: String);
}

#[derive(Debug, uniffi::Error)]
pub enum MothrixError {
    /// What went wrong, for showing.
    Failed { message: String },
}

impl std::fmt::Display for MothrixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for MothrixError {}

impl From<anyhow::Error> for MothrixError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed {
            message: format!("{error:#}"),
        }
    }
}

/// The accounts, running. One per app process.
#[derive(uniffi::Object)]
pub struct Mothrix {
    requests: Requests,
    /// Runs the accounts' task; dropped with the app's last reference.
    _runtime: tokio::runtime::Runtime,
}

#[uniffi::export]
impl Mothrix {
    /// Start every signed-in account kept under `folder` (the app group's
    /// folder, shared with the notification extension). The first events
    /// follow at once, a `snapshot` among them.
    #[uniffi::constructor]
    pub fn new(folder: String, listener: Arc<dyn Listener>) -> Result<Arc<Self>, MothrixError> {
        let runtime = start(Path::new(&folder), "app")?;
        let (sender, incoming) = mpsc::unbounded_channel();
        let requests = Requests(sender);
        runtime.spawn(run(listener, requests.clone(), incoming));
        requests.ask(Request::Snapshot);
        Ok(Arc::new(Self {
            requests,
            _runtime: runtime,
        }))
    }

    /// Something the user did, as a JSON action (`{"type": "send", …}`).
    pub fn act(&self, json: String) -> Result<(), MothrixError> {
        let action: Action = serde_json::from_str(&json).map_err(|error| MothrixError::Failed {
            message: format!("not an action: {error}"),
        })?;
        if cfg!(debug_assertions) {
            tracing::warn!("action {json}");
        }
        self.requests.ask(Request::Act(action));
        Ok(())
    }

    /// Ask for everything there is to show, as a `snapshot` event (after
    /// the app's views were rebuilt, say).
    pub fn refresh(&self) {
        self.requests.ask(Request::Snapshot);
    }

    /// The app came to the front (true) or went to the background.
    pub fn focus(&self, focused: bool) {
        self.requests.ask(Request::Focus(focused));
    }

    /// A notification about this room (its key) was tapped.
    pub fn open_from_notification(&self, room: String) {
        self.requests.ask(Request::Clicked(room));
    }
}

/// What a notification shows.
#[derive(Debug, uniffi::Record)]
pub struct NotificationContent {
    /// The room's name, or for a direct chat the sender's.
    pub title: String,
    /// Who wrote it, in a room; empty in a direct chat.
    pub subtitle: String,
    pub body: String,
    /// The room's key, for opening it when the notification is tapped.
    pub room: String,
    pub thread_root: Option<String>,
    pub noisy: bool,
}

/// For the notification extension: fetch and decrypt the event a push was
/// about, for the account `profile`, from the store under `folder`. None:
/// nothing to show (the push rules filter it out, or it is gone).
#[uniffi::export]
pub fn notification(
    folder: String,
    profile: String,
    room_id: String,
    event_id: String,
) -> Result<Option<NotificationContent>, MothrixError> {
    let runtime = start(Path::new(&folder), "nse")?;
    let content: anyhow::Result<_> = runtime.block_on(async {
        let client = signed_in(&profile).await?;
        let found = mothrix_core::push::notification(&client, &room_id, &event_id).await?;
        Ok(found.map(|found| NotificationContent {
            title: if found.direct {
                found.sender.clone()
            } else {
                found.room_name.clone()
            },
            subtitle: if found.direct {
                String::new()
            } else {
                found.sender
            },
            body: found.body,
            room: format!("{}{room_id}", accounts::prefix_for(&profile)),
            thread_root: found.thread_root,
            noisy: found.noisy,
        }))
    });
    Ok(content?)
}

/// Which mothrix this is: "0.2.0", or with the commit for a build that is
/// not the release (`crates/build-version.rs`); for the app's menu and
/// its logs.
#[uniffi::export]
pub fn mothrix_version() -> String {
    env!("MOTHRIX_VERSION").to_owned()
}

/// Where mothrix's source is (Cargo.toml's `repository`), for the app's
/// menu.
#[uniffi::export]
pub fn mothrix_repository() -> String {
    env!("CARGO_PKG_REPOSITORY").to_owned()
}

/// For a notification's actions when the app is not running: send `text`
/// (if any) into the room with the key `room` and read it up to the
/// newest message, from the store under `folder`.
#[uniffi::export]
pub fn respond(folder: String, room: String, text: Option<String>) -> Result<(), MothrixError> {
    let runtime = start(Path::new(&folder), "action")?;
    let done: anyhow::Result<()> = runtime.block_on(async {
        let (profile, room_id) = accounts::split_by_profile(&matrix::signed_in_profiles(), &room);
        let client = signed_in(&profile).await?;
        if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
            matrix::send_text(&client, room_id, &text).await?;
        }
        if let Some(room) = client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(room_id)?) {
            matrix::read_newest(&room).await;
        }
        Ok(())
    });
    Ok(done?)
}

/// For the share extension: send `text` (if any) and the files at `files`
/// (the text as their caption when there are files) into the room with the
/// key `room`.
#[uniffi::export]
pub fn share(
    folder: String,
    room: String,
    text: Option<String>,
    files: Vec<String>,
) -> Result<(), MothrixError> {
    let runtime = start(Path::new(&folder), "share")?;
    let done: anyhow::Result<()> = runtime.block_on(async {
        let (profile, room_id) = accounts::split_by_profile(&matrix::signed_in_profiles(), &room);
        let client = signed_in(&profile).await?;
        let text = text.filter(|text| !text.trim().is_empty());
        if files.is_empty() {
            if let Some(text) = text {
                matrix::send_text(&client, room_id, &text).await?;
            }
        } else {
            matrix::send_files(&client, room_id, &files, text).await?;
        }
        Ok(())
    });
    Ok(done?)
}

async fn signed_in(profile: &str) -> anyhow::Result<matrix_sdk::Client> {
    let paths = Paths::for_profile(profile)?;
    matrix::restore(&paths)
        .await?
        .ok_or_else(|| anyhow::anyhow!("account '{profile}' is not signed in"))
}

/// Keep everything under `folder`, share stores under `holder`, log to
/// `mothrix-<holder>.log` there, and make the runtime the accounts run on.
fn start(folder: &Path, holder: &str) -> anyhow::Result<tokio::runtime::Runtime> {
    if !folders::set_root(folder) {
        anyhow::bail!("mothrix already keeps its files elsewhere");
    }
    matrix::share_stores(holder);
    matrix::hand_media_over();
    init_logging(folder, holder);
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

/// Diagnostics to `mothrix-<holder>.log` in the folder; warnings only.
fn init_logging(folder: &Path, holder: &str) {
    let _ = std::fs::create_dir_all(folder);
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(folder.join(format!("mothrix-{holder}.log")))
    {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .try_init();
    }
}

/// What reaches the accounts' task.
enum Request {
    Snapshot,
    Act(Action),
    /// The app came to the front or went to the background.
    Focus(bool),
    /// A notification about this room was tapped.
    Clicked(String),
    /// News from a sign-in under way.
    SignIn(SignInEvent),
}

enum SignInEvent {
    Progress(String),
    /// The profile, the name typed for it, and its client if it worked.
    Done(String, String, anyhow::Result<matrix_sdk::Client>),
}

#[derive(Clone)]
struct Requests(mpsc::UnboundedSender<Request>);

impl Requests {
    fn ask(&self, request: Request) {
        let _ = self.0.send(request);
    }
}

fn emit(listener: &dyn Listener, event: &impl serde::Serialize) {
    match serde_json::to_string(event) {
        Ok(json) => {
            // Debug builds: what went to the app, in the log.
            if cfg!(debug_assertions) {
                tracing::warn!(len = json.len(), "event {}", &json[..json.len().min(120)]);
                // And the last one of each kind whole, in debug/.
                if let (Some(root), Ok(value)) = (
                    folders::root(),
                    serde_json::from_str::<serde_json::Value>(&json),
                ) && let Some(kind) = value["type"].as_str()
                {
                    let dir = root.join("debug");
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join(format!("{kind}.json")), &json);
                }
            }
            listener.on_event(json)
        }
        Err(error) => tracing::warn!(%error, "an event could not be written"),
    }
}

fn config_file() -> Option<PathBuf> {
    Paths::for_profile("default")
        .ok()
        .map(|paths| paths.config_file)
}

/// Where files to send wait until they are sent.
fn upload_dir() -> PathBuf {
    folders::cache()
        .unwrap_or_else(std::env::temp_dir)
        .join("uploads")
}

/// The accounts' task: start them, then hand updates to the view and
/// the app's requests to the accounts, until the app goes.
async fn run(
    listener: Arc<dyn Listener>,
    requests: Requests,
    mut incoming: mpsc::UnboundedReceiver<Request>,
) {
    let config = config_file()
        .map(|file| Config::load(&file))
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(upload_dir());
    let (clients, problems) = restore_accounts(&config).await;
    let (mut accounts, list) = accounts::spawn(clients);
    let mut view = View::new(list, problems, &config);
    view.set_emoji_usage(mothrix_core::emoji::Usage::load());
    loop {
        // Effects can lead to more (signing out saves the config).
        loop {
            let effects = view.take_effects();
            if effects.is_empty() {
                break;
            }
            for effect in effects {
                carry_out(effect, &mut view, &mut accounts, &*listener, &requests);
            }
        }
        // Sections changed here go to the accounts that keep them.
        accounts.sync_sections(&mut view);
        let later = view.take_commands();
        send_all(&mut accounts, &view, later);
        for event in view.take_events() {
            emit(&*listener, &event);
        }
        tokio::select! {
            Some((id, update)) = accounts.updates.recv() => accounts.deliver(&mut view, id, update),
            request = incoming.recv() => {
                let Some(request) = request else { break };
                let commands = match request {
                    Request::Snapshot => {
                        emit(&*listener, &serde_json::json!({
                            "type": "snapshot",
                            "snapshot": view.snapshot(),
                        }));
                        Vec::new()
                    }
                    Request::Act(action) => view.act(action),
                    Request::Focus(focused) => {
                        view.focus(focused);
                        Vec::new()
                    }
                    Request::Clicked(room) => view.notification_clicked(room),
                    Request::SignIn(event) => {
                        finish_sign_in(event, &mut view, &mut accounts);
                        Vec::new()
                    }
                };
                send_all(&mut accounts, &view, commands);
            }
        }
    }
}

fn send_all(accounts: &mut Accounts, view: &View, commands: Vec<Command>) {
    for command in commands {
        accounts.send(view, command);
    }
}

fn carry_out(
    effect: Effect,
    view: &mut View,
    accounts: &mut Accounts,
    listener: &dyn Listener,
    requests: &Requests,
) {
    match effect {
        Effect::SaveConfig => {
            if let Some(file) = config_file()
                && let Err(error) = (Config {
                    theme: view.theme_name().to_owned(),
                    ..Config::load(&file).with_shown(view.shown())
                })
                .save(&file)
            {
                tracing::warn!(%error, "could not save the config");
            }
        }
        // The app decides: in the foreground a banner, in the background
        // the push does it.
        Effect::Notify { room, title } => emit(
            listener,
            &serde_json::json!({ "type": "notify", "room": room, "title": title }),
        ),
        Effect::SignIn {
            profile,
            label,
            server,
            user,
            password,
        } => start_sign_in(profile, label, server, user, password, requests.clone()),
        Effect::SignOut(account) => {
            if let Some((profile, Some(client))) = accounts.remove(view, account)
                && let Ok(paths) = Paths::for_profile(&profile)
            {
                tokio::spawn(matrix::sign_out(client, paths));
            }
        }
        Effect::Upload {
            name,
            data,
            caption,
        } => match keep_upload(&name, &data) {
            Ok(path) => accounts.send(
                view,
                Command::SendFile {
                    path: path.display().to_string(),
                    caption,
                },
            ),
            Err(error) => tracing::warn!(%error, "could not keep the file to send"),
        },
        Effect::UsedEmoji(emoji) => mothrix_core::emoji::Usage::store(&emoji),
    }
}

/// The file the app read, on disk under its own name, for sending.
fn keep_upload(name: &str, data: &str) -> anyhow::Result<PathBuf> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
    // Only the last part of the name, and nothing a path could be made of.
    let name: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let name = match name.trim_start_matches('.') {
        "" => "file".to_owned(),
        name => name.to_owned(),
    };
    // A folder per upload: two files of the same name do not meet.
    let dir = upload_dir().join(format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn start_sign_in(
    profile: String,
    label: String,
    server: String,
    user: String,
    password: String,
    requests: Requests,
) {
    tokio::spawn(async move {
        let progress = requests.clone();
        let result = async {
            let paths = Paths::for_profile(&profile)?;
            if matrix::signed_in_as(&paths).is_some() {
                anyhow::bail!("there is already an account called '{profile}'");
            }
            // Browser sign-in (SSO) is not there yet in the app: always
            // with the password.
            matrix::sign_in(
                &paths,
                &server,
                true,
                move || {
                    if user.is_empty() || password.is_empty() {
                        anyhow::bail!("fill in the user name and the password");
                    }
                    Ok((user, password))
                },
                move |line| progress.ask(Request::SignIn(SignInEvent::Progress(line))),
            )
            .await
        }
        .await;
        requests.ask(Request::SignIn(SignInEvent::Done(profile, label, result)));
    });
}

fn finish_sign_in(event: SignInEvent, view: &mut View, accounts: &mut Accounts) {
    match event {
        SignInEvent::Progress(line) => view.sign_in_progress(line),
        SignInEvent::Done(profile, label, Ok(client)) => {
            let mut account = accounts.add(profile, client);
            if !label.is_empty() {
                account.label = label;
            }
            view.sign_in_done(Ok(account));
        }
        SignInEvent::Done(profile, _, Err(error)) => {
            // Nothing of a failed sign-in is worth keeping.
            if let Ok(paths) = Paths::for_profile(&profile) {
                let _ = std::fs::remove_dir_all(&paths.data_dir);
            }
            view.sign_in_done(Err(format!("{error:#}")));
        }
    }
}

/// Every signed-in account, in the config's order. One that cannot start
/// is left out and named.
async fn restore_accounts(config: &Config) -> (Vec<(String, matrix_sdk::Client)>, Vec<String>) {
    let mut clients = Vec::new();
    let mut problems = Vec::new();
    let mut profiles = matrix::signed_in_profiles();
    config.order_profiles(&mut profiles);
    for profile in profiles {
        let restored = match Paths::for_profile(&profile) {
            Ok(paths) => matrix::restore(&paths).await,
            Err(error) => Err(error),
        };
        match restored {
            Ok(Some(client)) => clients.push((profile, client)),
            Ok(None) => {}
            Err(error) => problems.push(format!("{profile}: {error:#}")),
        }
    }
    (clients, problems)
}
