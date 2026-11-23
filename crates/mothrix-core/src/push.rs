// SPDX-License-Identifier: MPL-2.0

//! Push notifications for phones: the "pushers" an account has at its
//! homeserver. A pusher tells the homeserver to hand each message that
//! should notify to a push gateway (Sygnal), which passes it on to Apple
//! (APNs) for one device. mothrix asks for `event_id_only`: the gateway gets
//! the room and event ids and the unread count, never the text; the phone
//! fetches and decrypts the message itself.
//!
//! `mothrix pushers` lists, adds and removes them by hand (for trying a
//! gateway out); the iOS app registers its own. [`notification`] is what
//! the app's notification extension shows for a push.

use anyhow::Result;
use matrix_sdk::ruma::{
    EventId, RoomId,
    events::{
        AnySyncMessageLikeEvent, AnySyncTimelineEvent, SyncMessageLikeEvent,
        room::message::MessageType,
    },
};
use matrix_sdk::{
    Client,
    ruma::{
        api::client::push::{
            Pusher, PusherIds, PusherInit, PusherKind, get_pushers::v3 as get_pushers,
        },
        push::{HttpPusherData, PushFormat},
        serde::JsonObject,
    },
};
use matrix_sdk_ui::notification_client::{
    NotificationClient, NotificationEvent, NotificationProcessSetup, NotificationStatus,
};
use serde_json::json;

/// Where pushes for one device go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The gateway's notify address, like
    /// `https://push.example.org/_matrix/push/v1/notify`.
    pub gateway: String,
    /// The app at the gateway: which APNs key and topic it uses (for
    /// mothrix `de.tmrk.mothrix`, or `de.tmrk.mothrix.dev` for builds from Xcode,
    /// which go to Apple's sandbox).
    pub app_id: String,
    /// The device's APNs token, base64 (Sygnal makes hex of it for Apple).
    pub pushkey: String,
    /// Shown to the user in other clients' lists of pushers.
    pub device_name: String,
    /// Language of the notifications, like "de".
    pub lang: String,
    /// mothrix's name for the account (its profile): the push carries it,
    /// so that the phone knows whose store to open.
    pub profile: String,
}

/// A pusher as the homeserver has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    pub app_id: String,
    pub pushkey: String,
    pub app_name: String,
    pub device_name: String,
    /// The gateway, for http pushers; empty for e-mail ones.
    pub gateway: String,
    pub event_id_only: bool,
}

/// The pusher for `target`: an http one with `event_id_only`, and a
/// payload that lets the phone's notification extension run
/// (`mutable-content`) and says "New message" if it cannot decrypt.
pub fn pusher(target: &Target) -> Pusher {
    let mut data = HttpPusherData::new(target.gateway.clone());
    data.format = Some(PushFormat::EventIdOnly);
    let mut extra = JsonObject::new();
    extra.insert(
        "default_payload".to_owned(),
        json!({
            "aps": {
                "mutable-content": 1,
                "alert": { "title": "mothrix", "body": "New message" },
            },
            "mothrix_profile": target.profile,
        }),
    );
    data.data = extra;
    PusherInit {
        ids: PusherIds::new(target.pushkey.clone(), target.app_id.clone()),
        kind: PusherKind::Http(data),
        app_display_name: "mothrix".to_owned(),
        device_display_name: target.device_name.clone(),
        profile_tag: None,
        lang: target.lang.clone(),
    }
    .into()
}

/// Add (or update) the pusher for `target`, next to the account's others.
pub async fn add(client: &Client, target: &Target) -> Result<()> {
    client.pusher().set(pusher(target), true).await?;
    Ok(())
}

/// Remove the pusher of this app and device.
pub async fn remove(client: &Client, app_id: &str, pushkey: &str) -> Result<()> {
    let ids = PusherIds::new(pushkey.to_owned(), app_id.to_owned());
    client.pusher().delete(ids).await?;
    Ok(())
}

/// The account's pushers, of every app.
pub async fn list(client: &Client) -> Result<Vec<Registered>> {
    let response = client.send(get_pushers::Request::new()).await?;
    Ok(response.pushers.iter().map(registered).collect())
}

fn registered(pusher: &Pusher) -> Registered {
    let (gateway, event_id_only) = match &pusher.kind {
        PusherKind::Http(data) => (
            data.url.clone(),
            matches!(data.format, Some(PushFormat::EventIdOnly)),
        ),
        _ => (String::new(), false),
    };
    Registered {
        app_id: pusher.ids.app_id.clone(),
        pushkey: pusher.ids.pushkey.clone(),
        app_name: pusher.app_display_name.clone(),
        device_name: pusher.device_display_name.clone(),
        gateway,
        event_id_only,
    }
}

/// What a notification about one message says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notification {
    /// The room's name.
    pub room_name: String,
    /// Who sent it (their name in the room).
    pub sender: String,
    /// The text, or what was sent ("sent a picture"); empty when it could
    /// not be read (the notification then keeps "New message").
    pub body: String,
    /// A direct chat: the sender's name is the title, no room name.
    pub direct: bool,
    /// The thread's first message, for a reply in a thread.
    pub thread_root: Option<String>,
    /// Whether the push rules ask for a sound.
    pub noisy: bool,
}

/// Fetch and decrypt the event a push was about, in a process of its own
/// (the iOS notification extension, which shares the store with the app:
/// [`crate::matrix::share_stores`]). `None`: nothing to show (filtered
/// out by push rules, redacted, or not found).
pub async fn notification(
    client: &Client,
    room_id: &str,
    event_id: &str,
) -> Result<Option<Notification>> {
    let room_id = <&RoomId>::try_from(room_id)?;
    let event_id = <&EventId>::try_from(event_id)?;
    let notifications =
        NotificationClient::new(client.clone(), NotificationProcessSetup::MultipleProcesses)
            .await?;
    let NotificationStatus::Event(item) = notifications.get_notification(room_id, event_id).await?
    else {
        return Ok(None);
    };
    let sender = item
        .sender_display_name
        .clone()
        .unwrap_or_else(|| item.event.sender().localpart().to_owned());
    let body = match &item.event {
        NotificationEvent::Timeline(event) => describe(event),
        NotificationEvent::Invite(_) => "invited you".to_owned(),
    };
    Ok(Some(Notification {
        room_name: item.room_computed_display_name.clone(),
        sender,
        body,
        direct: item.is_direct_message_room,
        thread_root: item.thread_id.as_ref().map(ToString::to_string),
        noisy: item.is_noisy.unwrap_or(true),
    }))
}

/// A message as one line of a notification.
fn describe(event: &AnySyncTimelineEvent) -> String {
    let AnySyncTimelineEvent::MessageLike(event) = event else {
        return String::new();
    };
    match event {
        AnySyncMessageLikeEvent::RoomMessage(SyncMessageLikeEvent::Original(message)) => {
            match &message.content.msgtype {
                MessageType::Text(text) => without_quote(&text.body),
                MessageType::Notice(text) => without_quote(&text.body),
                MessageType::Emote(text) => format!("* {}", text.body),
                MessageType::Image(_) => "sent a picture".to_owned(),
                MessageType::Video(_) => "sent a video".to_owned(),
                MessageType::Audio(_) => "sent a voice message".to_owned(),
                MessageType::File(file) => format!("sent a file: {}", file.body),
                MessageType::Location(_) => "shared a location".to_owned(),
                other => without_quote(other.body()),
            }
        }
        AnySyncMessageLikeEvent::Reaction(SyncMessageLikeEvent::Original(reaction)) => {
            format!("reacted {}", reaction.content.relates_to.key)
        }
        AnySyncMessageLikeEvent::UnstablePollStart(SyncMessageLikeEvent::Original(poll)) => {
            format!("Poll: {}", poll.content.poll_start().question.text)
        }
        AnySyncMessageLikeEvent::PollStart(SyncMessageLikeEvent::Original(poll)) => {
            format!(
                "Poll: {}",
                poll.content
                    .poll
                    .question
                    .text
                    .find_plain()
                    .unwrap_or_default()
            )
        }
        _ => String::new(),
    }
}

/// A reply's text without the quote older clients put in front of it.
fn without_quote(body: &str) -> String {
    match crate::convert::reply_fallback(body) {
        Some(reply) => reply.rest.to_owned(),
        None => body.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target {
            gateway: "https://push.example.org/_matrix/push/v1/notify".to_owned(),
            app_id: "de.tmrk.mothrix.dev".to_owned(),
            pushkey: "abc123".to_owned(),
            device_name: "iPhone".to_owned(),
            lang: "de".to_owned(),
            profile: "work".to_owned(),
        }
    }

    #[test]
    fn the_pusher_asks_for_ids_only_and_lets_the_phone_decrypt() {
        let json = serde_json::to_value(pusher(&target())).unwrap();
        assert_eq!(json["kind"], "http");
        assert_eq!(json["app_id"], "de.tmrk.mothrix.dev");
        assert_eq!(json["pushkey"], "abc123");
        assert_eq!(json["lang"], "de");
        let data = &json["data"];
        assert_eq!(
            data["url"],
            "https://push.example.org/_matrix/push/v1/notify"
        );
        assert_eq!(data["format"], "event_id_only");
        assert_eq!(data["default_payload"]["aps"]["mutable-content"], 1);
        assert_eq!(data["default_payload"]["mothrix_profile"], "work");
    }

    #[test]
    fn a_reply_is_shown_without_the_quote_in_front() {
        assert_eq!(
            without_quote("> <@amy:example.org> Lunch?\n\nYes, at noon"),
            "Yes, at noon"
        );
        assert_eq!(without_quote("> just a quote"), "> just a quote");
        assert_eq!(without_quote("plain"), "plain");
    }

    #[test]
    fn a_pusher_reads_back_as_registered() {
        let read = registered(&pusher(&target()));
        assert_eq!(read.app_id, "de.tmrk.mothrix.dev");
        assert_eq!(read.pushkey, "abc123");
        assert_eq!(read.app_name, "mothrix");
        assert_eq!(read.device_name, "iPhone");
        assert!(read.event_id_only);
        assert!(read.gateway.ends_with("/notify"));
    }
}
