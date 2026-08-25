//! Sending: one attempt at a time, the outbox clock that schedules them, and the reconciliation that decides what an uncertain outcome was.

use crate::commands::compose::guess_content_type;
use crate::config::{imap_config, smtp_config_for};
use crate::diag::log_sync;
use crate::state::{AppState, active_account, now_ms};
use crate::sync::drafts::drop_server_draft_using;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Wakes the drain worker when a queued message's time comes.
///
/// Nothing else in the system is clock-driven. The drain runs when a triage
/// action asks for it or when the sync loop comes round — and with IDLE the
/// sync loop sleeps until the server pushes something, so a message scheduled
/// for twenty seconds out waited for *unrelated mail to arrive*. Observed on
/// the live account: due at t+20s, still untouched at t+64s.
///
/// Sleeps to the exact instant rather than polling, so an empty outbox costs
/// nothing; re-checked after every drain, because a drain is what changes the
/// answer. The one-minute cap is for the clock being wrong — a laptop lid
/// closed through the scheduled time — not for accuracy: the send happens on
/// the rung, not a minute late.
pub(crate) fn spawn_outbox_clock(state: Arc<AppState>, account: i64) {
    tauri::async_runtime::spawn(async move {
        loop {
            let next = state
                .store
                .lock()
                .ok()
                .and_then(|s| s.next_due_ms(account).ok())
                .flatten();
            let wait_ms = match next {
                Some(at) => (at - now_ms()).clamp(0, 60_000),
                None => 60_000,
            };
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms as u64)).await;
            if next.is_some_and(|at| at <= now_ms()) {
                state.drain_signal.notify_one();
                // Give the drain its head before asking again, or this loop
                // sees the same due row and fires a second time for nothing.
                tokio::time::sleep(std::time::Duration::from_millis(2_000)).await;
            }
        }
    });
}

/// Builds and sends one message, then files a copy in Sent.
///
/// Shared by the composer and the scheduled-send worker so there is one
/// definition of what sending means — two would drift, and the half that
/// drifted would be the one nobody watches.
/// What one attempt to send came to.
///
/// Carries the transport's verdict rather than collapsing it to an error
/// string, because the four verdicts call for four different responses: done,
/// try again, stop and tell the user, or — the one that matters — *find out*.
struct Attempt {
    message_id: String,
    outcome: petrel_engine::outbox::AttemptOutcome,
    /// The server's words, for the row to show when it stopped.
    detail: String,
}

/// Sends once and reports what happened. Never retries, never files a copy in
/// Sent on an uncertain outcome — that is the caller's decision to make, with
/// the state machine's help.
#[allow(clippy::too_many_arguments)]
async fn attempt(
    state: &Arc<AppState>,
    to: Vec<String>,
    cc: Vec<String>,
    subject: String,
    body: String,
    html: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    attachments: Vec<String>,
) -> Result<Attempt, String> {
    use petrel_engine::outbox::AttemptOutcome;
    use petrel_providers::smtp::{Attachment, Outgoing, SendResult, SmtpConfig, send_tls};

    let account_id = {
        let store = state.store()?;
        store.active_account().ok().flatten()
    };
    let cfg = account_id
        .and_then(|a| imap_config(state, a))
        .ok_or("no account is configured")?;
    // The SMTP host the account was set up with. Derived from the IMAP host
    // only for the environment-driven account, which has no record of its own.
    let smtp = account_id
        .and_then(|a| {
            state
                .store
                .lock()
                .ok()
                .and_then(|st| smtp_config_for(&st, a))
        })
        .unwrap_or_else(|| SmtpConfig::for_imap_host(&cfg.host, &cfg.user, &cfg.pass));
    let domain = cfg
        .user
        .split('@')
        .nth(1)
        .unwrap_or("localhost")
        .to_string();

    let identity = {
        let store = state.store()?;
        store
            .active_account()
            .ok()
            .flatten()
            .and_then(|a| store.identity(a).ok())
    };
    // Read here rather than shuttled through the bridge. A 20MB file becomes a
    // 27MB JSON string on the way across, held twice in memory and slow enough
    // to notice; the path costs nothing and the file is read once.
    let mut files = Vec::new();
    for path in &attachments {
        let p = std::path::Path::new(path);
        let bytes = std::fs::read(p).map_err(|e| format!("Could not read {path}: {e}"))?;
        files.push(Attachment {
            filename: p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "attachment".into()),
            content_type: guess_content_type(p),
            bytes,
        });
    }

    let msg = Outgoing {
        from_addr: cfg.user.clone(),
        from_name: identity.map(|i| i.display_name).unwrap_or_default(),
        to,
        cc,
        subject,
        body_text: body,
        body_html: html.filter(|h| !h.trim().is_empty()),
        in_reply_to,
        references,
        attachments: files,
    };
    if msg.recipients().is_empty() {
        return Err("a message needs at least one recipient".into());
    }
    let (message_id, raw) = msg.render(&domain);

    let (outcome, detail) = match send_tls(&smtp, &msg, &raw).await {
        SendResult::Committed { response } => (AttemptOutcome::Accepted, response),
        SendResult::RejectedPermanently { response } => {
            log_sync(&format!("send rejected: {response}"));
            (AttemptOutcome::RejectedPermanently, response)
        }
        SendResult::FailedBeforeCommit { stage, detail } => {
            log_sync(&format!("send failed at {stage}: {detail}"));
            (
                AttemptOutcome::FailedBeforeCommit,
                format!("{stage}: {detail}"),
            )
        }
        SendResult::UnknownAfterTransmit { detail } => {
            // Spike S5's case: the body went, the acknowledgement did not. The
            // message may well have been delivered, so a retry could duplicate
            // it. Reported as exactly that, for the caller to resolve by
            // looking rather than by guessing.
            log_sync(&format!(
                "send outcome unknown: {detail} (message-id {message_id})"
            ));
            (AttemptOutcome::UnknownAfterTransmit, detail)
        }
    };
    if outcome != AttemptOutcome::Accepted {
        return Ok(Attempt {
            message_id,
            outcome,
            detail,
        });
    }

    // Filed second, and separately: a failure here has not lost the message.
    let sent_path = {
        let store = state.store()?;
        let account = active_account(&store)?;
        store
            .folder_for_role(account, "sent")
            .ok()
            .flatten()
            .and_then(|fid| store.folder_path(fid).ok().flatten())
    };
    if let Some(path) = sent_path
        && let Err(e) =
            petrel_providers::imap::append_message(&cfg, &path, Some("(\\Seen)"), &raw).await
    {
        log_sync(&format!("sent, but could not file a copy in {path}: {e}"));
    }
    log_sync(&format!("sent {message_id}"));
    Ok(Attempt {
        message_id,
        outcome,
        detail,
    })
}

/// Sends anything in the outbox whose time has come.
///
/// Runs on the same signal as the action drain, so a scheduled send goes out
/// promptly rather than on whatever the sync cadence happens to be. Failures
/// leave the message in the outbox: a send that did not happen is not one to
/// forget about, and the next pass will try again.
/// The retry ladder, in milliseconds: 30s, 2m, 8m, 30m, then hourly.
///
/// Exponential and capped, as the spec asks, and starting short because the
/// common failure is a connection that comes back in seconds. Past the cap
/// there is no giving up: a message waiting for a network that is genuinely
/// away for a day should still go when it returns, not be abandoned because
/// it waited too long.
fn retry_delay_ms(attempts: i64) -> i64 {
    const LADDER: [i64; 4] = [30_000, 120_000, 480_000, 1_800_000];
    LADDER
        .get(attempts.max(1) as usize - 1)
        .copied()
        .unwrap_or(3_600_000)
}

/// Looks in Sent for a message whose send outcome the transport could not
/// report.
///
/// This is the whole of the ambiguous-outcome rule's second half. SMTP has no
/// way to ask "did you get that?", so the answer has to come from evidence:
/// the copy a delivering server places in Sent. Found means it went. Searched
/// and absent means it did not, and a retry is safe. Unable to search — which
/// is the likely case straight after a dropped connection — means nobody
/// knows, and that is the one answer that must reach a person unchanged.
pub(crate) async fn sent_folder_evidence(
    state: &Arc<AppState>,
    cfg: &petrel_providers::imap::ImapConfig,
    account: i64,
    message_id: &str,
) -> petrel_engine::outbox::ServerEvidence {
    use petrel_engine::outbox::ServerEvidence;
    let sent = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.folder_for_role(account, "sent").ok().flatten())
        .and_then(|fid| {
            state
                .store
                .lock()
                .ok()
                .and_then(|s| s.folder_path(fid).ok().flatten())
        });
    let Some(path) = sent else {
        // No Sent folder known for this account: there is nowhere to look.
        return ServerEvidence::Indeterminate;
    };
    match petrel_providers::imap::find_message_id(cfg, &path, message_id).await {
        Ok(uids) if !uids.is_empty() => ServerEvidence::Found,
        Ok(_) => ServerEvidence::Absent,
        Err(e) => {
            log_sync(&format!("could not check Sent for {message_id}: {e}"));
            ServerEvidence::Indeterminate
        }
    }
}

/// Sends whatever is due, and records where each message ended up.
///
/// This is the outbox's state machine in motion. Each attempt's outcome goes
/// through `reconcile`, which is the only place that decides what a message's
/// state becomes — and for the uncertain case it decides by *looking*, in the
/// Sent folder, rather than by guessing. A message the engine cannot prove
/// either way is held for a person and is never picked up here again.
pub(crate) async fn send_due(state: Arc<AppState>, account: i64) {
    use petrel_engine::outbox::{AttemptOutcome, SendState, ServerEvidence, reconcile};

    let due = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        store.due_sends(account, now_ms()).unwrap_or_default()
    };
    if due.is_empty() {
        return;
    }
    log_sync(&format!("{} queued message(s) due", due.len()));
    let Some(cfg) = imap_config(&state, account) else {
        return;
    };

    for d in due {
        let id = d.id;
        if let Ok(store) = state.store.lock() {
            let _ = store.set_send_state(id, SendState::Transmitting, None, None, None);
        }
        let to: Vec<String> =
            d.to.split([',', ';'])
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
        let attempts = {
            state
                .store
                .lock()
                .ok()
                .and_then(|s| s.outbox(account).ok())
                .and_then(|rows| rows.into_iter().find(|r| r.id == id).map(|r| r.attempts))
                .unwrap_or(0)
        };

        let cc: Vec<String> =
            d.cc.split([',', ';'])
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
        // The whole message, not only its text: a reply that waited in the
        // undo window still threads into its conversation, and still carries
        // what was attached to it.
        let result = attempt(
            &state,
            to,
            cc,
            d.subject,
            d.body,
            // Stored alongside the text, so a message posted hours ago goes
            // out in the form it was written rather than flattened by the wait.
            Some(d.html).filter(|h| !h.trim().is_empty()),
            d.envelope.in_reply_to,
            d.envelope.references,
            d.envelope.attachments,
        )
        .await;

        let (outcome, detail, message_id) = match result {
            Ok(a) => (a.outcome, a.detail, Some(a.message_id)),
            // Could not even assemble the message — a missing attachment, no
            // account. Not a transport verdict, so it is treated as a failure
            // before anything was committed: safe to retry once it is fixed.
            Err(e) => (AttemptOutcome::FailedBeforeCommit, e, None),
        };

        // Evidence is consulted only when the transport could not say.
        let evidence = if outcome == AttemptOutcome::UnknownAfterTransmit {
            match &message_id {
                Some(m) => sent_folder_evidence(&state, &cfg, account, m).await,
                None => ServerEvidence::Indeterminate,
            }
        } else {
            ServerEvidence::Indeterminate
        };
        let next = reconcile(outcome, evidence);

        let Ok(store) = state.store.lock() else {
            continue;
        };
        match next {
            SendState::Sent => {
                log_sync(&format!(
                    "queued send delivered {}",
                    message_id.as_deref().unwrap_or("?")
                ));
                drop_server_draft_using(
                    &store,
                    id,
                    state.server_has_uidplus.load(Ordering::Relaxed),
                );
                let _ = store.delete_draft(id);
            }
            SendState::RetryQueued => {
                let n = attempts + 1;
                let wait = retry_delay_ms(n);
                log_sync(&format!(
                    "queued send failed, retrying in {}s: {detail}",
                    wait / 1000
                ));
                let _ = store.set_send_state(
                    id,
                    SendState::RetryQueued,
                    Some(&detail),
                    Some(now_ms() + wait),
                    message_id.as_deref(),
                );
            }
            SendState::FailedPermanent => {
                log_sync(&format!("queued send rejected: {detail}"));
                let _ = store.set_send_state(
                    id,
                    SendState::FailedPermanent,
                    Some(&detail),
                    None,
                    message_id.as_deref(),
                );
            }
            SendState::NeedsAttention => {
                // The one outcome no amount of engineering resolves. Said in
                // the row, and raised as a notification, because silence is
                // the one response that loses mail.
                log_sync(&format!("queued send needs attention: {detail}"));
                let _ = store.set_send_state(
                    id,
                    SendState::NeedsAttention,
                    Some(&detail),
                    None,
                    message_id.as_deref(),
                );
            }
            SendState::UndoWindow | SendState::Transmitting => {}
        }
    }
}
