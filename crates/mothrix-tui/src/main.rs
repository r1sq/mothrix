// SPDX-License-Identifier: MPL-2.0

//! mothrix: a terminal Matrix client with a room sidebar and full mouse support.

#[cfg(test)]
mod accounts_tests;
mod app;
mod config;
mod format;
mod input;
mod keys;
mod kitty;
mod logo;
mod notify;
mod pictures;
mod sixel;
mod theme;
mod ui;

// What lives in `mothrix-core`, under the same paths as the terminal's own
// modules.
use mothrix_core::{
    accounts, client_state, codeblock, emoji, inline, languagetool, matrix, message, model, search,
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

/// A server's or another person's words for printing outside the drawn
/// screen (the command-line modes, errors at the end): without control
/// characters, which the terminal would obey.
fn plain(text: impl AsRef<str>) -> String {
    message::for_terminal(text.as_ref())
}

/// What the status line says when the settings could not be written.
fn not_saved(error: &anyhow::Error) -> String {
    plain(format!(
        "The settings are not saved, and hold only until mothrix ends: {error:#}"
    ))
}

/// Fetching releases through the config's proxy.
fn release_fetch(paths: &Paths) -> update::Http {
    update::Http {
        client: Config::http_client_at(&paths.config_file),
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
use mothrix_core::accounts::SignInEvent;
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
  mothrix --snapshot SECONDS [--room NAME] [--say TEXT] [--size WIDTHxHEIGHT]
                                        without a keyboard: sync that long, print
                                        the screen as text and end. --room names
                                        the room to open (else the first one);
                                        --say (only with --room) SENDS its text
                                        there as a message (\"keys:...\" presses
                                        keys instead); --size is the screen's
                                        (110x36)
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
  F6           format the word or line at the cursor (bold, code, lists …)
  PgUp/PgDn    scroll the conversation Ctrl+T        next colour theme
  Enter        open room / send        Ctrl+Q, Ctrl+C  quit
  room list    p pin, l low priority, s sort, i room info, M notifications,
               m mix rooms and people, g section, z fold, a accounts,
               U only unread chats
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
                // A folder's name, and none mothrix keeps something else
                // under; ":" would also name a drive on Windows.
                if name.is_empty()
                    || name.contains(['/', '\\', ':'])
                    || name.contains(char::is_control)
                    || name.starts_with('.')
                    || name == "dictionaries"
                {
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
    if let Mode::Snapshot { room, say, .. } = &mode {
        said_into_a_named_room(room.as_deref(), say.as_deref())?;
    }
    Ok(Some(Args { profile, mode }))
}

/// `--say` sends a real message (or presses keys that can): only into a
/// room that `--room` names. Without a name the first room of the list
/// would be opened, whichever that is today.
fn said_into_a_named_room(room: Option<&str>, say: Option<&str>) -> Result<()> {
    let named = room.is_some_and(|room| room.split('>').all(|name| !name.trim().is_empty()));
    if say.is_some() && !named {
        bail!(
            "--say needs --room NAME: the room its text is sent into (or its keys are pressed in)"
        );
    }
    Ok(())
}

/// Close the folders with drafts and opened attachments to other users of
/// this computer (older versions made them open), and delete the
/// attachments opened more than a day ago.
fn keep_private() {
    use mothrix_core::folders;
    if let Some(data) = folders::data() {
        let _ = folders::private_dir(&data);
    }
    if let Some(cache) = folders::cache() {
        let _ = folders::private_dir(&cache);
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        folders::sweep(&cache.join("media"), day);
        folders::sweep(&cache.join("pasted"), day);
    }
}

fn init_logging(paths: &Paths) {
    // The terminal belongs to the UI, so diagnostics go to a file.
    let _ = mothrix_core::folders::private_dir(&paths.data_dir);
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
        // An error may carry the server's own words (its `error` text):
        // here no screen is drawn that would take control characters out.
        eprintln!("mothrix: {}", plain(format!("{error:#}")));
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

/// Where this program is, as asked at the start.
static PROGRAM: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// Where this program is, for putting a new release in its place. Asked
/// once, at the start: after another mothrix installed an update, this
/// one still runs from the file that was replaced, and on Linux its path
/// then reads "… (deleted)", where nothing can be written.
fn program() -> Result<std::path::PathBuf> {
    PROGRAM
        .get_or_init(|| std::env::current_exe().ok())
        .clone()
        .context("mothrix could not find out where its program is")
}

async fn run() -> Result<()> {
    let _ = program();
    let Some(args) = parse_args()? else {
        return Ok(());
    };
    let primary = args.profile.clone().unwrap_or_else(|| "default".to_owned());
    let paths = Paths::for_profile(&primary)?;

    match args.mode {
        Mode::Logout => {
            let left = matrix::logout(&paths).await?;
            forget_in_config(&paths, &primary);
            match left {
                None => println!("Signed out; local data for profile '{primary}' removed."),
                Some(left) => println!("Local data for profile '{primary}' removed. {left}"),
            }
            Ok(())
        }
        Mode::Accounts => {
            let profiles = matrix::signed_in_profiles();
            if profiles.is_empty() {
                println!("No account yet; sign in with `mothrix login`.");
            }
            for profile in profiles {
                let user = matrix::signed_in_as(&Paths::for_profile(&profile)?);
                println!("{profile:<12} {}", plain(user.unwrap_or_default()));
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
            forget_in_config(&paths, &primary);
            println!(
                "Signed in as {}. Start mothrix to chat.",
                plain(app::short_id(&user_of(&client), 48))
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
            let (mut clients, left_out) = restore_accounts(&args.profile).await?;
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
                forget_in_config(&paths, &primary);
                clients.push((primary, client));
            }
            init_logging(&paths);
            keep_private();
            // Only a build that installs updates has left such files;
            // any other would delete a file of the user's by that name.
            if build() == update::Build::Release
                && let Ok(exe) = program()
            {
                update::cleanup(&exe);
            }
            run_tui(clients, &paths, left_out).await
        }
        Mode::Snapshot {
            seconds,
            room,
            say,
            width,
            height,
        } => {
            let (clients, _) = restore_accounts(&args.profile).await?;
            if clients.is_empty() {
                bail!("not signed in; run `mothrix login` first");
            }
            init_logging(&paths);
            snapshot(clients, &paths, seconds, room, say, width, height).await
        }
    }
}

/// A profile was signed out, or a new account signed in under its name:
/// the sections and fold state the config file keeps for that name were
/// another account's, and the next account of the name would take those
/// rooms for its own (and store them). A mothrix running meanwhile writes
/// the file as it knows it; a config with a mistake is left alone.
fn forget_in_config(paths: &Paths, profile: &str) {
    if let Ok(mut config) = Config::read(&paths.config_file)
        && config.forget_profile(profile)
    {
        let _ = config.save(&paths.config_file);
    }
}

/// The accounts to run with: the named profile, or all that are signed in.
/// One that cannot be restored is reported and left out, as long as
/// another works; the second value is a line about it for the status
/// bar (what is printed here is gone once the screen is taken).
async fn restore_accounts(
    named: &Option<String>,
) -> Result<(Vec<(String, Client)>, Option<String>)> {
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
    let (clients, problems) = accounts::restore_all(profiles).await;
    let mut failure = None;
    let mut said = None;
    for (profile, error) in problems {
        eprintln!(
            "mothrix: account '{profile}' left out: {}",
            plain(format!("{error:#}"))
        );
        said = Some(format!(
            "Account '{profile}' was left out: {error:#}. `mothrix logout --profile {profile}` \
             removes it."
        ));
        failure = Some(error.context(format!("account '{profile}'")));
    }
    match failure {
        Some(error) if clients.is_empty() => Err(error),
        _ => Ok((clients, said)),
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
                // All of it is what the server says some app registered.
                println!(
                    "{} on {}\n  app id   {}\n  pushkey  {}\n  gateway  {}{}",
                    plain(pusher.app_name),
                    plain(pusher.device_name),
                    plain(pusher.app_id),
                    plain(pusher.pushkey),
                    plain(pusher.gateway),
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
                plain(user_of(client)),
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
    // `read`: a config with a mistake in it may name a proxy.
    let http = Config::read(&paths.config_file)?.http_client();
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
    let read = Config::read(&paths.config_file);
    let broken = read.as_ref().err().map(|error| {
        format!(
            "{error}. mothrix runs with its defaults and saves no settings until that is put right"
        )
    });
    let config = read.unwrap_or_default();
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
    if broken.is_some() {
        app.notice = broken;
    }
    app.state.accounts = accounts;
    app.pictures.kitty_supported = kitty::supported();
    app.pictures.sixel_supported = sixel::supported();
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

/// Whether a room key belongs to one of the accounts with these prefixes
/// (an id as it is: the account without a prefix).
fn of_these_accounts(prefixes: &[String], room: &str) -> bool {
    prefixes.iter().any(|prefix| match prefix.as_str() {
        "" => room.starts_with('!'),
        prefix => room.starts_with(prefix),
    })
}

fn save_state(app: &mut App) {
    let Some(file) = state_file() else { return };
    let (last_room, drafts) = app.session_state();
    // Another mothrix, running with other accounts, keeps its drafts in
    // the same file: those stay as the file has them.
    let prefixes: Vec<String> = app
        .state
        .accounts
        .iter()
        .map(|account| mothrix_core::accounts::prefix_for(&account.profile))
        .collect();
    let on_disk: SessionState = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let state = SessionState {
        last_room: last_room.unwrap_or_default(),
        drafts: on_disk
            .drafts
            .into_iter()
            .filter(|(room, _)| !of_these_accounts(&prefixes, room))
            .chain(drafts)
            .collect(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&state) {
        // Drafts are private.
        let _ = mothrix_core::folders::write_private(&file, &bytes);
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

    // Beside the screen: each tool is waited for, and the next one tried
    // when it could not do it (wl-copy is installed but this is X11, or a
    // session over SSH).
    let text = text.to_owned();
    std::thread::spawn(move || {
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
                if child.wait().is_ok_and(|status| status.success()) {
                    break;
                }
            }
        }
    });
}

/// Write escape sequences straight to the terminal, between frames.
fn write_terminal(sequence: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = stdout();
    out.write_all(sequence.as_bytes())?;
    out.flush()
}

/// Draw a frame. Returns the Sixel pictures that can be painted over it:
/// those whose cells nothing else (a popup, say) was drawn into.
fn paint_frame(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<Vec<pictures::Placement>> {
    let frame = terminal.draw(|frame| ui::draw(frame, app))?;
    Ok(app.pictures.visible(frame.buffer))
}

/// Do what the accounts dialog asked for: start signing in, or sign out.
fn start_account_request(
    request: app::AccountRequest,
    app: &mut App,
    backend: &mut accounts::Accounts,
    signin_tx: &tokio::sync::mpsc::UnboundedSender<SignInEvent>,
) {
    match request {
        app::AccountRequest::Add(request) => {
            let tx = signin_tx.clone();
            accounts::start_sign_in(request, true, move |event| {
                let _ = tx.send(event);
            });
        }
        app::AccountRequest::Swap(a, b) => backend.swap(app, a, b),
        app::AccountRequest::Remove(account) => {
            if let Some(profile) = backend.sign_out(app, account) {
                app.notice = Some(format!("Signed out '{profile}'."));
            }
        }
    }
}

/// Take in what a sign-in from the accounts dialog came to.
fn finish_sign_in(app: &mut App, backend: &mut accounts::Accounts, event: SignInEvent) {
    match event {
        SignInEvent::Progress(line) => app.account_progress(line),
        SignInEvent::Done {
            profile,
            label,
            result: Ok(client),
        } => {
            let account = backend.signed_in(profile, label, client);
            app.state.accounts.push(account);
            // The name is worth keeping.
            app.state.config_dirty = true;
            app.account_added(Ok(()));
        }
        SignInEvent::Done {
            result: Err(error), ..
        } => app.account_added(Err(error)),
    }
}

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
    let (exe, version) = (program()?, available.version.clone());
    // Off the async threads: the new program is started once to try it.
    tokio::task::spawn_blocking(move || update::install_tried(&exe, &data, &version)).await?
}

/// `mothrix update`: install a newer release, if this build may.
async fn update_command(paths: &Paths) -> Result<()> {
    let build = build();
    if build == update::Build::Development {
        bail!("this build is not a release ({VERSION}): it does not update itself");
    }
    let key = update::public_key()
        .context("this build has no release key: it cannot look for updates")?;
    // A config with a mistake in it may name a proxy: say what is wrong
    // with it, rather than fail to connect.
    Config::read(&paths.config_file)?;
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
    update::install_tried(&program()?, &data, &available.version)?;
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
        let error = std::process::Command::new(installed.program())
            .args(std::env::args_os().skip(1))
            .exec();
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

/// The signals that ask a program to end: SIGTERM and SIGHUP, and SIGINT
/// and SIGQUIT, which the keyboard does not send while mothrix has the
/// terminal (Ctrl+C comes as a key) but `kill -INT` does. Left to
/// themselves they end the program where it stands: the terminal stays
/// as mothrix set it up, and what was being written is not saved.
struct EndSignals {
    #[cfg(unix)]
    signals: Vec<tokio::signal::unix::Signal>,
}

impl EndSignals {
    fn new() -> Self {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Self {
                signals: [
                    SignalKind::terminate(),
                    SignalKind::hangup(),
                    SignalKind::interrupt(),
                    SignalKind::quit(),
                ]
                .into_iter()
                .filter_map(|kind| signal(kind).ok())
                .collect(),
            }
        }
        #[cfg(not(unix))]
        Self {}
    }

    /// Comes back when one arrived; never, where there are none.
    async fn next(&mut self) {
        #[cfg(unix)]
        if !self.signals.is_empty() {
            return std::future::poll_fn(|context| {
                let arrived = self
                    .signals
                    .iter_mut()
                    .any(|signal| signal.poll_recv(context).is_ready());
                if arrived {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            })
            .await;
        }
        std::future::pending::<()>().await;
    }
}

/// Put the terminal back, also when mothrix panics.
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

async fn run_tui(
    clients: Vec<(String, Client)>,
    paths: &Paths,
    left_out: Option<String>,
) -> Result<()> {
    let (mut backend, accounts) = accounts::spawn(clients);
    let mut app = new_app(paths, accounts);
    // (What is wrong with the config comes first.)
    if app.notice.is_none() {
        app.notice = left_out;
    }
    load_state(&mut app);

    // Dictionaries can take a moment to load (German is large); do it in
    // the background and hand them over when ready.
    let (speller_tx, mut speller_rx) = tokio::sync::mpsc::unbounded_channel();
    let languages = spell_languages(paths);

    // LanguageTool (only with a server set): ask once typing pauses.
    let (grammar_tx, mut grammar_rx) = tokio::sync::mpsc::unbounded_channel();
    let grammar_http = Config::http_client_at(&paths.config_file);
    let grammar_variants = languagetool::preferred_variants(&languages);
    let mut grammar_due: Option<(String, tokio::time::Instant)> = None;
    tokio::task::spawn_blocking(move || {
        // None installed is nothing to say at every start; one that is
        // there and cannot be used is.
        let loaded = spell::load(&languages, &spell::dictionary_dirs());
        let _ = speller_tx.send(match loaded {
            Ok(dictionaries) => Ok(dictionaries),
            Err(error) if error.is::<spell::NoneInstalled>() => return,
            Err(error) => Err(format!("{error:#}")),
        });
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
                // A day by the clock on the wall: the timer alone stands
                // still while a laptop sleeps.
                let looked = std::time::SystemTime::now();
                while looked
                    .elapsed()
                    .is_ok_and(|since| since < Duration::from_secs(24 * 60 * 60))
                {
                    tokio::time::sleep(Duration::from_secs(15 * 60)).await;
                }
            }
        });
    }

    // Asked to end from outside (the terminal's window closed, `kill`):
    // as with a quit, drafts are saved and the terminal is put right.
    // Listened for before the terminal is changed, so that none of them
    // ends the program with a terminal left as it is.
    let mut asked_to_end = EndSignals::new();
    let mut ended = false;

    enable_raw_mode()?;
    // A terminal that takes the first but not the rest is put back too.
    if let Err(error) = execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    ) {
        restore_terminal();
        return Err(error.into());
    }
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The screen's own thread: the program ends, on a terminal put
        // right first. A task in the background that panics is caught by
        // the runtime and the screen goes on: its terminal has to stay as
        // it is, and the message goes to the log instead of over it.
        if std::thread::current().name() == Some("main") {
            restore_terminal();
            default_panic(info);
        } else {
            tracing::error!("a background task panicked: {info}");
        }
    }));
    let mut terminal = match Terminal::new(CrosstermBackend::new(stdout())) {
        Ok(terminal) => terminal,
        Err(error) => {
            restore_terminal();
            return Err(error.into());
        }
    };
    let mut events = EventStream::new();
    let mut clicks = notify::ClickReader::default();
    let (toast_clicks, mut toast_clicked) = tokio::sync::mpsc::unbounded_channel();
    let mut notifier = notify::Notifier::new(toast_clicks);
    let mut save_failed_said = false;
    let result: Result<()> = async {
        let mut updated = false;
        loop {
            release_looks.store(
                app.updates.looks(&app.state.prefs),
                std::sync::atomic::Ordering::Relaxed,
            );
            if app.pictures.graphics == app::Graphics::Sixel
                && let Ok(size) = crossterm::terminal::window_size()
                && size.width > 0
                && size.columns > 0
                && size.rows > 0
            {
                app.pictures
                    .set_cell_px((size.width / size.columns, size.height / size.rows));
            }
            // Sections changed here go to the accounts that keep them.
            backend.sync_sections(&mut app);
            let shown = paint_frame(&mut terminal, &mut app)?;
            // Sixel pictures sit on top of the text. When what is to be
            // seen of them changed, wipe the screen, draw it again and
            // paint the pictures anew.
            if app.pictures.stale(&shown) {
                terminal.clear()?;
                let shown = paint_frame(&mut terminal, &mut app)?;
                write_terminal(&app.pictures.paint(shown))?;
            }

            let commands = tokio::select! {
                () = asked_to_end.next() => {
                    ended = true;
                    Vec::new()
                }
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
                Some(loaded) = speller_rx.recv() => {
                    match loaded {
                        Ok(dictionaries) => {
                            if let Some(problem) = dictionaries.problems().first() {
                                app.notice = Some(format!("Spell check: {problem}"));
                            }
                            let languages = dictionaries.languages();
                            app.set_speller(std::sync::Arc::new(dictionaries), languages);
                        }
                        Err(problem) => app.notice = Some(format!("Spell check: {problem}")),
                    }
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
            let mut quit = ended;
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
                        // Never over a file with a mistake in it: that
                        // would cost what the user wrote there.
                        let saved = Config::read(file)
                            .and_then(|config| config.updated(&app).save(file));
                        // Said once: what is set holds while mothrix
                        // runs, and every later change would say it again.
                        if let Err(error) = saved
                            && !std::mem::replace(&mut save_failed_said, true)
                        {
                            app.notice = Some(not_saved(&error));
                        }
                    }
                    Effect::Quit => quit = true,
                }
            }
            // The event's commands are out: which account it named says
            // nothing about what a timer or the news sends next.
            app.state.named_account = None;
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
    if let Some(sequence) = app.pictures.leave() {
        let _ = write_terminal(&sequence);
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
    app.state.named_account = None;
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
                        // what can be clicked and how wide things are.
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
                            thread: None,
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
            Some("f6") => KeyCode::F(6),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn say_is_refused_without_a_room_to_say_it_in() {
        assert!(said_into_a_named_room(None, None).is_ok());
        assert!(said_into_a_named_room(Some("lunch"), None).is_ok());
        assert!(said_into_a_named_room(Some("lunch"), Some("hello")).is_ok());
        assert!(said_into_a_named_room(Some("work/>admin"), Some("keys:<f1>")).is_ok());
        // No name, or one that every room matches: the first of the list.
        for room in [None, Some(""), Some(" "), Some("lunch>"), Some(">lunch")] {
            for say in ["hello", "keys:hello <enter>"] {
                let refused = said_into_a_named_room(room, Some(say));
                assert!(
                    refused.is_err_and(|error| error.to_string().contains("--room")),
                    "{room:?} {say}"
                );
            }
        }
    }

    /// `kill -INT` and `kill -QUIT` from outside are heard like SIGTERM
    /// (the loop then saves the drafts and puts the terminal right);
    /// unheard, either ends the program, and this test with it.
    #[cfg(unix)]
    #[tokio::test]
    async fn interrupt_and_quit_from_outside_ask_to_end() {
        let mut asked_to_end = EndSignals::new();
        for signal in ["-INT", "-QUIT", "-TERM"] {
            let sent = std::process::Command::new("kill")
                .args([signal, &std::process::id().to_string()])
                .status();
            assert!(sent.is_ok_and(|status| status.success()), "kill {signal}");
            tokio::time::timeout(Duration::from_secs(10), asked_to_end.next())
                .await
                .unwrap_or_else(|_| panic!("{signal} was not heard"));
        }
    }
}
