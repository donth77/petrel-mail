//! Sending: one attempt at a time, the outbox clock that schedules them, and the reconciliation that decides what an uncertain outcome was.

use crate::commands::compose::guess_content_type;
use crate::config::{imap_config_from_env, imap_config_from_servers};
use crate::diag::{log_sync, without_addresses};
use crate::state::{AppState, now_ms};
use crate::sync::drafts::drop_server_draft_using;
use petrel_engine::store::Store;
use std::sync::Arc;
use std::sync::{MutexGuard, TryLockError};

/// The store lock, without pinning a tokio worker on `Mutex::lock`.
///
/// Ingest and backfill hold this lock across FTS and fsync. A send that
/// blocked on `lock()` sat in `Transmitting` with no SMTP socket and no
/// timeout, because the worker was stuck in the kernel and could not poll
/// the SMTP deadlines. Yielding between tries lets those deadlines run, and
/// lets the inbox keep answering.
async fn wait_store(state: &AppState) -> Option<MutexGuard<'_, Store>> {
    for i in 0..500 {
        match state.store.try_lock() {
            Ok(g) => return Some(g),
            Err(TryLockError::Poisoned(p)) => return Some(p.into_inner()),
            Err(TryLockError::WouldBlock) => {}
        }
        if i == 0 {
            log_sync("outbox: store is busy, waiting");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    log_sync("outbox: store stayed busy");
    None
}

/// Sends whatever is due, without waiting for IMAP drain.
///
/// Send now used to raise the drain signal. The drain worker always finished
/// `drain_actions` first, so a mailbox with a backlog of STORE/MOVE held SMTP
/// for as long as that backlog took — two minutes, live, for fifteen actions.
/// This worker is the other half of that split: triage still has its signal;
/// a send wakes this one.
pub(crate) fn spawn_send_worker(state: Arc<AppState>, account: i64) {
    let signals = state.outbox_signals(account);
    let mut stop = state.stop_signal(account);
    tauri::async_runtime::spawn(async move {
        loop {
            if *stop.borrow() {
                break;
            }
            tokio::select! {
                _ = signals.send.notified() => {}
                _ = stop.changed() => break,
            }
            // Two rows marked due a few milliseconds apart should be one pass.
            // Not the drain's 900ms: that is for triage bursts, and it is the
            // wait this worker exists to avoid.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            send_due(Arc::clone(&state), account).await;
        }
        log_sync(&format!("account {account}: send worker stopped"));
    });
}

/// Wakes the send worker when a queued message's time comes.
///
/// Nothing else in the system is clock-driven. The send used to ride the
/// drain, and with IDLE the sync loop sleeps until the server pushes
/// something, so a message scheduled for twenty seconds out waited for
/// *unrelated mail to arrive*. Observed on the live account: due at t+20s,
/// still untouched at t+64s.
///
/// Sleeps to the exact instant rather than polling, so an empty outbox costs
/// nothing. A new schedule aborts that sleep via the account's clock signal —
/// without it, a send queued during the empty-outbox nap waited until the nap
/// ended, or until someone pressed Send now. The one-minute cap is for the
/// clock being wrong — a laptop lid closed through the scheduled time.
pub(crate) fn spawn_outbox_clock(state: Arc<AppState>, account: i64) {
    let signals = state.outbox_signals(account);
    let mut stop = state.stop_signal(account);
    tauri::async_runtime::spawn(async move {
        loop {
            if *stop.borrow() {
                break;
            }
            let next = wait_store(&state)
                .await
                .and_then(|s| s.next_due_ms(account).ok())
                .flatten();
            let wait_ms = match next {
                Some(at) => (at - now_ms()).clamp(0, 60_000),
                None => 60_000,
            };
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(wait_ms as u64)) => {}
                _ = signals.clock.notified() => continue,
                _ = stop.changed() => break,
            }
            if next.is_some_and(|at| at <= now_ms()) {
                signals.send.notify_one();
                // Give the send worker its head before asking again, or this
                // loop sees the same due row and fires a second time for nothing.
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
///
/// `account` is the account the message was written from, and the only one it
/// may be sent from. It used to be read from `active_account()` instead —
/// whichever account happened to be selected in the window when the undo
/// window ran out. The outbox is per account and `send_due` already knew
/// which one; this threw that away and asked the UI. With two accounts set
/// up, a message written from one went out over the other's SMTP server, as
/// the other's address, carrying the other's signature, and was filed in the
/// other's Sent folder — with every layer reporting success.
/// The password inside a config, for the SMTP fallback that derives its
/// settings from the IMAP host.
///
/// Empty for a token account, which is correct rather than convenient: that
/// fallback exists for the environment-driven developer account, and an
/// account signing in with OAuth has real SMTP settings of its own.
fn cfg_password(cfg: &petrel_providers::imap::ImapConfig) -> &str {
    match &cfg.credential {
        petrel_providers::imap::Credential::Password(p) => p,
        petrel_providers::imap::Credential::Bearer(_) => "",
    }
}

#[allow(clippy::too_many_arguments)]
async fn attempt(
    state: &Arc<AppState>,
    account: i64,
    cfg: &petrel_providers::imap::ImapConfig,
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
    use petrel_providers::smtp::{Attachment, Outgoing, SendResult, SmtpConfig, send_tls_with};

    log_sync("outbox: assembling");
    // Servers under the lock, password after it drops: a keychain consent
    // dialog must not pin every other command behind the store.
    let servers = wait_store(state)
        .await
        .and_then(|st| st.account_servers(account).ok().flatten());
    let smtp = servers
        .and_then(|s| crate::config::smtp_config_from_servers(account, s))
        .unwrap_or_else(|| SmtpConfig::for_imap_host(&cfg.host, &cfg.user, cfg_password(cfg)));
    let domain = cfg
        .user
        .split('@')
        .nth(1)
        .unwrap_or("localhost")
        .to_string();

    let identity = {
        let store = wait_store(state)
            .await
            .ok_or("could not read the account while the mailbox is busy")?;
        store.identity(account).ok()
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
    let (message_id, raw) =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| msg.render(&domain)))
            .map_err(|_| "could not assemble the message".to_string())?;

    log_sync(&format!("outbox: connecting smtp :{}", smtp.port));
    let (outcome, detail) = match send_tls_with(&smtp, &msg, &raw, |s| {
        log_sync(&format!("outbox: smtp {}", without_addresses(s)));
    })
    .await
    {
        SendResult::Committed { response } => (AttemptOutcome::Accepted, response),
        SendResult::RejectedPermanently { response } => {
            log_sync(&format!("send rejected: {}", without_addresses(&response)));
            (AttemptOutcome::RejectedPermanently, response)
        }
        SendResult::FailedBeforeCommit { stage, detail } => {
            log_sync(&format!(
                "send failed at {stage}: {}",
                without_addresses(&detail)
            ));
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
        let store = wait_store(state).await;
        store.and_then(|store| {
            store
                .folder_for_role(account, "sent")
                .ok()
                .flatten()
                .and_then(|fid| store.folder_path(fid).ok().flatten())
        })
    };
    if let Some(path) = sent_path {
        match tokio::time::timeout(
            std::time::Duration::from_secs(60),
            petrel_providers::imap::append_message(cfg, &path, Some("(\\Seen)"), &raw),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                log_sync(&format!("sent, but could not file a copy in {path}: {e}"));
            }
            Err(_) => {
                log_sync(&format!("sent, but filing a copy in {path} timed out"));
            }
        }
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
    let sent = wait_store(state).await.and_then(|s| {
        s.folder_for_role(account, "sent")
            .ok()
            .flatten()
            .and_then(|fid| s.folder_path(fid).ok().flatten())
    });
    let Some(path) = sent else {
        // No Sent folder known for this account: there is nowhere to look.
        return ServerEvidence::Indeterminate;
    };
    match petrel_providers::imap::find_message_id(cfg, &path, message_id).await {
        Ok(uids) => evidence_from_search(!uids.is_empty(), server_files_sent_copies(cfg)),
        Err(e) => {
            log_sync(&format!("could not check Sent for {message_id}: {e}"));
            ServerEvidence::Indeterminate
        }
    }
}

/// Whether this server places a copy of every submitted message in Sent
/// itself. Gmail does. Almost nobody else does: on a plain IMAP/SMTP host the
/// only copy in Sent is the one Petrel appends after the send is confirmed —
/// which, after an ambiguous send, is exactly the copy that was never made.
fn server_files_sent_copies(cfg: &petrel_providers::imap::ImapConfig) -> bool {
    crate::sync::account_is_gmail(cfg)
}

/// What a Sent search proves. Found is found. Absent means "did not go" only
/// where the server would have filed a copy on its own; anywhere else an
/// empty Sent folder says nothing, and the answer has to reach a person
/// rather than start a retry that may deliver the message twice.
fn evidence_from_search(
    found: bool,
    server_files_sent: bool,
) -> petrel_engine::outbox::ServerEvidence {
    use petrel_engine::outbox::ServerEvidence;
    if found {
        ServerEvidence::Found
    } else if server_files_sent {
        ServerEvidence::Absent
    } else {
        ServerEvidence::Indeterminate
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

    // One pass at a time per account, by construction: the account's send
    // worker is the only caller, and it awaits each pass before taking the
    // next wake-up. Two accounts may transmit at once; they hold different
    // rows.
    let due = {
        let Some(store) = wait_store(&state).await else {
            return;
        };
        store.due_sends(account, now_ms()).unwrap_or_default()
    };
    if due.is_empty() {
        return;
    }
    log_sync(&format!("{} queued message(s) due", due.len()));
    let cfg = {
        let servers = wait_store(&state)
            .await
            .and_then(|s| s.account_servers(account).ok().flatten());
        servers
            .and_then(|s| imap_config_from_servers(account, s))
            .or_else(imap_config_from_env)
    };
    let Some(cfg) = cfg else {
        log_sync("outbox: no account configured, leaving the queue as-is");
        return;
    };

    for d in due {
        let id = d.id;
        // Claim first. If the store stays busy we leave the row queued rather
        // than transmit without a durable Transmitting mark, or mark it and
        // then fail to start SMTP. The guard must die before the next await:
        // MutexGuard is not Send.
        {
            let Some(store) = wait_store(&state).await else {
                log_sync("outbox: store stayed busy, leaving the row queued");
                continue;
            };
            let _ = store.set_send_state(id, SendState::Transmitting, None, None, None);
        }
        let to: Vec<String> =
            d.to.split([',', ';'])
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
        let attempts = {
            wait_store(&state)
                .await
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
            account,
            &cfg,
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
        log_sync(&format!("outbox: outcome {next:?}"));

        // Must not leave Transmitting with no buttons. Keep asking until the
        // store is free, then write the real outcome rather than guessing.
        let store = loop {
            if let Some(store) = wait_store(&state).await {
                break store;
            }
            log_sync("outbox: could not record the send outcome, retrying");
        };
        match next {
            SendState::Sent => {
                log_sync(&format!(
                    "queued send delivered {}",
                    message_id.as_deref().unwrap_or("?")
                ));
                drop_server_draft_using(&state, &store, id);
                let _ = store.delete_draft(id);
            }
            SendState::RetryQueued => {
                let n = attempts + 1;
                let wait = retry_delay_ms(n);
                log_sync(&format!(
                    "queued send failed, retrying in {}s: {}",
                    wait / 1000,
                    without_addresses(&detail)
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
                log_sync(&format!(
                    "queued send rejected: {}",
                    without_addresses(&detail)
                ));
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
                log_sync(&format!(
                    "queued send needs attention: {}",
                    without_addresses(&detail)
                ));
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

#[cfg(test)]
mod evidence_tests {
    use super::evidence_from_search;
    use petrel_engine::outbox::ServerEvidence;

    /// The rule the outbox lives by: absence is proof only where the server
    /// files its own copies. Everywhere else an empty Sent after a dropped
    /// connection means nobody knows, and "nobody knows" reaches a person.
    #[test]
    fn an_empty_sent_folder_is_evidence_only_where_the_server_files_copies() {
        assert_eq!(evidence_from_search(true, false), ServerEvidence::Found);
        assert_eq!(evidence_from_search(true, true), ServerEvidence::Found);
        assert_eq!(evidence_from_search(false, true), ServerEvidence::Absent);
        assert_eq!(
            evidence_from_search(false, false),
            ServerEvidence::Indeterminate
        );
    }
}
