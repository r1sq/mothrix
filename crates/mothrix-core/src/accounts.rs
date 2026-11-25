// SPDX-License-Identifier: MPL-2.0

//! Several signed-in accounts at once. Every account (profile) has its own
//! backend; this module merges their updates into the one room list the UI
//! shows and sends each command to the account it belongs to.
//!
//! Two accounts can be in the same room, so inside the UI a room is known
//! by a key: the room id, with the profile name in front for every profile
//! but "default" (`work/!abc:example.org`). The keys are also what the
//! sidebar sections in the config file hold.

use matrix_sdk::Client;
use tokio::sync::mpsc;

use crate::{
    client_state::ClientState,
    matrix::{self, Command, SyncStatus, Update},
    model::{RoomEntry, SearchHit},
};

/// One signed-in account, as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Account {
    /// The profile it was signed in under.
    pub profile: String,
    /// Its name in the sidebar: the profile, unless the user chose one.
    pub label: String,
    pub user_id: String,
    /// Our own display name, once the server told us.
    pub own_name: Option<String>,
    pub sync: SyncStatus,
    /// Whether this device is verified; `None` until known.
    pub verified: Option<bool>,
}

/// The front end [`Accounts`] serve: its [`ClientState`] (the accounts in
/// the same order, the rooms by their keys), and what it does with an
/// update beyond keeping it. The terminal's `App` is one, the app's `View`
/// another.
pub trait Frontend {
    fn state(&self) -> &ClientState;
    fn state_mut(&mut self) -> &mut ClientState;
    /// Take an update, with rooms known by their keys.
    fn on_update(&mut self, update: Update);
    /// The account the user is at when the state does not say (the
    /// terminal: the room under the cursor).
    fn account_hint(&self) -> Option<usize> {
        None
    }
    /// A signed-out account is gone from the state; `closed`: the open
    /// room was one of its rooms. Let go of what was shown of it.
    fn account_gone(&mut self, _closed: bool) {}

    /// The account commands without a room go to
    /// ([`ClientState::active_account`]).
    fn active_account(&self) -> usize {
        self.state().active_account(self.account_hint())
    }
}

/// What goes in front of the room ids of a profile.
pub fn prefix_for(profile: &str) -> String {
    if profile == "default" {
        String::new()
    } else {
        format!("{profile}/")
    }
}

/// The account a room key belongs to, and the room id in it; none for a
/// key of no account here: one with the prefix of a profile that is not
/// running (signed out, left out at this start), or a plain room id when
/// the account without a prefix is not among them. Such a key is nobody
/// else's: answering a notification of the account that is gone must not
/// go out as another one that happens to be in the same room.
pub fn split<'a>(prefixes: &[String], key: &'a str) -> Option<(usize, &'a str)> {
    for (index, prefix) in prefixes.iter().enumerate() {
        if !prefix.is_empty()
            && let Some(room_id) = key.strip_prefix(prefix.as_str())
        {
            return Some((index, room_id));
        }
    }
    // A room id as it is belongs to the account without a prefix only.
    let unprefixed = prefixes.iter().position(String::is_empty)?;
    key.starts_with('!').then_some((unprefixed, key))
}

/// The profile of `profiles` a room key belongs to, and the room id in it,
/// for what has only the profiles on disk (the iPhone's extensions). A
/// key without a profile's prefix is the default profile's.
pub fn split_by_profile<'a>(profiles: &[String], key: &'a str) -> (String, &'a str) {
    profiles
        .iter()
        .filter(|name| *name != "default")
        .find_map(|name| Some((name.clone(), key.strip_prefix(prefix_for(name).as_str())?)))
        .unwrap_or_else(|| ("default".to_owned(), key))
}

/// A profile name made from a server address: "https://matrix.example.org"
/// gives "matrix"; anything a folder cannot be called is left out.
pub fn profile_from_server(server: &str) -> String {
    let host = server
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    // A Matrix id: the part after the colon is the server.
    let host = match host.strip_prefix('@') {
        Some(id) => id.split_once(':').map_or(id, |(_, server)| server),
        None => host,
    };
    let first = host.split(['.', ':', '/']).next().unwrap_or_default();
    clean_profile(first)
}

pub fn clean_profile(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .collect::<String>()
        .to_lowercase()
}

/// The profile for a new account: the name typed (cleaned up), or one
/// made from the server, with a number added where that is taken. A name
/// typed that is taken is a problem to show.
pub fn pick_profile(typed: &str, server: &str, taken: &[String]) -> Result<String, String> {
    let mut profile = if typed.trim().is_empty() {
        profile_from_server(server)
    } else {
        clean_profile(typed)
    };
    if profile.is_empty() {
        profile = "account".to_owned();
    }
    // "dictionaries" is a folder of mothrix's own next to the profiles.
    let free = |name: &str| !taken.iter().any(|t| t == name) && name != "dictionaries";
    if free(&profile) {
        return Ok(profile);
    }
    if !typed.trim().is_empty() {
        return Err(format!("The name '{profile}' is taken; choose another."));
    }
    Ok((2..)
        .map(|number| format!("{profile}{number}"))
        .find(|name| free(name))
        .unwrap_or(profile))
}

/// The profile a sign-in really goes to: `profile` as the form picked it
/// among the running accounts, unless an account is on disk under that
/// name (`on_disk` says) without running: one left out at this start
/// (its store did not open, another mothrix has it), or one this mothrix
/// was not started with. A name made from the server (`typed` false)
/// then gets the next free number, instead of a refusal the user can do
/// nothing about; a name the user typed is theirs to change.
pub fn free_profile(
    profile: &str,
    typed: bool,
    on_disk: impl Fn(&str) -> bool,
) -> Result<String, String> {
    if !on_disk(profile) {
        return Ok(profile.to_owned());
    }
    if typed {
        return Err(format!(
            "There is already an account called '{profile}' on this device (it is not \
             running); choose another name."
        ));
    }
    let base = profile.trim_end_matches(|c: char| c.is_ascii_digit());
    let base = if base.is_empty() { profile } else { base };
    Ok((2..)
        .map(|number| format!("{base}{number}"))
        .find(|name| !on_disk(name))
        .unwrap_or_else(|| profile.to_owned()))
}

/// A sign-in a front end asks for ([`start_sign_in`]).
#[derive(Clone, PartialEq, Eq)]
pub struct SignIn {
    /// The profile the account is kept under.
    pub profile: String,
    /// The name as typed, for the room list; empty for none.
    pub label: String,
    pub server: String,
    pub user: String,
    pub password: String,
}

impl std::fmt::Debug for SignIn {
    /// Without the password: this may end up in a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignIn")
            .field("profile", &self.profile)
            .field("label", &self.label)
            .field("server", &self.server)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// What is wrong with a sign-in form, and in which of its fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormProblem {
    Server(String),
    Name(String),
}

impl FormProblem {
    pub fn text(&self) -> &str {
        match self {
            Self::Server(text) | Self::Name(text) => text,
        }
    }
}

impl SignIn {
    /// From what was typed into a sign-in form; `taken`: the profiles
    /// of the accounts there are. The name may be empty (the profile is
    /// then made from the server), and so may the user where the server
    /// is a full Matrix id, which then is the user too.
    ///
    /// (The form knows the accounts that run. One that is on disk and
    /// does not run is seen when the sign-in starts: [`free_profile`].)
    pub fn from_form(
        server: &str,
        user: &str,
        password: &str,
        name: &str,
        taken: &[String],
    ) -> Result<Self, FormProblem> {
        let server = server.trim();
        if server.is_empty() {
            return Err(FormProblem::Server(
                "Which server? Like matrix.org, or your full Matrix ID.".to_owned(),
            ));
        }
        let profile = pick_profile(name, server, taken).map_err(FormProblem::Name)?;
        let user = match user.trim() {
            "" if server.starts_with('@') => server,
            user => user,
        };
        Ok(Self {
            profile,
            label: name.trim().to_owned(),
            server: server.to_owned(),
            user: user.to_owned(),
            password: password.to_owned(),
        })
    }
}

/// News from a sign-in under way ([`start_sign_in`]).
pub enum SignInEvent {
    /// A line the user should see, like the address to open.
    Progress(String),
    /// It came to an end: the client, or what went wrong.
    /// [`Accounts::signed_in`] takes the client on.
    Done {
        profile: String,
        label: String,
        result: Result<Client, String>,
    },
}

/// Sign in, in a task of its own; `tell` hears how it goes. `browser`:
/// through the browser where the server offers that and no password was
/// typed (a password typed means: use it). Nothing of a sign-in that
/// failed stays on disk.
pub fn start_sign_in(
    request: SignIn,
    browser: bool,
    tell: impl Fn(SignInEvent) + Clone + Send + Sync + 'static,
) {
    tokio::spawn(async move {
        let SignIn {
            profile,
            label,
            server,
            user,
            password,
        } = request;
        // The name typed is the label; without one the profile was made
        // from the server.
        let free = free_profile(&profile, !label.is_empty(), |name| {
            matrix::Paths::for_profile(name).is_ok_and(|paths| matrix::has_session(&paths))
        });
        let (profile, result) = match free {
            Ok(profile) => {
                let signed_in =
                    sign_in(&profile, &server, user, password, browser, tell.clone()).await;
                (profile, signed_in.map_err(|error| format!("{error:#}")))
            }
            Err(taken) => (profile, Err(taken)),
        };
        tell(SignInEvent::Done {
            profile,
            label,
            result,
        });
    });
}

async fn sign_in(
    profile: &str,
    server: &str,
    user: String,
    password: String,
    browser: bool,
    tell: impl Fn(SignInEvent) + Clone + Send + Sync + 'static,
) -> anyhow::Result<Client> {
    let paths = matrix::Paths::for_profile(profile)?;
    // Before anything is written: what is there is that account's, also
    // when its session file cannot be read (`matrix::sign_in` refuses).
    if matrix::has_session(&paths) {
        anyhow::bail!("there is already an account called '{profile}'");
    }
    let password_login = !browser || !password.is_empty();
    matrix::sign_in(
        &paths,
        server,
        password_login,
        move || {
            if user.is_empty() || password.is_empty() {
                anyhow::bail!("this server signs in with a user name and a password; fill in both");
            }
            Ok((user, password))
        },
        move |line| tell(SignInEvent::Progress(line)),
    )
    .await
}

/// Start these signed-in profiles' accounts, in this order. Returns the
/// clients of those that started, and for the others why not (one that
/// is not signed in after all is left out silently).
pub async fn restore_all(
    profiles: Vec<String>,
) -> (Vec<(String, Client)>, Vec<(String, anyhow::Error)>) {
    let mut clients = Vec::new();
    let mut problems = Vec::new();
    for profile in profiles {
        let restored = match matrix::Paths::for_profile(&profile) {
            Ok(paths) => matrix::restore(&paths).await,
            Err(error) => Err(error),
        };
        match restored {
            Ok(Some(client)) => clients.push((profile, client)),
            Ok(None) => {}
            Err(error) => problems.push((profile, error)),
        }
    }
    (clients, problems)
}

/// Said when something is asked for a room whose key names no running
/// account ([`split`]).
const NO_ACCOUNT: &str = "That room's account is not signed in here.";

/// What [`Accounts`] keeps of one account.
struct Slot {
    /// What its updates are marked with. Positions shift when an account
    /// is signed out; this does not.
    id: u64,
    /// What goes in front of its room ids in a room key.
    prefix: String,
    commands: mpsc::UnboundedSender<Command>,
    /// Its SDK client (none behind a test's backend).
    client: Option<Client>,
    /// Its latest room list.
    rooms: Vec<RoomEntry>,
    /// What it found for the running search.
    found: Vec<SearchHit>,
    /// What it stores of the own sections ([`crate::sections`]).
    sections: crate::sections::Sync,
    /// Signed in during this run, and not looked at yet: what the UI
    /// keeps under its profile's name is an earlier account's (see
    /// `Accounts::welcome`).
    fresh: bool,
}

pub struct Accounts {
    /// In the order the UI lists them.
    slots: Vec<Slot>,
    next_id: u64,
    merged: mpsc::UnboundedSender<(u64, Update)>,
    /// Updates of all accounts, with the mark of the account they come
    /// from (for [`Accounts::deliver`]).
    pub updates: mpsc::UnboundedReceiver<(u64, Update)>,
    /// The running search.
    search: String,
    /// The account whose room is open.
    open: Option<usize>,
    /// The account a verification is going on with.
    verify: usize,
    /// Whether one is under way there (asked, emoji shown, not yet done
    /// or failed): the one dialog is its.
    verifying: bool,
}

/// Start a backend per account. Returns what the UI needs to know about
/// them, in the same order.
pub fn spawn(clients: Vec<(String, Client)>) -> (Accounts, Vec<Account>) {
    let (merged, updates) = mpsc::unbounded_channel();
    let mut accounts = Accounts {
        slots: Vec::new(),
        next_id: 0,
        merged,
        updates,
        search: String::new(),
        open: None,
        verify: 0,
        verifying: false,
    };
    let list = clients
        .into_iter()
        .map(|(profile, client)| accounts.add(profile, client))
        .collect();
    (accounts, list)
}

impl Accounts {
    /// Start the backend of one more account, which becomes the last one.
    pub fn add(&mut self, profile: String, client: Client) -> Account {
        let backend = matrix::spawn(client.clone());
        self.add_backend(profile, backend, Some(client))
    }

    /// Take on the account a sign-in ended with ([`SignInEvent::Done`]),
    /// under the name typed for it, if any. The front end lists it.
    pub fn signed_in(&mut self, profile: String, label: String, client: Client) -> Account {
        let mut account = self.add(profile, client);
        self.newly_signed_in();
        if !label.is_empty() {
            account.label = label;
        }
        account
    }

    /// The account added last was signed in just now, not restored: it
    /// starts with nothing here (see `welcome`).
    pub fn newly_signed_in(&mut self) {
        if let Some(slot) = self.slots.last_mut() {
            slot.fresh = true;
        }
    }

    /// Whether an account of this profile runs here.
    pub fn is_running(&self, profile: &str) -> bool {
        let prefix = prefix_for(profile);
        self.slots.iter().any(|slot| slot.prefix == prefix)
    }

    /// Remove an account that is on disk but does not run: one left out
    /// at the start (the second list of [`restore_all`]: its store could
    /// not be opened, its session file is damaged), which is not in the
    /// list [`Accounts::sign_out`] goes by. It is signed out on the server
    /// where its session still opens, and deleted here; its name is then
    /// free for a new sign-in. An error says why not (another mothrix
    /// runs with it), for the user to read.
    pub fn remove_left_out(
        &self,
        profile: &str,
    ) -> impl Future<Output = anyhow::Result<()>> + Send + 'static {
        let running = self.is_running(profile);
        let profile = profile.to_owned();
        async move {
            if running {
                anyhow::bail!("'{profile}' is running here: sign it out instead");
            }
            if profile.is_empty() || clean_profile(&profile) != profile {
                anyhow::bail!("'{profile}' is no account's name");
            }
            matrix::logout(&matrix::Paths::for_profile(&profile)?).await
        }
    }

    /// An account signed in during this run starts with nothing here: the
    /// sections and fold state the UI still holds under its profile's name
    /// (from the config file, of an account signed out with
    /// `mothrix logout` or deleted by hand) are another account's rooms,
    /// and would go up to the new one as its share. Before anything of
    /// the new account is heard or sent.
    fn welcome(&mut self, app: &mut impl Frontend) {
        if !self.slots.iter().any(|slot| slot.fresh) {
            return;
        }
        let prefixes = self.prefixes();
        for (account, slot) in self.slots.iter_mut().enumerate() {
            if std::mem::take(&mut slot.fresh) {
                Self::forget_sections(app, &prefixes, account);
            }
        }
    }

    /// Take the rooms of the account at `account` out of the UI's
    /// sections, and what was folded for it.
    fn forget_sections(app: &mut impl Frontend, prefixes: &[String], account: usize) {
        let state = app.state_mut();
        let mut changed = state.sections.forget_account(prefixes, account);
        if let Some(profile) = state
            .accounts
            .get(account)
            .map(|known| known.profile.clone())
        {
            changed |= state.folded.forget_profile(&profile);
        }
        state.config_dirty |= changed;
    }

    /// Sign an account out: off the server and off the disk (in a task of
    /// its own), out of the UI at once. Returns its profile.
    ///
    /// Its rooms leave the sections and its fold state goes: the profile's
    /// name is free again, and another account signed in under it would
    /// else take them for its own. (What the account stores of its
    /// sections stays with it, for its other devices and the next
    /// sign-in.)
    pub fn sign_out(&mut self, app: &mut impl Frontend, account: usize) -> Option<String> {
        if account < self.slots.len() {
            Self::forget_sections(app, &self.prefixes(), account);
        }
        let (profile, client) = self.remove(app, account)?;
        if let Some(client) = client
            && let Ok(paths) = matrix::Paths::for_profile(&profile)
        {
            tokio::spawn(matrix::sign_out(client, paths));
        }
        Some(profile)
    }

    /// Take on the backend of one more account: the one `matrix::spawn`
    /// started, or a stand-in in tests.
    pub fn add_backend(
        &mut self,
        profile: String,
        mut backend: matrix::Backend,
        client: Option<Client>,
    ) -> Account {
        let id = self.next_id;
        self.next_id += 1;
        let tx = self.merged.clone();
        tokio::spawn(async move {
            while let Some(update) = backend.updates.recv().await {
                if tx.send((id, update)).is_err() {
                    return;
                }
            }
            // The backend ended (it could not go on, or was signed out):
            // a verification with it ended too. Nothing else would say
            // so, and the one dialog would stay this account's.
            let _ = tx.send((id, Update::Verify(None)));
        });
        self.slots.push(Slot {
            id,
            prefix: prefix_for(&profile),
            commands: backend.commands,
            client,
            rooms: Vec::new(),
            found: Vec::new(),
            sections: Default::default(),
            fresh: false,
        });
        Account {
            label: profile.clone(),
            profile,
            user_id: backend.user_id,
            own_name: None,
            sync: SyncStatus::Connecting,
            verified: None,
        }
    }

    /// Let two accounts change places in the list.
    pub fn swap(&mut self, app: &mut impl Frontend, a: usize, b: usize) {
        if a == b || a >= self.slots.len() || b >= self.slots.len() {
            return;
        }
        self.slots.swap(a, b);
        let other = |index: usize| match index {
            index if index == a => b,
            index if index == b => a,
            index => index,
        };
        self.open = self.open.map(other);
        self.verify = other(self.verify);
        self.renumber_rooms();
        app.state_mut().swap_accounts(a, b);
    }

    /// Stop the backend of an account and take it out of the UI. Returns
    /// its profile and client, for signing out.
    pub fn remove(
        &mut self,
        app: &mut impl Frontend,
        account: usize,
    ) -> Option<(String, Option<Client>)> {
        if account >= self.slots.len() {
            return None;
        }
        // Without its sender the backend winds down.
        let slot = self.slots.remove(account);
        self.open = match self.open {
            Some(open) if open == account => None,
            Some(open) if open > account => Some(open - 1),
            other => other,
        };
        // A verification with it ends with it.
        if self.verify == account {
            app.on_update(Update::Verify(None));
            self.verifying = false;
        }
        self.verify = match self.verify {
            verify if verify == account => 0,
            verify if verify > account => verify - 1,
            verify => verify,
        };
        self.renumber_rooms();
        let profile = app.state().accounts[account].profile.clone();
        let closed = app.state_mut().forget_account(account);
        app.account_gone(closed);
        Some((profile, slot.client))
    }

    /// Each room says which account it belongs to, by its position.
    fn renumber_rooms(&mut self) {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            for room in &mut slot.rooms {
                room.account = index;
            }
        }
    }

    /// The running client of the account a room (by its key) belongs to,
    /// and the room's id there: for what is done to a room outside the
    /// conversation followed (answering from a notification). A second
    /// client on the same store beside the running one must never be
    /// made: both would encrypt with keys the other has moved on from.
    pub fn client_of(&self, key: &str) -> Option<(Client, String)> {
        let (account, room_id) = split(&self.prefixes(), key)?;
        let client = self.slots.get(account)?.client.clone()?;
        Some((client, room_id.to_owned()))
    }

    /// What goes in front of the room ids of each account.
    fn prefixes(&self) -> Vec<String> {
        self.slots.iter().map(|slot| slot.prefix.clone()).collect()
    }

    /// Hand an update of one account to the UI.
    pub fn deliver(&mut self, app: &mut impl Frontend, id: u64, mut update: Update) {
        // Late news from an account that was signed out meanwhile.
        let Some(account) = self.slots.iter().position(|slot| slot.id == id) else {
            return;
        };
        // News is no action of the user: which account the last one named
        // says nothing about the commands this leads to (a link to
        // preview, a receipt).
        app.state_mut().named_account = None;
        self.welcome(app);
        // Inside the UI a room is known by its key.
        let prefix = &self.slots[account].prefix;
        for room_id in update.room_ids_mut() {
            *room_id = format!("{prefix}{room_id}");
        }
        match update {
            Update::Sections(said) => {
                let prefixes = self.prefixes();
                if self.slots[account].sections.said(
                    &mut app.state_mut().sections,
                    &prefixes,
                    account,
                    said,
                ) {
                    app.state_mut().config_dirty = true;
                }
            }
            Update::Rooms(mut rooms) => {
                for room in &mut rooms {
                    room.account = account;
                }
                self.slots[account].rooms = rooms;
                let all = self.slots.iter().flat_map(|slot| slot.rooms.clone());
                app.on_update(Update::Rooms(all.collect()));
            }
            Update::SearchResults { query, mut hits } => {
                if query != self.search {
                    return;
                }
                // The same room can turn up in two accounts.
                if app.state().accounts.len() > 1 {
                    let label = &app.state().accounts[account].label;
                    for hit in &mut hits {
                        hit.room_name = format!("{label} › {}", hit.room_name);
                    }
                }
                self.slots[account].found = hits;
                let mut all: Vec<SearchHit> = self
                    .slots
                    .iter()
                    .flat_map(|slot| slot.found.clone())
                    .collect();
                all.sort_by_key(|hit| std::cmp::Reverse(hit.ts_ms));
                app.on_update(Update::SearchResults { query, hits: all });
            }
            // Read by the open room's account; sent by the target's.
            Update::ToForward {
                to,
                content,
                encrypted_file,
            } => {
                let Some((target, room_id)) = split(&self.prefixes(), &to) else {
                    app.on_update(Update::Notice(NO_ACCOUNT.to_owned()));
                    return;
                };
                let clients = (
                    self.slots[account].client.clone(),
                    self.slots.get(target).and_then(|slot| slot.client.clone()),
                );
                match clients {
                    // Into a room of another account: its homeserver may
                    // not be able to fetch a file from this one's, so the
                    // file goes along.
                    (Some(from), Some(into)) if target != account => {
                        let commands = self.slots[target].commands.clone();
                        let (news, room_id) = (self.merged.clone(), room_id.to_owned());
                        tokio::spawn(async move {
                            match crate::forward::carry(&from, &into, content).await {
                                Ok(content) => {
                                    let _ = commands.send(Command::SendForwarded {
                                        room_id,
                                        content,
                                        encrypted_file,
                                    });
                                }
                                Err(error) => {
                                    let _ = news.send((id, Update::Notice(format!("{error:#}"))));
                                }
                            }
                        });
                    }
                    _ => self.send(
                        &*app,
                        Command::SendForwarded {
                            room_id: to,
                            content,
                            encrypted_file,
                        },
                    ),
                }
            }
            Update::Sync(status) => app.state_mut().accounts[account].sync = status,
            Update::OwnName(name) => app.state_mut().accounts[account].own_name = Some(name),
            Update::Verified(verified) => {
                app.state_mut().accounts[account].verified = Some(verified)
            }
            Update::Verify(state) => match state {
                // One at a time: there is one dialog, and a request for
                // another account would take it, and the keys meant for
                // the first, away. That request is turned down.
                Some(state) if self.verifying && self.verify != account => {
                    // Its end is the answer to our turning it down: said
                    // once already.
                    if matches!(
                        state,
                        crate::model::VerifyState::Done | crate::model::VerifyState::Failed(_)
                    ) {
                        return;
                    }
                    let _ = self.slots[account]
                        .commands
                        .send(Command::Verify(crate::verify::VerifyInput::Cancel));
                    let label = app.state().accounts[account].label.clone();
                    app.on_update(Update::Notice(format!(
                        "A verification for {label} was turned down: another one is under way."
                    )));
                }
                // Answers go back to the account that asked.
                Some(state) => {
                    self.verify = account;
                    self.verifying = !matches!(
                        state,
                        crate::model::VerifyState::Done | crate::model::VerifyState::Failed(_)
                    );
                    app.on_update(Update::Verify(Some(state)));
                }
                None if self.verify == account => {
                    self.verifying = false;
                    app.on_update(Update::Verify(None));
                }
                None => {}
            },
            Update::Notice(text)
                if app.state().accounts.len() > 1 && app.active_account() != account =>
            {
                let label = app.state().accounts[account].label.clone();
                app.on_update(Update::Notice(format!("[{label}] {text}")));
            }
            other => app.on_update(other),
        }
    }

    /// Tell each account whose share of the UI's sections changed (by the
    /// user, here): it keeps them for its other devices. Cheap when nothing
    /// changed; the UIs call it after every round of their loop.
    pub fn sync_sections(&mut self, app: &mut impl Frontend) {
        self.welcome(app);
        let prefixes = self.prefixes();
        let sections = &app.state_mut().sections;
        for (account, slot) in self.slots.iter_mut().enumerate() {
            if let Some(share) = slot.sections.to_send(sections, &prefixes, account) {
                let _ = slot.commands.send(Command::SetSections(share));
            }
        }
    }

    /// Send a command of the UI to the account it is meant for.
    pub fn send(&mut self, app: &impl Frontend, mut command: Command) {
        if let Command::Search {
            query,
            room_id: None,
        } = &command
        {
            // Everywhere: all accounts, or the one being shown.
            self.start_search(query);
            for (account, slot) in self.slots.iter().enumerate() {
                if app
                    .state()
                    .account_filter
                    .is_none_or(|shown| shown == account)
                {
                    let _ = slot.commands.send(Command::Search {
                        query: query.clone(),
                        room_id: None,
                    });
                }
            }
            return;
        }
        // About a room: goes to the account the room's key names, and to
        // nobody when that account is not running.
        let prefixes = self.prefixes();
        let owner = match command.room_id_mut() {
            Some(key) => {
                let Some((account, room_id)) = split(&prefixes, key) else {
                    tracing::warn!("a command for a room of no running account was dropped");
                    if let Some(slot) = self.slots.first() {
                        let _ = self
                            .merged
                            .send((slot.id, Update::Notice(NO_ACCOUNT.to_owned())));
                    }
                    return;
                };
                *key = room_id.to_owned();
                Some(account)
            }
            None => None,
        };
        let account = match (&command, owner) {
            // Only one room is open at a time; the account that showed
            // the last one can let go of it.
            (Command::Open(_), Some(account)) => {
                if let Some(previous) = self.open.replace(account)
                    && previous != account
                {
                    let _ = self.slots[previous].commands.send(Command::Close);
                }
                account
            }
            (Command::Search { query, .. }, Some(account)) => {
                self.start_search(query);
                account
            }
            (_, Some(account)) => account,
            // Answers go back to the account that asked.
            (Command::Verify(_), None) => self.verify,
            // The message is read where it is shown.
            (Command::Forward { .. }, None) => self.open.unwrap_or_else(|| app.active_account()),
            (_, None) => app.active_account(),
        };
        if let Some(slot) = self.slots.get(account) {
            let _ = slot.commands.send(command);
        }
    }

    /// A new search: what was found for the last one goes.
    fn start_search(&mut self, query: &str) {
        self.search = query.to_owned();
        for slot in &mut self.slots {
            slot.found.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_form_names_a_free_profile_and_takes_the_user_from_a_matrix_id() {
        let taken = ["default".to_owned(), "work".to_owned()];
        let form = |server: &str, user: &str, name: &str| {
            SignIn::from_form(server, user, "pw", name, &taken)
        };
        assert!(matches!(form("  ", "amy", ""), Err(FormProblem::Server(_))));
        assert!(matches!(
            form("x.example", "amy", "Work"),
            Err(FormProblem::Name(_))
        ));
        let typed = form(" https://chat.example.org ", " amy ", " Club ").unwrap();
        assert_eq!(
            (typed.profile.as_str(), typed.label.as_str()),
            ("club", "Club")
        );
        assert_eq!(
            (typed.server.as_str(), typed.user.as_str()),
            ("https://chat.example.org", "amy")
        );
        // A full Matrix id is the user too; the profile is its server's,
        // numbered where that is taken.
        let id = form("@amy:work.example", "", "").unwrap();
        assert_eq!(
            (id.profile.as_str(), id.user.as_str()),
            ("work2", "@amy:work.example")
        );
        assert_eq!(form("@amy:work.example", "amy", "").unwrap().user, "amy");
        // The password is not in what a log would show.
        assert!(!format!("{id:?}").contains("pw"));
    }

    #[test]
    fn a_name_made_from_the_server_passes_over_accounts_that_do_not_run() {
        // "work" runs, so the form made "work2"; that one is on disk too,
        // left out at this start.
        let on_disk = |name: &str| ["default", "work", "work2"].contains(&name);
        assert_eq!(free_profile("work2", false, on_disk), Ok("work3".into()));
        assert_eq!(free_profile("club", false, on_disk), Ok("club".into()));
        // A name the user typed is not changed for them.
        assert!(free_profile("work2", true, on_disk).is_err());
        assert_eq!(free_profile("club", true, on_disk), Ok("club".into()));
        // The first of its kind.
        let on_disk = |name: &str| name == "matrix";
        assert_eq!(free_profile("matrix", false, on_disk), Ok("matrix2".into()));
    }

    #[test]
    fn a_room_key_names_its_profile() {
        let profiles = ["default".to_owned(), "work".to_owned()];
        assert_eq!(
            split_by_profile(&profiles, "!abc:x.org"),
            ("default".into(), "!abc:x.org")
        );
        assert_eq!(
            split_by_profile(&profiles, "work/!abc:x.org"),
            ("work".into(), "!abc:x.org")
        );
        // Without the default profile signed in, an unprefixed key is
        // still its.
        assert_eq!(
            split_by_profile(&profiles[1..], "!abc:x.org"),
            ("default".into(), "!abc:x.org")
        );
    }
}
