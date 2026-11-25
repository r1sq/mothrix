// SPDX-License-Identifier: MPL-2.0

//! Verification by comparing emoji (SAS): of this device with another
//! device of the same account, or with another person. One flow runs at a time; it reports
//! its progress as [`VerifyState`]s and is steered by [`VerifyInput`]s.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use matrix_sdk::{
    Client,
    encryption::verification::{
        SasState, SasVerification, VerificationRequest, VerificationRequestState,
    },
    ruma::{
        TransactionId, UserId,
        api::client::to_device::send_event_to_device,
        events::{ToDeviceEventType, key::verification::VerificationMethod},
        serde::Raw,
        to_device::DeviceIdOrAllDevices,
    },
};
use tokio::sync::mpsc;

use crate::{matrix::Update, model::VerifyState};

/// What the user chose in the verification dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyInput {
    /// Take part in a verification another device asked for.
    Accept,
    /// The emoji are the same on both devices.
    Confirm,
    /// Stop, or the emoji differ.
    Cancel,
}

/// Ask our other devices to verify this one.
pub async fn request(client: &Client) -> Result<VerificationRequest> {
    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;
    if !client.encryption().has_devices_to_verify_against().await? {
        bail!("no other verified device is signed in; use /recover <recovery key> instead");
    }
    let user_id = client.user_id().context("not signed in")?;
    let identity = client
        .encryption()
        .get_user_identity(user_id)
        .await?
        .context("this account has no encryption identity yet")?;
    identity
        .request_verification_with_methods(vec![VerificationMethod::SasV1])
        .await
        .context("could not ask the other devices")
}

/// Ask another person to verify with us (they see the request in the
/// direct chat with us, in a client that can verify).
pub async fn request_user(client: &Client, user_id: &UserId) -> Result<VerificationRequest> {
    if Some(user_id) == client.user_id() {
        return request(client).await;
    }
    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;
    let identity = match client.encryption().get_user_identity(user_id).await? {
        Some(identity) => identity,
        // Not seen yet: ask the server.
        None => client
            .encryption()
            .request_user_identity(user_id)
            .await?
            .with_context(|| format!("{user_id} has not set up encryption"))?,
    };
    identity
        .request_verification_with_methods(vec![VerificationMethod::SasV1])
        .await
        .with_context(|| format!("could not ask {user_id}"))
}

type Inputs = mpsc::UnboundedReceiver<VerifyInput>;

/// Run one verification from request to result.
/// `unanswered` is true for as long as an incoming request waits for the
/// user to accept it; such a request can be put aside for a newer one
/// (by dropping the sending end of `inputs`), and then ends without a word.
pub async fn run(
    client: Client,
    request: VerificationRequest,
    incoming: bool,
    tx: mpsc::UnboundedSender<Update>,
    mut inputs: Inputs,
    unanswered: Arc<AtomicBool>,
) {
    let outcome = match drive(&client, &request, incoming, &tx, &mut inputs, &unanswered).await {
        Ok(Some(state)) => state,
        Ok(None) => return,
        Err(error) => VerifyState::Failed(format!("{error:#}")),
    };
    unanswered.store(false, Ordering::Relaxed);
    let _ = tx.send(Update::Verify(Some(outcome)));
}

fn show(tx: &mpsc::UnboundedSender<Update>, state: VerifyState) {
    // Not at a level the log keeps by default: the emoji are in it.
    tracing::debug!(?state, "verification progress");
    let _ = tx.send(Update::Verify(Some(state)));
}

/// With another person, the steps travel as messages in the direct chat,
/// and our answers to them are only sent out when the encryption sync
/// comes round, which can take half a minute. A message to ourselves
/// that says nothing makes it come round now.
async fn nudge(client: &Client, request: &VerificationRequest) {
    if request.is_self_verification() {
        return;
    }
    let (Some(user), Some(device)) = (client.user_id(), client.device_id()) else {
        return;
    };
    let Ok(content) = Raw::new(&serde_json::json!({})) else {
        return;
    };
    let messages = BTreeMap::from([(
        user.to_owned(),
        BTreeMap::from([(
            DeviceIdOrAllDevices::DeviceId(device.to_owned()),
            content.cast_unchecked(),
        )]),
    )]);
    let request = send_event_to_device::v3::Request::new_raw(
        ToDeviceEventType::Dummy,
        TransactionId::new(),
        messages,
    );
    let _ = client.send(request).await;
}

async fn drive(
    client: &Client,
    request: &VerificationRequest,
    incoming: bool,
    tx: &mpsc::UnboundedSender<Update>,
    inputs: &mut Inputs,
    unanswered: &AtomicBool,
) -> Result<Option<VerifyState>> {
    let mut changes = Box::pin(request.changes());

    if incoming {
        unanswered.store(true, Ordering::Relaxed);
        show(
            tx,
            VerifyState::Incoming {
                device: if request.is_self_verification() {
                    "Another of your devices".to_owned()
                } else {
                    request.other_user_id().to_string()
                },
            },
        );
        loop {
            tokio::select! {
                input = inputs.recv() => match input {
                    Some(VerifyInput::Accept) => break,
                    Some(VerifyInput::Confirm) => {}
                    Some(VerifyInput::Cancel) => {
                        let _ = request.cancel().await;
                        return Ok(Some(VerifyState::Failed("Cancelled".to_owned())));
                    }
                    // Put aside for a newer request: nothing to say.
                    None => return Ok(None),
                },
                state = changes.next() => match state {
                    Some(VerificationRequestState::Cancelled(info)) => {
                        return Ok(Some(VerifyState::Failed(info.reason().to_owned())));
                    }
                    Some(_) => {}
                    None => bail!("the request went away"),
                },
            }
        }
        unanswered.store(false, Ordering::Relaxed);
        request
            .accept_with_methods(vec![VerificationMethod::SasV1])
            .await
            .context("could not accept the request")?;
    }

    show(tx, VerifyState::Waiting);

    // Once both sides are ready, exactly one of them starts the emoji
    // comparison: the device that asked for the verification. If both
    // started at once, each would end up waiting on its own superseded
    // attempt. The other side joins when it sees the comparison begin.
    let mut state = request.state();
    let sas = loop {
        match state {
            VerificationRequestState::Ready { .. } if request.we_started() => {
                if let Some(sas) = request
                    .start_sas()
                    .await
                    .context("could not start the emoji comparison")?
                {
                    break sas;
                }
            }
            VerificationRequestState::Transitioned { verification, .. } => {
                break verification
                    .sas()
                    .context("the other device chose a method mothrix does not support")?;
            }
            VerificationRequestState::Done => return Ok(Some(VerifyState::Done)),
            VerificationRequestState::Cancelled(info) => {
                return Ok(Some(VerifyState::Failed(info.reason().to_owned())));
            }
            _ => {}
        }
        tokio::select! {
            next = changes.next() => {
                state = next.context("the request went away")?;
                tracing::info!(?state, "verification request changed");
                nudge(client, request).await;
            }
            input = inputs.recv() => if matches!(input, Some(VerifyInput::Cancel) | None) {
                let _ = request.cancel().await;
                return Ok(Some(VerifyState::Failed("Cancelled".to_owned())));
            },
        }
    };

    compare(client, request, sas, tx, inputs).await
}

async fn compare(
    client: &Client,
    request: &VerificationRequest,
    sas: SasVerification,
    tx: &mpsc::UnboundedSender<Update>,
    inputs: &mut Inputs,
) -> Result<Option<VerifyState>> {
    let mut changes = Box::pin(sas.changes());
    let mut state = sas.state();
    let mut accepted = false;
    let mut shown = false;

    loop {
        match &state {
            SasState::Started { .. } if !sas.we_started() && !accepted => {
                accepted = true;
                sas.accept()
                    .await
                    .context("could not join the emoji comparison")?;
            }
            SasState::KeysExchanged { .. } if !shown => {
                let emojis = sas
                    .emoji()
                    .context("the other device does not support emoji")?;
                shown = true;
                show(
                    tx,
                    VerifyState::Emojis(
                        emojis
                            .iter()
                            .map(|emoji| (emoji.symbol.to_owned(), emoji.description.to_owned()))
                            .collect(),
                    ),
                );
            }
            SasState::Confirmed => show(tx, VerifyState::Confirming),
            SasState::Done { .. } => return Ok(Some(VerifyState::Done)),
            SasState::Cancelled(info) => {
                return Ok(Some(VerifyState::Failed(info.reason().to_owned())));
            }
            _ => {}
        }

        tokio::select! {
            next = changes.next() => {
                state = next.context("the comparison went away")?;
                // Its name only: the state itself holds the emoji.
                tracing::info!(state = step(&state), "emoji comparison changed");
                nudge(client, request).await;
            }
            input = inputs.recv() => match input {
                Some(VerifyInput::Confirm) if shown => {
                    sas.confirm().await.context("could not confirm")?;
                    show(tx, VerifyState::Confirming);
                }
                Some(VerifyInput::Cancel) | None => {
                    // Saying "no" while the emoji are on screen means they differ.
                    let _ = if shown { sas.mismatch().await } else { sas.cancel().await };
                    return Ok(Some(VerifyState::Failed(
                        if shown { "The emoji did not match" } else { "Cancelled" }.to_owned(),
                    )));
                }
                Some(_) => {}
            },
        }
    }
}

/// Where an emoji comparison stands, for the log: the state as it is
/// would write the emoji (and the numbers) there, which are for the two
/// people comparing and no one else.
fn step(state: &SasState) -> &'static str {
    match state {
        SasState::Created { .. } => "created",
        SasState::Started { .. } => "started",
        SasState::Accepted { .. } => "accepted",
        SasState::KeysExchanged { .. } => "keys exchanged",
        SasState::Confirmed => "confirmed",
        SasState::Done { .. } => "done",
        SasState::Cancelled(_) => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_names_the_step_of_a_comparison_not_what_is_compared() {
        let state = SasState::KeysExchanged {
            emojis: None,
            decimals: (1234, 5678, 9012),
        };
        assert_eq!(step(&state), "keys exchanged");
        // What the log line had before: the numbers (and the emoji).
        assert!(format!("{state:?}").contains("5678"));
    }
}
