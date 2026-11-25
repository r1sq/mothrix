// SPDX-License-Identifier: MPL-2.0

//! `Accounts` (mothrix-core) serving the terminal UI's `App`, against
//! stand-in backends.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::{
    accounts::*,
    app::App,
    matrix::{self, Command, Update},
    model::{Item, RoomEntry, SearchHit, VerifyState},
    theme,
    verify::VerifyInput,
};

/// The other ends of a stand-in backend: what it was sent, and a way to
/// send updates as if from the server.
struct Fake {
    commands: mpsc::UnboundedReceiver<Command>,
    updates: mpsc::UnboundedSender<Update>,
}

impl Fake {
    fn sent(&mut self) -> Vec<String> {
        let mut sent = Vec::new();
        while let Ok(command) = self.commands.try_recv() {
            sent.push(format!("{command:?}"));
        }
        sent
    }
}

fn fake(user_id: &str) -> (matrix::Backend, Fake) {
    let (update_tx, updates) = mpsc::unbounded_channel();
    let (commands, command_rx) = mpsc::unbounded_channel();
    let backend = matrix::Backend {
        updates,
        commands,
        user_id: user_id.to_owned(),
    };
    (
        backend,
        Fake {
            commands: command_rx,
            updates: update_tx,
        },
    )
}

/// Accounts "default" and "work", each behind a stand-in.
fn two_accounts() -> (Accounts, App, Fake, Fake) {
    let (mut accounts, _) = spawn(Vec::new());
    let (home, home_fake) = fake("@me:home");
    let (work, work_fake) = fake("@me:work");
    let mut app = App::new(theme::THEMES[0], String::new());
    app.state.accounts = vec![
        accounts.add_backend("default".into(), home, None),
        accounts.add_backend("work".into(), work, None),
    ];
    (accounts, app, home_fake, work_fake)
}

/// Hand everything the stand-ins sent on to the app.
async fn pump(accounts: &mut Accounts, app: &mut App) {
    while let Ok(Some((id, update))) =
        tokio::time::timeout(Duration::from_millis(50), accounts.updates.recv()).await
    {
        accounts.deliver(app, id, update);
    }
}

fn room(id: &str, name: &str) -> RoomEntry {
    RoomEntry {
        id: id.into(),
        name: name.into(),
        ..RoomEntry::default()
    }
}

#[tokio::test]
async fn rooms_of_each_account_arrive_under_their_keys() {
    let (mut accounts, mut app, home, work) = two_accounts();
    home.updates
        .send(Update::Rooms(vec![room("!a", "Lunch")]))
        .unwrap();
    work.updates
        .send(Update::Rooms(vec![room("!a", "Standup")]))
        .unwrap();
    pump(&mut accounts, &mut app).await;
    let rooms: Vec<_> = app
        .state
        .rooms
        .iter()
        .map(|room| (room.id.as_str(), room.account))
        .collect();
    assert_eq!(rooms, [("!a", 0), ("work/!a", 1)]);

    // And so does whatever else names a room.
    app.state.open_room = Some("work/!a".into());
    work.updates
        .send(Update::Timeline {
            room_id: "!a".into(),
            items: vec![Item::default()],
        })
        .unwrap();
    pump(&mut accounts, &mut app).await;
    assert_eq!(app.state.items.len(), 1);
}

#[tokio::test]
async fn commands_go_to_the_account_of_their_room() {
    let (mut accounts, app, mut home, mut work) = two_accounts();
    accounts.send(&app, Command::Open("!a".into()));
    assert_eq!(home.sent(), [r#"Open("!a")"#]);
    // Opening a room of the other account lets go of the first one.
    accounts.send(&app, Command::Open("work/!a".into()));
    assert_eq!(home.sent(), ["Close"]);
    assert_eq!(work.sent(), [r#"Open("!a")"#]);
    accounts.send(
        &app,
        Command::SetFavourite {
            room_id: "work/!b".into(),
            value: true,
        },
    );
    assert_eq!(
        work.sent(),
        [r#"SetFavourite { room_id: "!b", value: true }"#]
    );
    assert!(home.sent().is_empty());
}

#[tokio::test]
async fn a_search_everywhere_merges_the_accounts_newest_first() {
    let (mut accounts, mut app, mut home, mut work) = two_accounts();
    accounts.send(
        &app,
        Command::Search {
            query: "lunch".into(),
            room_id: None,
        },
    );
    assert_eq!(home.sent().len(), 1);
    assert_eq!(work.sent().len(), 1);
    let hit = |room_id: &str, ts_ms| SearchHit {
        room_id: room_id.into(),
        room_name: "Room".into(),
        ts_ms,
        ..SearchHit::default()
    };
    home.updates
        .send(Update::SearchResults {
            query: "lunch".into(),
            hits: vec![hit("!a", 1)],
        })
        .unwrap();
    work.updates
        .send(Update::SearchResults {
            query: "lunch".into(),
            hits: vec![hit("!b", 2)],
        })
        .unwrap();
    // An answer to an older search is left out.
    work.updates
        .send(Update::SearchResults {
            query: "old".into(),
            hits: vec![hit("!c", 3)],
        })
        .unwrap();
    app.overlays.open(crate::app::Search {
        searched: Some(("lunch".into(), true)),
        ..crate::app::Search::default()
    });
    pump(&mut accounts, &mut app).await;
    let hits: Vec<_> = app
        .overlays
        .get::<crate::app::Search>()
        .map(|search| {
            search
                .hits
                .iter()
                .map(|hit| (hit.room_id.clone(), hit.room_name.clone()))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        hits,
        [
            ("work/!b".to_owned(), "work › Room".to_owned()),
            ("!a".to_owned(), "default › Room".to_owned()),
        ]
    );
}

#[tokio::test]
async fn news_of_an_account_not_shown_says_whose_it_is() {
    let (mut accounts, mut app, _home, work) = two_accounts();
    work.updates
        .send(Update::Notice("Signed in.".into()))
        .unwrap();
    pump(&mut accounts, &mut app).await;
    assert_eq!(app.notice.as_deref(), Some("[work] Signed in."));
}

#[test]
fn room_keys_carry_the_profile_except_for_the_default_one() {
    let prefixes: Vec<String> = ["default", "work", "school"]
        .iter()
        .map(|profile| prefix_for(profile))
        .collect();
    assert_eq!(prefixes, ["", "work/", "school/"]);
    assert_eq!(split(&prefixes, "!a:x.org"), (0, "!a:x.org"));
    assert_eq!(split(&prefixes, "work/!a:x.org"), (1, "!a:x.org"));
    assert_eq!(split(&prefixes, "school/!b:y.org"), (2, "!b:y.org"));
}

#[test]
fn a_single_named_profile_gets_everything() {
    let prefixes = vec![prefix_for("work")];
    assert_eq!(split(&prefixes, "work/!a:x.org"), (0, "!a:x.org"));
    // Not ours by its key, but there is nobody else to ask.
    assert_eq!(split(&prefixes, "!a:x.org"), (0, "!a:x.org"));
}

#[tokio::test]
async fn a_verification_stays_with_its_account() {
    let (mut accounts, mut app, mut home, mut work) = two_accounts();
    work.updates
        .send(Update::Verify(Some(VerifyState::Incoming {
            device: "Phone".into(),
        })))
        .unwrap();
    pump(&mut accounts, &mut app).await;
    assert!(app.overlays.is_open::<VerifyState>());
    // The answer goes back to the account that asked.
    accounts.send(&app, Command::Verify(VerifyInput::Accept));
    assert_eq!(work.sent(), ["Verify(Accept)"]);
    assert!(home.sent().is_empty());
    // News that it ended from the other account changes nothing.
    home.updates.send(Update::Verify(None)).unwrap();
    pump(&mut accounts, &mut app).await;
    assert!(app.overlays.is_open::<VerifyState>());
    // Signing that account out ends it.
    accounts.remove(&mut app, 1);
    assert!(!app.overlays.is_open::<VerifyState>());
}

#[tokio::test]
async fn sections_come_from_each_account_and_changes_here_go_back_to_it() {
    let (mut accounts, mut app, mut home, mut work) = two_accounts();
    app.state.sections = crate::sections::OwnSections::new(vec![crate::model::Section {
        name: "Mine".into(),
        rooms: vec!["!old".into()],
    }]);
    // Nothing goes out before an account said what it stores.
    accounts.sync_sections(&mut app);
    assert!(home.sent().is_empty());

    // Home stores nothing yet: this device's sections go up to it.
    home.updates.send(Update::Sections(None)).unwrap();
    // Work stores a section of its own: it is merged in, under keys.
    work.updates
        .send(Update::Sections(Some(vec![crate::model::Section {
            name: "Clients".into(),
            rooms: vec!["!c".into()],
        }])))
        .unwrap();
    pump(&mut accounts, &mut app).await;
    assert_eq!(app.state.sections.len(), 2);
    assert_eq!(app.state.sections[1].name, "Clients");
    assert_eq!(app.state.sections[1].rooms, ["work/!c"]);
    accounts.sync_sections(&mut app);
    let sent = home.sent();
    assert!(
        sent.len() == 1 && sent[0].starts_with("SetSections") && sent[0].contains("!old"),
        "{sent:?}"
    );
    assert!(work.sent().is_empty());

    // A room of work moved here into "Mine": only work hears of it, as ids.
    // (An emptied section goes.)
    app.state.sections.assign("work/!c", Some("Mine")).unwrap();
    assert_eq!(app.state.sections.len(), 1);
    accounts.sync_sections(&mut app);
    let sent = work.sent();
    assert!(
        sent.len() == 1 && sent[0].contains("Mine") && sent[0].contains("\"!c\""),
        "{sent:?}"
    );
    assert!(!sent[0].contains("work/"), "{sent:?}");
    assert!(home.sent().is_empty());
}

#[tokio::test]
async fn a_message_is_read_where_it_is_shown_and_sent_by_the_target_s_account() {
    let (mut accounts, mut app, mut home, mut work) = two_accounts();
    accounts.send(&app, Command::Open("work/!a".into()));
    work.sent();
    // Into a room of the other account: the open room's account reads it…
    accounts.send(
        &app,
        Command::Forward {
            event_id: "$1".into(),
            room_id: "!b".into(),
        },
    );
    assert!(home.sent().is_empty());
    let sent = work.sent();
    assert!(
        sent.len() == 1 && sent[0].contains("Forward") && sent[0].contains(r#"room_id: "!b""#),
        "{sent:?}"
    );
    work.updates
        .send(Update::ToForward {
            to: "!b".into(),
            content: r#"{"msgtype":"m.text","body":"hi"}"#.into(),
            encrypted_file: false,
        })
        .unwrap();
    pump(&mut accounts, &mut app).await;
    // …and the target's account sends it, into its room by its id.
    let sent = home.sent();
    assert!(
        sent.len() == 1
            && sent[0].starts_with("SendForwarded")
            && sent[0].contains(r#"room_id: "!b""#),
        "{sent:?}"
    );
    assert!(work.sent().is_empty());

    // Within one account the same way round.
    accounts.send(
        &app,
        Command::Forward {
            event_id: "$1".into(),
            room_id: "work/!c".into(),
        },
    );
    assert!(work.sent()[0].contains(r#"room_id: "work/!c""#));
    work.updates
        .send(Update::ToForward {
            to: "work/!c".into(),
            content: "{}".into(),
            encrypted_file: true,
        })
        .unwrap();
    pump(&mut accounts, &mut app).await;
    let sent = work.sent();
    assert!(
        sent.len() == 1
            && sent[0].contains(r#"room_id: "!c""#)
            && sent[0].contains("encrypted_file: true"),
        "{sent:?}"
    );
}

#[tokio::test]
async fn swapped_accounts_take_their_rooms_commands_and_sections_along() {
    let (mut accounts, mut app, mut home, mut work) = two_accounts();
    home.updates
        .send(Update::Rooms(vec![room("!a", "Lunch")]))
        .unwrap();
    work.updates
        .send(Update::Rooms(vec![room("!b", "Standup")]))
        .unwrap();
    pump(&mut accounts, &mut app).await;
    accounts.swap(&mut app, 0, 1);
    assert_eq!(app.state.accounts[0].profile, "work");

    // A new room list of home comes after work's, under home's keys.
    home.updates
        .send(Update::Rooms(vec![room("!a", "Lunch"), room("!c", "Gym")]))
        .unwrap();
    pump(&mut accounts, &mut app).await;
    let rooms: Vec<_> = app
        .state
        .rooms
        .iter()
        .map(|room| (room.id.as_str(), room.account))
        .collect();
    assert_eq!(rooms, [("work/!b", 0), ("!a", 1), ("!c", 1)]);

    accounts.send(&app, Command::Open("!c".into()));
    assert_eq!(home.sent(), [r#"Open("!c")"#]);
    assert!(work.sent().is_empty());

    // Home's share of the sections still goes to home.
    home.updates.send(Update::Sections(None)).unwrap();
    pump(&mut accounts, &mut app).await;
    app.state.sections.assign("!a", Some("Mine")).unwrap();
    accounts.sync_sections(&mut app);
    let sent = home.sent();
    assert!(sent.len() == 1 && sent[0].contains("Mine"), "{sent:?}");
    assert!(work.sent().is_empty());
}

#[tokio::test]
async fn one_verification_at_a_time_and_the_other_account_s_is_turned_down() {
    let (mut accounts, mut app, mut home, mut work) = two_accounts();
    let asks = |device: &str| {
        Update::Verify(Some(VerifyState::Incoming {
            device: device.into(),
        }))
    };
    home.updates.send(asks("Phone")).unwrap();
    pump(&mut accounts, &mut app).await;
    work.updates.send(asks("Laptop")).unwrap();
    pump(&mut accounts, &mut app).await;
    // The dialog is still the first one's, and the second was told no.
    assert!(matches!(
        app.overlays.get::<VerifyState>(),
        Some(VerifyState::Incoming { device }) if device == "Phone"
    ));
    assert!(work.sent().iter().any(|sent| sent.contains("Cancel")));
    assert!(home.sent().is_empty());
    // Once the first is done, the other account can verify.
    home.updates
        .send(Update::Verify(Some(VerifyState::Done)))
        .unwrap();
    pump(&mut accounts, &mut app).await;
    work.updates.send(asks("Laptop")).unwrap();
    pump(&mut accounts, &mut app).await;
    assert!(matches!(
        app.overlays.get::<VerifyState>(),
        Some(VerifyState::Incoming { device }) if device == "Laptop"
    ));
}

#[tokio::test]
async fn a_link_s_preview_is_asked_of_its_room_s_account_whoever_was_named_last() {
    let (mut accounts, mut app, mut home, mut work) = two_accounts();
    home.updates
        .send(Update::Rooms(vec![room("!a", "Lunch")]))
        .unwrap();
    work.updates
        .send(Update::Rooms(vec![room("!w", "Standup")]))
        .unwrap();
    pump(&mut accounts, &mut app).await;
    app.state.prefs.link_previews = true;
    app.state.open_room = Some("!a".into());
    // A key in the room info of the other account's room named that
    // account; then a message with a link arrives in the open room.
    app.state.named_account = Some(1);
    home.updates
        .send(Update::Timeline {
            room_id: "!a".into(),
            items: vec![Item {
                event_id: "$1".into(),
                body: "see https://example.org/a".into(),
                ..Item::default()
            }],
        })
        .unwrap();
    pump(&mut accounts, &mut app).await;
    // News ends what the last action named.
    assert_eq!(app.state.named_account, None);
    for command in app.take_commands() {
        accounts.send(&app, command);
    }
    assert!(work.sent().is_empty());
    let asked = home.sent();
    assert!(
        asked
            .iter()
            .any(|command| command
                == r#"PreviewLink { room_id: "!a", url: "https://example.org/a" }"#),
        "{asked:?}"
    );
}
