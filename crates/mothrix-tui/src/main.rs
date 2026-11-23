// SPDX-License-Identifier: MPL-2.0

//! mothrix: a terminal Matrix client with a room sidebar and full mouse support.

#[cfg(test)]
mod accounts_tests;
mod app;
mod config;
mod input;
mod keys;
mod kitty;
mod logo;
mod notify;
mod sixel;
mod theme;
mod ui;

// What lives in `mothrix-core`, under the same paths as the terminal's own
// modules.
use mothrix_core::{
    accounts, client_state, codeblock, emoji, inline, languagetool, matrix, model, search,
    sections, sidebar, spell, update, verify,
};

/// The version shown: "0.2.0", or with the commit for a build that is not
/// the release (`crates/build-version.rs`).
pub const VERSION: &str = env!("MOTHRIX_VERSION");

/// Where mothrix's source is (Cargo.toml's `repository`), as shown:
/// without "https://".
pub fn repository() -> &'static str {
    let address = env!("CARGO_PKG_REPOSITORY");
    address.strip_prefix("https://").unwrap_or(address)
}

/// What this build may do about updates: a release file installs them, a
/// packaged build (`MOTHRIX_PACKAGED` set when it was built, by the `.deb`
/// script and the PKGBUILD) only tells, any other never looks.
fn build() -> update::Build {
    let packaged = option_env!("MOTHRIX_PACKAGED").is_some_and(|value| !value.is_empty());
    update::Build::of(VERSION, env!("CARGO_PKG_VERSION"), packaged)
}

/// Fetching releases through the config's proxy.
fn release_fetch(paths: &Paths) -> update::Http {
    update::Http {
        client: Config::load(&paths.config_file).http_client(),
        agent: format!("mothrix/{}", env!("CARGO_PKG_VERSION")),
    }
}

/// A new release installed while running: started in place of this one
/// once the terminal is back (see `main`).
static INSTALLED: std::sync::Mutex<Option<(String, update::Installed)>> =
    std::sync::Mutex::new(None);

use std::{io::stdout, time::Duration};

use anyhow::{Context, Result, bail};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use matrix_sdk::Client;
use ratatui::{Terminal, backend::CrosstermBackend};
use serde::{Deserialize, Serialize};

use crate::{
    app::{App, Effect},
    config::{Config, ConfigExt},
    matrix::{Command, LoginOptions, Paths, Update},
};

const HELP: &str = "\
mothrix - a terminal Matrix client with a room sidebar and mouse support

Usage:
  mothrix                                  start with all signed-in accounts
                                        (signs in first if there is none)
  mothrix --profile NAME                   start with only that account
  mothrix login [SERVER] [--user NAME] [--password]
                                        sign in; SERVER is a homeserver name or
                                        URL, or a full Matrix ID. Browser sign-in
                                        (SSO) is used when the server offers it;
                                        --password forces password login.
  mothrix login SERVER --profile NAME      add another account under a name
                                        of your choice (work, school ...)
  mothrix accounts                         list the signed-in accounts
  mothrix update                           install a new release (release builds)
  mothrix logout [--profile NAME]          sign out and delete local data
  mothrix dictionary [LANGUAGE]            list spell-check dictionaries, or download
                                        one (e.g. de_DE) into mothrix's own folder
  mothrix events [ROOM] [--limit N]        show the types and structure of a room's
                                        recent events, with all text masked (for
                                        reporting what another client sends)
  mothrix pushers [list]                   the account's push targets (phones)
  mothrix pushers add URL --app-id ID --pushkey KEY
  mothrix pushers remove --app-id ID --pushkey KEY
                                        add or remove one by hand, for trying a
                                        push gateway (Sygnal)
  mothrix --help | --version

Inside mothrix (F1 or ? shows every key and command):
  mouse        click a room, a message, a link, a poll answer, a reaction or
               the message box; right-click a room for its menu; wheel scrolls
  Tab          switch between the room list and the message box
  Ctrl+K       find a room             Alt+Up/Down   previous / next room
  Ctrl+F       search messages         F2            settings, accounts
  F3           with several accounts: show all, or one at a time
  F4           only unread chats, or all
  F5           all emoji: search, pick one for the message box
  PgUp/PgDn    scroll the conversation Ctrl+T        next colour theme
  Enter        open room / send        Ctrl+Q        quit
  room list    p pin, l low priority, s sort, i room info, M notifications,
               m mix rooms and people, g section, z fold, a accounts
  messages     Up from the message box selects one; then r reply, a react,
               c copy, e edit, d delete, o open, t thread, p pin, f forward,
               u first new message, x more (long text, all who reacted);
               after a: 1-9 your favourites, 0 all emoji
  typing       @name suggests members, :name emoji; Alt+Enter starts a new line
  /            lists the commands of the message box, like /poll, /file,
               /room, /dm, /invite, /info, /pins, /verify, /backup, /settings

Files:
  config   ~/.config/mothrix/config.toml
  data     ~/.local/share/mothrix/<profile>/   (session, encryption keys, log)
";

enum Mode {
    Run,
    Login(LoginOptions),
    Logout,
    /// List the signed-in accounts.
    Accounts,
    /// Install a new release.
    Update,
    /// List installed spell-check dictionaries, or download one.
    Dictionary {
        language: Option<String>,
    },
    /// Print what the recent events of a room look like (no message text).
    Events {
        room: Option<String>,
        limit: u32,
    },
    /// List the account's pushers, or add or remove one (for trying a
    /// push gateway).
    Pushers(PushersAction),
    /// Debug aid: sync for a while, then print the screen as plain text.
    Snapshot {
        seconds: u64,
        room: Option<String>,
        say: Option<String>,
        width: u16,
        height: u16,
    },
}

enum PushersAction {
    List,
    Add(mothrix_core::push::Target),
    Remove { app_id: String, pushkey: String },
}

struct Args {
    /// The profile named with `--profile`; without, "default" (and for
    /// running: all that are signed in).
    profile: Option<String>,
    mode: Mode,
}

fn parse_args() -> Result<Option<Args>> {
    let mut args = std::env::args().skip(1).peekable();
    let mut profile = None;
    let mut mode = Mode::Run;
    let value = |args: &mut std::iter::Peekable<std::iter::Skip<std::env::Args>>, flag: &str| {
        args.next().with_context(|| format!("{flag} needs a value"))
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                println!("\nSource, bugs: https://{}", repository());
                return Ok(None);
            }
            "--version" | "-V" => {
                println!("mothrix {VERSION}");
                return Ok(None);
            }
            "--profile" | "-p" => {
                let name = value(&mut args, "--profile")?;
                if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
                    bail!("'{name}' cannot be used as a profile name");
                }
                profile = Some(name);
            }
            "logout" => mode = Mode::Logout,
            "accounts" => mode = Mode::Accounts,
            "update" => mode = Mode::Update,
            "dictionary" => {
                let language = args.next_if(|next| !next.starts_with('-'));
                mode = Mode::Dictionary { language };
            }
            "events" => {
                let mut room = None;
                let mut limit = 30;
                while let Some(next) = args.peek().cloned() {
                    match next.as_str() {
                        "--limit" => {
                            args.next();
                            limit = value(&mut args, "--limit")?
                                .parse()
                                .context("--limit takes a number")?;
                        }
                        "--profile" | "-p" => break,
                        other if other.starts_with('-') => bail!("unknown events option '{other}'"),
                        _ => room = args.next(),
                    }
                }
                mode = Mode::Events { room, limit };
            }
            "pushers" => {
                let action = args.next_if(|next| !next.starts_with('-'));
                let mut app_id = None;
                let mut pushkey = None;
                let mut gateway = None;
                while let Some(next) = args.peek().cloned() {
                    match next.as_str() {
                        "--app-id" => {
                            args.next();
                            app_id = Some(value(&mut args, "--app-id")?);
                        }
                        "--pushkey" => {
                            args.next();
                            pushkey = Some(value(&mut args, "--pushkey")?);
                        }
                        "--profile" | "-p" => break,
                        other if other.starts_with('-') => {
                            bail!("unknown pushers option '{other}'")
                        }
                        _ => gateway = args.next(),
                    }
                }
                let needed = |value: Option<String>, flag: &str| {
                    value.with_context(|| {
                        format!(
                            "pushers {}: {flag} is needed",
                            action.as_deref().unwrap_or("")
                        )
                    })
                };
                mode = Mode::Pushers(match action.as_deref() {
                    None | Some("list") => PushersAction::List,
                    Some("add") => PushersAction::Add(mothrix_core::push::Target {
                        gateway: needed(gateway, "the gateway's address")?,
                        app_id: needed(app_id, "--app-id")?,
                        pushkey: needed(pushkey, "--pushkey")?,
                        device_name: "mothrix pushers".to_owned(),
                        lang: "en".to_owned(),
                        profile: "default".to_owned(),
                    }),
                    Some("remove") => PushersAction::Remove {
                        app_id: needed(app_id, "--app-id")?,
                        pushkey: needed(pushkey, "--pushkey")?,
                    },
                    Some(other) => bail!("pushers: '{other}' is not list, add or remove"),
                });
            }
            "login" => {
                let mut options = LoginOptions {
                    server: None,
                    user: None,
                    password_login: false,
                };
                while let Some(next) = args.peek().cloned() {
                    match next.as_str() {
                        "--user" => {
                            args.next();
                            options.user = Some(value(&mut args, "--user")?);
                        }
                        "--password" => {
                            args.next();
                            options.password_login = true;
                        }
                        "--profile" | "-p" => break,
                        other if other.starts_with('-') => bail!("unknown login option '{other}'"),
                        _ => options.server = args.next(),
                    }
                }
                mode = Mode::Login(options);
            }
            "--snapshot" => {
                let seconds = value(&mut args, "--snapshot")?
                    .parse()
                    .context("--snapshot takes seconds")?;
                mode = Mode::Snapshot {
                    seconds,
                    room: None,
                    say: None,
                    width: 110,
                    height: 36,
                };
            }
            "--room" | "--say" | "--size" => {
                let Mode::Snapshot {
                    room,
                    say,
                    width,
                    height,
                    ..
                } = &mut mode
                else {
                    bail!("{arg} only works together with --snapshot");
                };
                let given = value(&mut args, &arg)?;
                match arg.as_str() {
                    "--room" => *room = Some(given),
                    "--say" => *say = Some(given),
                    _ => {
                        let (w, h) = given.split_once('x').context("--size takes WIDTHxHEIGHT")?;
                        *width = w.parse()?;
                        *height = h.parse()?;
                    }
                }
            }
            other => bail!("unknown argument '{other}' (try --help)"),
        }
    }
    Ok(Some(Args { profile, mode }))
}

fn init_logging(paths: &Paths) {
    // The terminal belongs to the UI, so diagnostics go to a file.
    let _ = std::fs::create_dir_all(&paths.data_dir);
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_file())
    {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .with_max_level(match std::env::var("MOTHRIX_LOG").as_deref() {
                Ok("debug") => tracing::Level::DEBUG,
                Ok("info") => tracing::Level::INFO,
                _ => tracing::Level::WARN,
            })
            .try_init();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let result = run().await;
    if let Err(error) = &result {
        eprintln!("mothrix: {error:#}");
    }
    let installed = INSTALLED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some((version, installed)) = installed {
        restart(&version, installed);
    }
    // Leave without unwinding the async runtime: matrix-sdk's background
    // tasks would otherwise be cancelled mid-flight and print panics over
    // the restored terminal. Everything durable is already in SQLite.
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}

async fn run() -> Result<()> {
    let Some(args) = parse_args()? else {
        return Ok(());
    };
    let primary = args.profile.clone().unwrap_or_else(|| "default".to_owned());
    let paths = Paths::for_profile(&primary)?;

    match args.mode {
        Mode::Logout => {
            matrix::logout(&paths).await?;
            println!("Signed out; local data for profile '{primary}' removed.");
            Ok(())
        }
        Mode::Accounts => {
            let profiles = matrix::signed_in_profiles();
            if profiles.is_empty() {
                println!("No account yet; sign in with `mothrix login`.");
            }
            for profile in profiles {
                let user = matrix::signed_in_as(&Paths::for_profile(&profile)?);
                println!("{profile:<12} {}", user.unwrap_or_default());
            }
            println!("Add one with `mothrix login SERVER --profile NAME`.");
            Ok(())
        }
        Mode::Login(options) => {
            if let Some(user) = matrix::signed_in_as(&paths) {
                bail!(
                    "profile '{primary}' is already signed in as {user}. Add another account \
                     with `mothrix login SERVER --profile NAME`, or sign this one out first \
                     with `mothrix logout{}`.",
                    args.profile
                        .as_ref()
                        .map(|name| format!(" --profile {name}"))
                        .unwrap_or_default()
                );
            }
            let client = matrix::login(&paths, options).await?;
            println!(
                "Signed in as {}. Start mothrix to chat.",
                app::short_id(&user_of(&client), 48)
            );
            Ok(())
        }
        Mode::Dictionary { language } => dictionary_command(&paths, language).await,
        Mode::Update => update_command(&paths).await,
        Mode::Events { room, limit } => {
            let client = matrix::restore(&paths)
                .await?
                .context("not signed in; run `mothrix login` first")?;
            matrix::print_events(&client, room.as_deref(), limit).await
        }
        Mode::Pushers(action) => {
            let client = matrix::restore(&paths)
                .await?
                .context("not signed in; run `mothrix login` first")?;
            pushers_command(&client, action).await
        }
        Mode::Run => {
            let mut clients = restore_accounts(&args.profile).await?;
            if clients.is_empty() {
                println!("Not signed in yet.");
                let client = matrix::login(
                    &paths,
                    LoginOptions {
                        server: None,
                        user: None,
                        password_login: false,
                    },
                )
                .await?;
                clients.push((primary, client));
            }
            init_logging(&paths);
            if let Ok(exe) = std::env::current_exe() {
                update::cleanup(&exe);
            }
            run_tui(clients, &paths).await
        }
        Mode::Snapshot {
            seconds,
            room,
            say,
            width,
            height,
        } => {
            let clients = restore_accounts(&args.profile).await?;
            if clients.is_empty() {
                bail!("not signed in; run `mothrix login` first");
            }
            init_logging(&paths);
            snapshot(clients, &paths, seconds, room, say, width, height).await
        }
    }
}

/// The accounts to run with: the named profile, or all that are signed in.
/// One that cannot be restored is reported and left out, as long as
/// another works.
async fn restore_accounts(named: &Option<String>) -> Result<Vec<(String, Client)>> {
    let profiles = match named {
        Some(profile) => vec![profile.clone()],
        None => {
            // In the order chosen in the accounts dialog; new ones last.
            let mut profiles = matrix::signed_in_profiles();
            if let Ok(paths) = Paths::for_profile("default") {
                Config::load(&paths.config_file).order_profiles(&mut profiles);
            }
            profiles
        }
    };
    let mut clients = Vec::new();
    let mut failure = None;
    for profile in profiles {
        match matrix::restore(&Paths::for_profile(&profile)?).await {
            Ok(Some(client)) => clients.push((profile, client)),
            Ok(None) => {}
            Err(error) => {
                eprintln!("mothrix: account '{profile}' left out: {error:#}");
                failure = Some(error.context(format!("account '{profile}'")));
            }
        }
    }
    match failure {
        Some(error) if clients.is_empty() => Err(error),
        _ => Ok(clients),
    }
}

/// `mothrix dictionary`: show what the spell checker can use; with a
/// language, download that dictionary into mothrix's own folder.
/// `mothrix pushers`: list, add or remove a pusher of the account.
async fn pushers_command(client: &Client, action: PushersAction) -> Result<()> {
    use mothrix_core::push;
    match action {
        PushersAction::List => {
            let pushers = push::list(client).await?;
            if pushers.is_empty() {
                println!("No pushers.");
            }
            for pusher in pushers {
                println!(
                    "{} on {}\n  app id   {}\n  pushkey  {}\n  gateway  {}{}",
                    pusher.app_name,
                    pusher.device_name,
                    pusher.app_id,
                    pusher.pushkey,
                    pusher.gateway,
                    if pusher.event_id_only {
                        " (ids only)"
                    } else {
                        ""
                    },
                );
            }
        }
        PushersAction::Add(target) => {
            push::add(client, &target).await?;
            println!(
                "Added: {} sends pushes for {} to {}.",
                user_of(client),
                target.app_id,
                target.gateway
            );
        }
        PushersAction::Remove { app_id, pushkey } => {
            push::remove(client, &app_id, &pushkey).await?;
            println!("Removed the pusher of {app_id}.");
        }
    }
    Ok(())
}

async fn dictionary_command(paths: &Paths, language: Option<String>) -> Result<()> {
    let dirs = spell::dictionary_dirs();
    let Some(language) = language else {
        let installed = spell::available_languages(&dirs);
        println!(
            "Installed dictionaries: {}",
            if installed.is_empty() {
                "none".to_owned()
            } else {
                installed.join(", ")
            }
        );
        println!("In use: {}", spell_languages(paths).join(", "));
        let known: Vec<&str> = spell::DOWNLOADABLE
            .iter()
            .map(|(language, _)| *language)
            .collect();
        println!(
            "Can be downloaded with `mothrix dictionary <language>`: {}",
            known.join(", ")
        );
        println!(
            "To choose, set e.g. spell_languages = [\"de_DE\", \"en_US\"] in {}",
            paths.config_file.display()
        );
        return Ok(());
    };

    let (aff_url, dic_url) = spell::download_urls(&language).with_context(|| {
        format!("no download known for '{language}' (run `mothrix dictionary` for the list)")
    })?;
    let target = spell::user_dictionary_dir()?;
    std::fs::create_dir_all(&target)?;
    let http = Config::load(&paths.config_file).http_client();
    for (url, extension) in [(aff_url, "aff"), (dic_url, "dic")] {
        println!("Downloading {url}");
        let response = http.get(&url).send().await?.error_for_status()?;
        std::fs::write(
            target.join(format!("{language}.{extension}")),
            response.bytes().await?,
        )?;
    }
    // Make sure it loads before promising anything.
    let check = language.clone();
    tokio::task::spawn_blocking(move || spell::load(&[check], &spell::dictionary_dirs()))
        .await?
        .context("the downloaded dictionary could not be loaded")?;
    println!("Installed {language} into {}.", target.display());
    if !spell_languages(paths).contains(&language) {
        println!(
            "Add it to spell_languages in {} to use it.",
            paths.config_file.display()
        );
    }
    Ok(())
}

fn user_of(client: &Client) -> String {
    client
        .user_id()
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn new_app(paths: &Paths, accounts: Vec<app::Account>) -> App {
    let config = Config::load(&paths.config_file);
    let (themes, mut problems) = config.custom_themes();
    theme::set_custom(themes);
    let (keymap, key_problems) = keys::keymap(&config.keys);
    problems.extend(key_problems);
    let mut app = App::new(theme::THEMES[0], String::new());
    app.keymap = keymap;
    app.updates = app::Updates::new(build(), update::public_key().is_some());
    // One line is all the status bar has.
    if let Some(first) = problems.first() {
        app.notice = Some(format!("config.toml: {first}"));
    }
    app.state.accounts = accounts;
    app.kitty_supported = kitty::supported();
    app.sixel_supported = sixel::supported();
    config.apply(&mut app, &config.dictionaries());
    app.emoji_usage = mothrix_core::emoji::Usage::load();
    app.ignored_words = std::fs::read_to_string(words_file(paths))
        .unwrap_or_default()
        .lines()
        .map(|word| word.trim().to_lowercase())
        .filter(|word| !word.is_empty())
        .collect();
    app
}

/// What a run leaves for the next one: the open room and unsent text.
#[derive(Default, Serialize, Deserialize)]
struct SessionState {
    #[serde(default)]
    last_room: String,
    #[serde(default)]
    drafts: std::collections::BTreeMap<String, String>,
}

fn state_file() -> Option<std::path::PathBuf> {
    Some(mothrix_core::folders::data()?.join("state.json"))
}

fn load_state(app: &mut App) {
    let state: SessionState = state_file()
        .and_then(|file| std::fs::read(file).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    app.restore_session(Some(state.last_room), state.drafts.into_iter().collect());
}

fn save_state(app: &mut App) {
    let Some(file) = state_file() else { return };
    let (last_room, drafts) = app.session_state();
    let state = SessionState {
        last_room: last_room.unwrap_or_default(),
        drafts: drafts.into_iter().collect(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&state) {
        let _ = std::fs::write(&file, bytes);
        // Drafts are private.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        }
    }
}

/// Words the user told the spell checker to ignore, one per line.
fn words_file(paths: &Paths) -> std::path::PathBuf {
    paths.config_file.with_file_name("words.txt")
}

fn remember_words(paths: &Paths, words: &[String]) {
    use std::io::Write;
    if words.is_empty() {
        return;
    }
    let file = words_file(paths);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut out) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        for word in words {
            let _ = writeln!(out, "{word}");
        }
    }
}

/// The dictionaries named in the config, or the defaults.
fn spell_languages(paths: &Paths) -> Vec<String> {
    Config::load(&paths.config_file).dictionaries()
}

/// Put `text` on the system clipboard. The terminal does it (OSC 52), which
/// also works over SSH; where a Wayland or X11 clipboard tool is installed
/// it is used as well, for terminals that ignore the escape sequence.
fn copy_to_clipboard(text: &str) {
    use std::io::Write;

    use base64::Engine;

    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();

    for (tool, args) in [
        ("wl-copy", &[][..]),
        ("xclip", &["-selection", "clipboard"][..]),
        // macOS.
        ("pbcopy", &[][..]),
    ] {
        let child = std::process::Command::new(tool)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            break;
        }
    }
}

/// Draw a frame. Returns the Sixel pictures that can be painted over it:
/// those whose cells nothing else (a popup, say) was drawn into.
/// Write escape sequences straight to the terminal, between frames.
fn write_terminal(sequence: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = stdout();
    out.write_all(sequence.as_bytes())?;
    out.flush()
}

fn paint_frame(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<Vec<app::SixelPlacement>> {
    let frame = terminal.draw(|frame| ui::draw(frame, app))?;
    let free = |placement: &app::SixelPlacement| {
        (placement.y..placement.y + placement.rows).all(|y| {
            (placement.x..placement.x + placement.columns).all(|x| {
                frame
                    .buffer
                    .cell((x, y))
                    .is_some_and(|cell| cell.symbol() == " ")
            })
        })
    };
    Ok(app
        .sixel_placements
        .iter()
        .filter(|placement| free(placement))
        .cloned()
        .collect())
}

/// News from a sign-in started in the accounts dialog.
enum SignInEvent {
    Progress(String),
    /// The profile, the name typed for it, and its client if signing in
    /// worked.
    Done(String, String, Result<Client>),
}

/// Do what the accounts dialog asked for: start signing in, or sign out.
fn start_account_request(
    request: app::AccountRequest,
    app: &mut App,
    backend: &mut accounts::Accounts,
    signin_tx: &tokio::sync::mpsc::UnboundedSender<SignInEvent>,
) {
    match request {
        app::AccountRequest::Add {
            profile,
            label,
            server,
            user,
            password,
        } => {
            let tx = signin_tx.clone();
            tokio::spawn(async move {
                let progress = tx.clone();
                let result = async {
                    let paths = Paths::for_profile(&profile)?;
                    if matrix::signed_in_as(&paths).is_some() {
                        bail!("there is already an account called '{profile}'");
                    }
                    // A password typed in means: use it, also where
                    // the browser would do.
                    let password_login = !password.is_empty();
                    matrix::sign_in(
                        &paths,
                        &server,
                        password_login,
                        move || {
                            if user.is_empty() || password.is_empty() {
                                bail!(
                                    "this server signs in with a user name and a \
                                     password; fill in both"
                                );
                            }
                            Ok((user, password))
                        },
                        move |line| {
                            let _ = progress.send(SignInEvent::Progress(line));
                        },
                    )
                    .await
                }
                .await;
                let _ = tx.send(SignInEvent::Done(profile, label, result));
            });
        }
        app::AccountRequest::Swap(a, b) => backend.swap(app, a, b),
        app::AccountRequest::Remove(account) => {
            if let Some((profile, Some(client))) = backend.remove(app, account)
                && let Ok(paths) = Paths::for_profile(&profile)
            {
                tokio::spawn(matrix::sign_out(client, paths));
                app.notice = Some(format!("Signed out '{profile}'."));
            }
        }
    }
}

/// Take in what a sign-in from the accounts dialog came to.
fn finish_sign_in(app: &mut App, backend: &mut accounts::Accounts, event: SignInEvent) {
    match event {
        SignInEvent::Progress(line) => app.account_progress(line),
        SignInEvent::Done(profile, label, Ok(client)) => {
            let mut account = backend.add(profile, client);
            if !label.is_empty() {
                account.label = label;
            }
            app.state.accounts.push(account);
            // The name is worth keeping.
            app.state.config_dirty = true;
            app.account_added(Ok(()));
        }
        SignInEvent::Done(profile, _, Err(error)) => {
            // Nothing of a failed sign-in is worth keeping.
            if let Ok(paths) = Paths::for_profile(&profile) {
                let _ = std::fs::remove_dir_all(&paths.data_dir);
            }
            app.account_added(Err(format!("{error:#}")));
        }
    }
}

/// Put the terminal back, also when mothrix panics.
/// What came of looking for a release, or of installing one.
enum ReleaseNews {
    Checked {
        found: Result<Option<update::Available>, String>,
        /// By `/update`.
        asked: bool,
    },
    Installed(String, update::Installed),
    Failed(String),
}

async fn look_for_release(
    fetch: &update::Http,
    key: &update::VerifyingKey,
) -> Result<Option<update::Available>, String> {
    update::check(
        fetch,
        env!("CARGO_PKG_REPOSITORY"),
        env!("CARGO_PKG_VERSION"),
        key,
    )
    .await
    .map_err(|error| format!("{error:#}"))
}

async fn install_release(
    fetch: &update::Http,
    available: &update::Available,
) -> Result<update::Installed> {
    let data = update::download(fetch, available).await?;
    update::install(&std::env::current_exe()?, &data)
}

/// `mothrix update`: install a newer release, if this build may.
async fn update_command(paths: &Paths) -> Result<()> {
    let build = build();
    if build == update::Build::Development {
        bail!("this build is not a release ({VERSION}): it does not update itself");
    }
    let key = update::public_key()
        .context("this build has no release key: it cannot look for updates")?;
    let fetch = release_fetch(paths);
    let repository = env!("CARGO_PKG_REPOSITORY");
    let Some(available) =
        update::check(&fetch, repository, env!("CARGO_PKG_VERSION"), &key).await?
    else {
        println!("mothrix {VERSION} is the newest release.");
        return Ok(());
    };
    if build == update::Build::Packaged {
        println!(
            "mothrix {} is out: update your package ({}).",
            available.version, available.page
        );
        return Ok(());
    }
    if available.file.is_none() {
        println!("mothrix {} is out: {}", available.version, available.page);
        return Ok(());
    }
    println!("Downloading mothrix {}…", available.version);
    let data = update::download(&fetch, &available).await?;
    update::install(&std::env::current_exe()?, &data)?;
    println!(
        "mothrix {} is installed; start it again.",
        available.version
    );
    Ok(())
}

/// Start the new release in place of this process, with the same
/// arguments, now that the terminal is restored. On Unix it replaces this
/// process, which lets go of the accounts as quitting does. Windows has no
/// such thing, and a second process beside this one would use the same
/// accounts at once: there mothrix says so and ends.
fn restart(version: &str, installed: update::Installed) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = std::env::current_exe()
            .map(|exe| {
                std::process::Command::new(exe)
                    .args(std::env::args_os().skip(1))
                    .exec()
            })
            .unwrap_or_else(|error| error);
        // Still here: it did not start. The old one goes back.
        let undone = installed.undo();
        eprintln!(
            "mothrix: could not start mothrix {version} ({error}){}",
            if undone.is_ok() {
                "; the old one is back"
            } else {
                ""
            }
        );
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    {
        drop(installed);
        println!("mothrix {version} is installed: start mothrix again.");
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange
    );
}

async fn run_tui(clients: Vec<(String, Client)>, paths: &Paths) -> Result<()> {
    let (mut backend, accounts) = accounts::spawn(clients);
    let mut app = new_app(paths, accounts);
    load_state(&mut app);

    // Dictionaries can take a moment to load (German is large); do it in
    // the background and hand them over when ready.
    let (speller_tx, mut speller_rx) = tokio::sync::mpsc::unbounded_channel();
    let languages = spell_languages(paths);

    // LanguageTool (only with a server set): ask once typing pauses.
    let (grammar_tx, mut grammar_rx) = tokio::sync::mpsc::unbounded_channel();
    let grammar_http = Config::load(&paths.config_file).http_client();
    let grammar_variants = languagetool::preferred_variants(&languages);
    let mut grammar_due: Option<(String, tokio::time::Instant)> = None;
    tokio::task::spawn_blocking(move || {
        if let Ok(dictionaries) = spell::load(&languages, &spell::dictionary_dirs()) {
            let _ = speller_tx.send(dictionaries);
        }
    });

    // Signing in from the accounts dialog: progress lines, then the result.
    let (signin_tx, mut signin_rx) = tokio::sync::mpsc::unbounded_channel::<SignInEvent>();

    // New releases of mothrix: a look 10 s after the start and then daily,
    // while the setting allows; and what `/update` asks for.
    let (release_tx, mut release_rx) = tokio::sync::mpsc::unbounded_channel::<ReleaseNews>();
    let release_looks = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        app.updates.looks(&app.state.prefs),
    ));
    if let Some(key) = update::public_key()
        && app.updates.build != update::Build::Development
    {
        let (fetch, tx, looks) = (
            release_fetch(paths),
            release_tx.clone(),
            release_looks.clone(),
        );
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            loop {
                if looks.load(std::sync::atomic::Ordering::Relaxed) {
                    let found = look_for_release(&fetch, &key).await;
                    if tx
                        .send(ReleaseNews::Checked {
                            found,
                            asked: false,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
            }
        });
    }

    enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_panic(info);
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut events = EventStream::new();
    let mut clicks = notify::ClickReader::default();
    let (toast_clicks, mut toast_clicked) = tokio::sync::mpsc::unbounded_channel();
    let mut notifier = notify::Notifier::new(toast_clicks);
    // The Sixel pictures on screen right now.
    let mut sixel_shown: Vec<app::SixelPlacement> = Vec::new();
    let result: Result<()> = async {
        let mut updated = false;
        loop {
            release_looks.store(
                app.updates.looks(&app.state.prefs),
                std::sync::atomic::Ordering::Relaxed,
            );
            if app.graphics == app::Graphics::Sixel
                && let Ok(size) = crossterm::terminal::window_size()
                && size.width > 0
                && size.columns > 0
                && size.rows > 0
            {
                app.cell_px = (size.width / size.columns, size.height / size.rows);
            }
            // Sections changed here go to the accounts that keep them.
            backend.sync_sections(&mut app);
            let shown = paint_frame(&mut terminal, &mut app)?;
            // Sixel pictures sit on top of the text. When what is to be
            // seen of them changed, wipe the screen, draw it again and
            // paint the pictures anew.
            if app.graphics == app::Graphics::Sixel && shown != sixel_shown {
                terminal.clear()?;
                let shown = paint_frame(&mut terminal, &mut app)?;
                use std::io::Write;
                let mut out = stdout();
                // Leave the cursor where the frame put it.
                out.write_all(b"\x1b7")?;
                for placement in &shown {
                    if let Some(sixel) = app.sixel.get(&placement.event_id) {
                        write!(out, "\x1b[{};{}H", placement.y + 1, placement.x + 1)?;
                        out.write_all(sixel.as_bytes())?;
                    }
                }
                out.write_all(b"\x1b8")?;
                out.flush()?;
                sixel_shown = shown;
            }

            let commands = tokio::select! {
                event = events.next() => match event {
                    Some(Ok(event)) => {
                        let mut commands = Vec::new();
                        for input in clicks.feed(event) {
                            match input {
                                notify::Input::Event(event) => commands.extend(app.on_event(event)),
                                notify::Input::Clicked(id) => {
                                    if let Some(room) = notifier.clicked(id) {
                                        app.open_from_notification(room);
                                    }
                                }
                            }
                        }
                        commands
                    }
                    Some(Err(error)) => return Err(error.into()),
                    None => break,
                },
                Some(room) = toast_clicked.recv() => {
                    app.open_from_notification(room);
                    Vec::new()
                }
                Some((text, result)) = grammar_rx.recv() => {
                    app.set_grammar(text, result);
                    Vec::new()
                }
                _ = tokio::time::sleep_until(
                    grammar_due.as_ref().map(|due| due.1).unwrap_or_else(tokio::time::Instant::now)
                ), if grammar_due.is_some() => {
                    if let Some((text, _)) = grammar_due.take() {
                        let (http, tx) = (grammar_http.clone(), grammar_tx.clone());
                        let variants = grammar_variants.clone();
                        let url = languagetool::Access {
                            url: app.state.prefs.languagetool_url.clone(),
                            username: app.state.prefs.languagetool_username.clone(),
                            api_key: app.state.prefs.languagetool_api_key.clone(),
                        };
                        let languages = app.state.prefs.languagetool_languages.clone();
                        tokio::spawn(async move {
                            let result =
                                languagetool::check_in(&http, &url, &text, &languages, &variants).await;
                            let _ = tx.send((text, result));
                        });
                    }
                    Vec::new()
                }
                Some(event) = signin_rx.recv() => {
                    finish_sign_in(&mut app, &mut backend, event);
                    Vec::new()
                }
                Some(news) = release_rx.recv() => {
                    match news {
                        ReleaseNews::Checked { found, asked } => app.update_checked(found, asked),
                        ReleaseNews::Installed(version, installed) => {
                            *INSTALLED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some((version, installed));
                            updated = true;
                        }
                        ReleaseNews::Failed(error) => app.update_failed(error),
                    }
                    Vec::new()
                }
                Some(dictionaries) = speller_rx.recv() => {
                    let languages = dictionaries.languages();
                    app.set_speller(std::sync::Arc::new(dictionaries), languages);
                    Vec::new()
                }
                update = backend.updates.recv() => {
                    let Some((account, update)) = update else { break };
                    backend.deliver(&mut app, account, update);
                    // Fold a burst of updates into a single redraw.
                    while let Ok((account, update)) = backend.updates.try_recv() {
                        backend.deliver(&mut app, account, update);
                    }
                    Vec::new()
                }
            };
            // Carry out what the app asks for, in order.
            let mut quit = false;
            let effects = commands.into_iter().map(Effect::Command).chain(app.take_effects());
            for effect in effects.collect::<Vec<_>>() {
                match effect {
                    Effect::Command(command) => backend.send(&app, command),
                    Effect::RoomOpened(room) => {
                        if let Some(close) = notifier.opened(&room) {
                            write_terminal(&close)?;
                        }
                    }
                    Effect::Terminal(sequence) => write_terminal(&sequence)?,
                    Effect::Notify { room, title } => {
                        if let Some(sequence) = notifier.show(&room, &title) {
                            write_terminal(&sequence)?;
                        }
                    }
                    Effect::Copy(text) => copy_to_clipboard(&text),
                    Effect::CheckGrammar(text) => {
                        let due = tokio::time::Instant::now() + Duration::from_millis(700);
                        grammar_due = Some((text, due));
                    }
                    Effect::IgnoreWord(word) => remember_words(paths, &[word]),
                    Effect::UsedEmoji(emoji) => mothrix_core::emoji::Usage::store(&emoji),
                    Effect::Account(request) => {
                        start_account_request(request, &mut app, &mut backend, &signin_tx);
                    }
                    Effect::CheckForUpdate => {
                        if let Some(key) = update::public_key() {
                            let (fetch, tx) = (release_fetch(paths), release_tx.clone());
                            tokio::spawn(async move {
                                let found = look_for_release(&fetch, &key).await;
                                let _ = tx.send(ReleaseNews::Checked { found, asked: true });
                            });
                        }
                    }
                    Effect::InstallUpdate(available) => {
                        let (fetch, tx) = (release_fetch(paths), release_tx.clone());
                        tokio::spawn(async move {
                            let _ = tx.send(match install_release(&fetch, &available).await {
                                Ok(installed) => ReleaseNews::Installed(available.version, installed),
                                Err(error) => ReleaseNews::Failed(format!("{error:#}")),
                            });
                        });
                    }
                    Effect::SaveState => save_state(&mut app),
                    Effect::SaveConfig => {
                        let file = &paths.config_file;
                        let _ = Config::load(file).updated(&app).save(file);
                    }
                    Effect::Quit => quit = true,
                }
            }
            // A new release is in place: it starts once the terminal is back.
            if quit || updated {
                break;
            }
        }
        Ok(())
    }
    .await;

    // The text in the message box right now is a draft too.
    save_state(&mut app);
    if app.graphics == app::Graphics::Kitty {
        use std::io::Write;
        let _ = stdout().write_all(kitty::delete_all().as_bytes());
    }
    restore_terminal();
    result
}

/// What snapshot mode carries out of what the app asks for: the backend
/// and the accounts get theirs; the terminal, the desktop and the files are
/// left alone.
fn snapshot_effects(
    app: &mut App,
    backend: &mut accounts::Accounts,
    signin_tx: &tokio::sync::mpsc::UnboundedSender<SignInEvent>,
    commands: Vec<Command>,
) {
    let effects = commands
        .into_iter()
        .map(Effect::Command)
        .chain(app.take_effects());
    for effect in effects.collect::<Vec<_>>() {
        match effect {
            Effect::Command(command) => backend.send(app, command),
            Effect::Account(request) => start_account_request(request, app, backend, signin_tx),
            Effect::Terminal(_)
            | Effect::Notify { .. }
            | Effect::RoomOpened(_)
            | Effect::CheckForUpdate
            | Effect::InstallUpdate(_)
            | Effect::Copy(_)
            | Effect::CheckGrammar(_)
            | Effect::IgnoreWord(_)
            | Effect::UsedEmoji(_)
            | Effect::SaveState
            | Effect::SaveConfig
            | Effect::Quit => {}
        }
    }
}

async fn snapshot(
    clients: Vec<(String, Client)>,
    paths: &Paths,
    seconds: u64,
    room: Option<String>,
    say: Option<String>,
    width: u16,
    height: u16,
) -> Result<()> {
    let (mut backend, accounts) = accounts::spawn(clients);
    let mut app = new_app(paths, accounts);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    // `--room "a>b"` opens a, then b two seconds later, and so on. A name
    // matches the room's name or its key (so "work/" finds an account).
    let mut targets: std::collections::VecDeque<String> = room
        .as_deref()
        .unwrap_or_default()
        .split('>')
        .map(str::to_lowercase)
        .collect();
    let mut opened = false;
    let mut next_hop = tokio::time::Instant::now() + Duration::from_secs(3600);
    // `--say "keys:<f2> <down> hello <enter>"` presses those keys instead
    // of sending a message: one a moment, starting when a room is open.
    let mut script: std::collections::VecDeque<Event> = say
        .as_deref()
        .and_then(|text| text.strip_prefix("keys:"))
        .map(script_keys)
        .unwrap_or_default();
    let scripted = !script.is_empty();
    let mut next_key = tokio::time::Instant::now() + Duration::from_secs(3600);
    let (signin_tx, mut signin_rx) = tokio::sync::mpsc::unbounded_channel::<SignInEvent>();

    loop {
        let update = tokio::select! {
            update = backend.updates.recv() => update,
            _ = tokio::time::sleep_until(deadline) => break,
            _ = tokio::time::sleep_until(next_hop) => {
                next_hop += Duration::from_secs(if targets.is_empty() { 3600 } else { 2 });
                if let Some(wanted) = targets.pop_front()
                    && let Some(id) = find_room(&app, &wanted)
                {
                    let mut commands = Vec::new();
                    app.on_hit(app::HitAction::OpenRoom(id), &mut commands);
                    snapshot_effects(&mut app, &mut backend, &signin_tx, commands);
                }
                continue;
            }
            _ = tokio::time::sleep_until(next_key), if !script.is_empty() => {
                next_key += Duration::from_millis(150);
                match script.pop_front() {
                    // A pause, written as <wait>.
                    Some(Event::FocusGained) => next_key += Duration::from_secs(2),
                    Some(key) => {
                        // Drawn first, as on a terminal: drawing notes
                        // what can be clicked or unfolded (`x`).
                        ui::render_to_text(&mut app, width, height);
                        let commands = app.on_event(key);
                        snapshot_effects(&mut app, &mut backend, &signin_tx, commands);
                    }
                    None => {}
                }
                continue;
            }
            Some(event) = signin_rx.recv() => {
                finish_sign_in(&mut app, &mut backend, event);
                continue;
            }
        };
        let Some((account, update)) = update else {
            break;
        };
        let rooms_arrived = matches!(update, Update::Rooms(_));
        backend.deliver(&mut app, account, update);
        snapshot_effects(&mut app, &mut backend, &signin_tx, Vec::new());

        if rooms_arrived && !opened {
            let wanted = targets.front().cloned().unwrap_or_default();
            if let Some(room_id) = find_room(&app, &wanted) {
                opened = true;
                targets.pop_front();
                next_hop = tokio::time::Instant::now() + Duration::from_secs(2);
                next_key = tokio::time::Instant::now() + Duration::from_secs(2);
                let mut commands = Vec::new();
                app.on_hit(app::HitAction::OpenRoom(room_id.clone()), &mut commands);
                snapshot_effects(&mut app, &mut backend, &signin_tx, commands);
                if let Some(text) = say.clone().filter(|_| !scripted) {
                    backend.send(
                        &app,
                        Command::Send {
                            room_id,
                            text,
                            mentions: Vec::new(),
                        },
                    );
                }
            }
        }
    }

    print!("{}", ui::render_to_text(&mut app, width, height));
    Ok(())
}

/// The key of the first room whose name or key contains `wanted`.
fn find_room(app: &App, wanted: &str) -> Option<String> {
    app.state
        .rooms
        .iter()
        .find(|entry| {
            entry.name.to_lowercase().contains(wanted) || entry.id.to_lowercase().contains(wanted)
        })
        .map(|entry| entry.id.clone())
}

/// Key presses from a script like `<c-f> hello <tab> <enter>`: words are
/// typed, names in angle brackets are keys; `<wait>` pauses.
fn script_keys(script: &str) -> std::collections::VecDeque<Event> {
    let press = |code, modifiers| Event::Key(KeyEvent::new(code, modifiers));
    let mut keys = std::collections::VecDeque::new();
    for token in script.split_whitespace() {
        let named = token
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'));
        let code = match named {
            Some("enter") => KeyCode::Enter,
            Some("tab") => KeyCode::Tab,
            Some("esc") => KeyCode::Esc,
            Some("up") => KeyCode::Up,
            Some("down") => KeyCode::Down,
            Some("space") => KeyCode::Char(' '),
            Some("f1") => KeyCode::F(1),
            Some("f2") => KeyCode::F(2),
            Some("f3") => KeyCode::F(3),
            Some("f4") => KeyCode::F(4),
            Some("f5") => KeyCode::F(5),
            Some("wait") => {
                keys.push_back(Event::FocusGained);
                continue;
            }
            Some(name) if name.starts_with("c-") && name.len() == 3 => {
                let c = name.chars().last().unwrap_or(' ');
                keys.push_back(press(KeyCode::Char(c), KeyModifiers::CONTROL));
                continue;
            }
            _ => {
                keys.extend(
                    token
                        .chars()
                        .map(|c| press(KeyCode::Char(c), KeyModifiers::NONE)),
                );
                continue;
            }
        };
        keys.push_back(press(code, KeyModifiers::NONE));
    }
    keys
}
