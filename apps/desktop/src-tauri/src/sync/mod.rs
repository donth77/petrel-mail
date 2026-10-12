//! Keeping the store and the server in step: the sync cycle, the folders it covers, and the workers that run it.

pub(crate) mod backfill;
#[cfg(test)]
mod caps_tests;
pub(crate) mod drafts;
pub(crate) mod drain;
pub(crate) mod reindex;
#[cfg(all(test, feature = "dev-plaintext-imap"))]
mod renumber_tests;
#[cfg(all(test, feature = "dev-plaintext-imap"))]
mod stateful_imap;

use crate::diag::{friendly_sync_error_for, is_imap_parse_error, is_sign_in_refusal, log_sync};
use crate::send::{spawn_outbox_clock, spawn_send_worker};
use crate::signin::{SignIn, say_sync_error, wait_for_signin};
use crate::state::{AppState, now_ms, stopped, unless_stopped};
use crate::sync::backfill::spawn_backfill;
use crate::sync::drain::{drain_actions, spawn_drain_worker};
use crate::sync::reindex::{
    run_startup as reindex_startup, spawn_remainder as spawn_reindex_remainder,
};
use petrel_engine::actions::ActionKind;
use petrel_engine::store::Store;
use petrel_providers::imap::ImapConfig;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) fn spawn_real_sync(state: Arc<AppState>, account: i64, cfg: ImapConfig) {
    // One switch for everything this account runs, taken here and handed
    // down. The watchers and the backfill used to ask for their own at the
    // end of the first pass — which, after a removal during that pass, was a
    // fresh switch nobody had flipped — and the account carried on syncing
    // into a store that no longer had it, or into the next account to reuse
    // its id.
    let mut stop = state.stop_signal(account);
    // This account's first pass, marked before anything is spawned so the
    // window's next status poll already sees it. Every first pass is one:
    // at launch, for an account just added, and after a new password.
    let seeding = state.mark_seeding(account);
    spawn_drain_worker(Arc::clone(&state), account, cfg.clone(), stop.clone());
    spawn_outbox_clock(Arc::clone(&state), account, stop.clone());
    spawn_send_worker(Arc::clone(&state), account, stop.clone());
    tauri::async_runtime::spawn(async move {
        *state.source.lock().unwrap_or_else(|p| p.into_inner()) = format!("syncing {}…", cfg.host);
        // Between the steps of the first pass, and around the long ones: a
        // removed account stands down here rather than at the end.
        let stand_down = || {
            log_sync(&format!("account {account}: sync stopped"));
            // The first pass never reaches its end, so what it clears there
            // is cleared here: otherwise "fetching your mail" stays on
            // screen for the life of the process over an account that no
            // longer exists.
            state.end_seeding(account, seeding);
            *state.source.lock().unwrap_or_else(|p| p.into_inner()) = "sync stopped".into();
        };

        // Mail already held was indexed by whatever the extraction did then.
        // A few newest slices repair what is on screen; the rest must not
        // sit in front of the first fetch — on a large store that delay is
        // how receiving looks like it has stopped.
        let reindex_done = reindex_startup(&state, &mut stop).await;
        if *stop.borrow() {
            stand_down();
            return;
        }

        let mut has_move = false;
        let mut has_idle = false;
        let mut has_uidplus = false;
        // Whether this account's folders are labels, which decides whether the
        // label sweep below has anything to ask for.
        let mut looks_like_gmail = false;
        // Folders first. Without them every message ingests with no placement,
        // so the rail's views have nothing to filter on and archiving has
        // nowhere to put anything — which is how a sync can look like it worked
        // while leaving the app unable to file a single message.
        let Some(probed) = unless_stopped(&mut stop, petrel_providers::imap::probe(&cfg, 0)).await
        else {
            stand_down();
            return;
        };
        match probed {
            Ok(report) => {
                // Signed in: whatever a late writer left behind — a stopped
                // run's refusal landing after this one began — does not
                // stand over a password the server has just taken.
                state.cleared_by(account, &stop);
                let caps = learned_caps(&report, &cfg);
                has_move = caps.has_move;
                has_idle = caps.has_idle;
                has_uidplus = caps.has_uidplus;
                looks_like_gmail = caps.is_gmail;
                log_sync(&format!(
                    "probe ok: {} folder(s), MOVE={has_move}, IDLE={has_idle}, UIDPLUS={has_uidplus}",
                    report.folders.len(),
                ));
                let rows = survey_rows(&report.folders, looks_like_gmail);
                let all_mail = all_mail_paths(&report.folders, looks_like_gmail);
                state.set_caps(account, caps);
                let mut stored = false;
                if let Ok(mut store) = state.store.lock() {
                    let tag_names: Vec<String> = store
                        .tags_for_account(account)
                        .map(|ts| ts.into_iter().map(|t| t.name).collect())
                        .unwrap_or_default();
                    let rows = without_tag_labels(rows, &tag_names, looks_like_gmail);
                    match store.sync_folders(account, &rows) {
                        Ok(n) => log_sync(&format!("{n} folder(s) stored")),
                        Err(e) => log_sync(&format!("folder sync failed: {e}")),
                    }
                    let _ =
                        store.set_folder_delimiters(account, &survey_delimiters(&report.folders));
                    stored = store.set_all_mail_folders(account, &all_mail).is_ok();
                    if looks_like_gmail {
                        let _ = store.set_account_kind(account, "gmail");
                    }
                }
                if stored {
                    state.mark_surveyed(account);
                }
                create_waiting_folders(&state, account, &cfg).await;
            }
            Err(e) => {
                let raw = format!("{e}");
                // By kind and code, as every other line: a NO here quotes
                // folder names, and a parse error the whole unparsed reply.
                log_sync(&format!("folder discovery FAILED: {}", loggable(&e)));
                // Refused at the door: the account stands down here, before
                // the drain, the sync or the bin's expiry ask again. They
                // used to, a sign-in per queued archive, and twenty-two
                // refused sign-ins at every launch is what gets a home
                // network banned. The launch's one sign-in is this one.
                if e.is_sign_in_refused() {
                    log_sync(&format!(
                        "account {account}: sign-in refused; standing down"
                    ));
                    state.refused_by(account, &stop);
                }
                say_sync_error(
                    &state,
                    &stop,
                    Some(friendly_sync_error_for(&cfg.host, &raw)),
                );
            }
        }

        // Due mail goes out on the send worker while this drain runs. Waiting
        // until after drain_actions was the two-minute stall: Send now sat
        // behind fifteen IMAP actions.
        state.nudge_send(account);

        // Everything from here to the end of the first pass asks the server,
        // and a refused password has nothing to ask it with.
        if state.signin(account).is_none() {
            // Deliver before reading back. Draining first means the server's answer
            // already includes what the user did, so the fetch below confirms local
            // state instead of contradicting it — and anything still queued is
            // protected from being overwritten by the pending checks in the store.
            // If another drain holds the floor the fetch proceeds without it —
            // the store's pending checks protect what is queued, and the drain
            // worker retries until the floor frees.
            let mine = stop.clone();
            let drained = unless_stopped(
                &mut stop,
                drain_actions(
                    Arc::clone(&state),
                    account,
                    cfg.clone(),
                    has_move,
                    has_uidplus,
                    account_is_gmail(&cfg),
                    &mine,
                ),
            )
            .await;
            if drained.is_none() {
                stand_down();
                return;
            }
            // A message due while the app was closed goes out now, rather than
            // waiting for whatever next wakes the worker. Notify, do not await:
            // send_due used to sit behind this drain, and a backlog of triage
            // made "Send now" look like the outbox had ignored the click.
            state.nudge_send(account);

            // One connection, one STATUS line per folder, fetch only what moved.
            // A relaunch over a warm store downloads nothing it already holds.
            let Some(report) = unless_stopped(
                &mut stop,
                run_sync_cycle(&state, account, &cfg, true, Scope::Everything),
            )
            .await
            else {
                stand_down();
                return;
            };
            let (fresh, failures) = (report.fresh, report.failures);
            let targets = folders_to_sync(&state, account);
            if failures > 0 {
                log_sync(&format!("{failures} folder(s) could not be synced"));
            }
            if !targets.is_empty() && failures >= targets.len() {
                let msg = crate::diag::NO_FOLDER_SYNCED;
                log_sync(msg);
                // The server's own reason, where there is one. "no folder could be
                // synced" told somebody whose password had been revoked nothing,
                // and replaced the sign-in advice the probe had just put up.
                let raw = report.last_failure.as_deref().unwrap_or(msg);
                if is_sign_in_refusal(raw) {
                    state.refused_by(account, &stop);
                }
                say_sync_error(&state, &stop, Some(friendly_sync_error_for(&cfg.host, raw)));
                *state.source.lock().unwrap_or_else(|p| p.into_inner()) = "sync failed".into();
            } else {
                let held = state.seeded.load(Ordering::Relaxed);
                log_sync(&format!(
                    "first pass done: {fresh} new, {held} held locally"
                ));
                *state.source.lock().unwrap_or_else(|p| p.into_inner()) =
                    format!("{} · {held} message(s) held", cfg.user);
            }
            // Where Gmail actually keeps each message.
            //
            // After the bodies rather than before: this decides filing, and filing
            // an empty mailbox helps nobody. Over plain IMAP a message is only ever
            // in the mailbox it was fetched from, so archived — not carrying the
            // Inbox label — is not something the protocol can express.
            //
            // Bounded on the first pass and incremental after it. A full sweep is
            // seconds at a thousand messages and minutes at a hundred thousand,
            // but with CONDSTORE every sweep after the first asks only for what
            // changed, which is usually nothing and costs one round trip.
            if looks_like_gmail && state.signin(account).is_none() {
                let swept = unless_stopped(&mut stop, async {
                    run_label_sweep(&state, account, &cfg).await;
                    run_thrid_sweep(&state, account, &cfg).await;
                })
                .await;
                if swept.is_none() {
                    stand_down();
                    return;
                }
            }
        } else {
            *state.source.lock().unwrap_or_else(|p| p.into_inner()) = "sign-in refused".into();
        }

        state.end_seeding(account, seeding);

        // The first pass may have re-listed messages whose move the drain has
        // since delivered; sweep once now rather than waiting out the first
        // IDLE, so a conversation never spends the first half hour standing
        // in both its folder and the inbox.
        // The bin's clock starts at launch too, not only on the next poll:
        // with IDLE holding a quiet account open, "the next cycle" can be
        // hours away, and mail would sit in the bin unstamped until then.
        let settled = unless_stopped(&mut stop, async {
            // A refused password has nothing to reconcile with, and every
            // ask is one more refused sign-in; nor does the bin's expiry,
            // which expunges message by message.
            if state.signin(account).is_none() {
                reconcile_ghost_placements(&state, account, &cfg).await;
                tend_the_bin(&state, account).await;
            }
        })
        .await;
        if settled.is_none() {
            stand_down();
            return;
        }
        // What waited for this account to sign in goes now. Not raced
        // against the switch: it takes the work it does, and a stop part-way
        // must hand back what it had not done, not drop it.
        if state.signin(account).is_none() {
            drafts::push_held(&state, account, &stop).await;
        }

        // History fills in behind the present, on its own clock — see
        // spawn_backfill for why it is not part of the poll loop.
        spawn_backfill(Arc::clone(&state), account, cfg.clone(), stop.clone());
        if !reindex_done {
            spawn_reindex_remainder(Arc::clone(&state), stop.clone());
        }

        // From here on the account is watched rather than polled, on two
        // clocks that answer two different questions.
        //
        // A wake says "the inbox changed" and nothing more, so it is answered
        // with the inbox and nothing more: one folder, about a second, and the
        // message is on screen. The sweep is the slower question — what did
        // another client do to the other hundred folders — and it is the
        // expensive one: 101 STATUS round trips twice over, measured at 31
        // seconds on a real account.
        //
        // Running them on one clock is what made Petrel feel slow. Every wake
        // paid the sweep's half minute before anything appeared, and IDLE was
        // torn down for all of it, so mail that arrived during a sweep was
        // announced to nobody and waited for whatever woke the next one.
        let every = std::time::Duration::from_secs(
            std::env::var("PETREL_POLL_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|s| *s >= 15)
                .unwrap_or(120),
        );
        // How often the other folders are swept. Five minutes rather than the
        // poll interval because a sweep is not free and nothing is waiting on
        // it: mail arrives through the wake path now, and this only catches up
        // with what was done elsewhere. It is also *more* often than the old
        // loop managed on a quiet account, where the sweep rode the 20-minute
        // IDLE ceiling.
        let sweep_every = std::time::Duration::from_secs(
            std::env::var("PETREL_SWEEP_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|s| *s >= 30)
                .unwrap_or(300),
        );
        // The gap-filling reconcile is the most expensive step of the lot and
        // the least urgent — it exists to catch mail a closed watermark
        // skipped, which is a rare accident rather than a daily event. Kept on
        // the old cadence so this change cannot make the account busier than
        // it already was.
        let reconcile_every = std::time::Duration::from_secs(
            std::env::var("PETREL_RECONCILE_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|s| *s >= 60)
                .unwrap_or(20 * 60),
        );
        // RFC 2177 puts the ceiling at 29 minutes; 20 leaves room for a server
        // that is stricter than the standard without making reconnects frequent.
        let idle_ceiling = std::time::Duration::from_secs(20 * 60);
        log_sync(&format!(
            "watching for new mail via {}",
            if has_idle { "IDLE" } else { "poll" }
        ));

        // The watcher owns the IDLE connection and nothing else, so the sweep
        // can take as long as it likes without the account going deaf. A
        // capacity of one is the coalescing: wakes that land while a pass is
        // running collapse into the single pass that follows it, which is the
        // right answer because a wake carries no detail to lose.
        let (wake_tx, mut wake_rx) = tokio::sync::mpsc::channel::<()>(1);
        // A launch that never reached the server does not know whether it can
        // IDLE. The watchers start anyway and wait to hear (`idle_known`):
        // left out, the account sat on the two-minute poll, each poll a full
        // sweep, until the app was next relaunched.
        if has_idle || !state.caps(account).known {
            spawn_open_folder_watch(
                Arc::clone(&state),
                account,
                cfg.clone(),
                stop.clone(),
                idle_ceiling,
            );
            let cfg = cfg.clone();
            let mut stop = stop.clone();
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                if !idle_known(&state, account, &mut stop).await {
                    return;
                }
                // Backoff rather than the flat two-minute sleep this used to
                // take on failure. A refused IDLE is usually a dropped socket
                // and retrying costs one connection; two minutes of blindness
                // for it was the worst trade in the loop.
                let mut backoff = std::time::Duration::from_secs(2);
                let ceiling_backoff = std::time::Duration::from_secs(120);
                loop {
                    // A refused password is not a dropped socket. Retried on
                    // this backoff it was thirty refused sign-ins an hour, on
                    // top of the sweep's; it waits now, and the sweep loop
                    // asks the server once an hour for all of them.
                    if !wait_for_signin(&state, account, &mut stop).await {
                        return;
                    }
                    let armed = std::time::Instant::now();
                    // The account may be removed while IDLE holds the
                    // connection open, for up to twenty minutes: the switch
                    // ends the watch there and then.
                    let watching = tokio::select! {
                        w = petrel_providers::imap::idle_watch(&cfg, "INBOX", idle_ceiling, || {
                            // Full means a pass is already coming; dropping
                            // this one loses nothing.
                            let _ = wake_tx.try_send(());
                        }) => w,
                        _ = stopped(&mut stop) => return,
                        // Stood down meanwhile: the session goes, and the
                        // watcher waits at the top for a password that works.
                        _ = crate::signin::until_signed_out(&state, account) => continue,
                    };
                    match watching {
                        Ok(()) => {
                            backoff = std::time::Duration::from_secs(2);
                            log_sync(&format!(
                                "idle held {:.0}s, reconnecting",
                                armed.elapsed().as_secs_f32()
                            ));
                        }
                        Err(e) => {
                            if e.is_sign_in_refused() {
                                log_sync(&format!(
                                    "account {account}: sign-in refused; standing down"
                                ));
                                state.refused_by(account, &stop);
                                continue;
                            }
                            log_sync(&format!(
                                "idle failed after {:.0}s, retrying in {}s: {}",
                                armed.elapsed().as_secs_f32(),
                                backoff.as_secs(),
                                loggable(&e)
                            ));
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = stopped(&mut stop) => return,
                            }
                            backoff = (backoff * 2).min(ceiling_backoff);
                        }
                    }
                    if wake_tx.is_closed() || *stop.borrow() {
                        return;
                    }
                }
            });
        }

        let mut swept = std::time::Instant::now();
        let mut reconciled = std::time::Instant::now();
        // Set when the server takes a refused password again: catch up at
        // once rather than at the next sweep.
        let mut resume_now = false;
        loop {
            if *stop.borrow() {
                break;
            }
            // Signed out, the account stands down: no drain, no sweep, no
            // fetch. A refused password waits out its hour, or a new password
            // restarts the account; then one sign-in, not a whole cycle, asks
            // whether the server takes it yet. Every cycle used to try, two
            // sign-ins at a time, for as long as the app ran. A missing
            // password has nothing to try with, and waits for one.
            if let Some(why) = state.signin(account) {
                let mut changed = state.signin_changed.subscribe();
                let wait = match why {
                    SignIn::Refused => state.refused_wait(account, now_ms()),
                    SignIn::Missing => Some(crate::signin::REFUSED_RETRY),
                };
                if let Some(wait) = wait {
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = changed.changed() => {}
                        _ = stopped(&mut stop) => break,
                    }
                    continue;
                }
                // Raced against the switch: a loop stopped by Sign in again
                // must not hear its old password refused after the new one
                // was saved, and write that over it.
                let Some(asked) =
                    unless_stopped(&mut stop, petrel_providers::imap::login_check(&cfg)).await
                else {
                    break;
                };
                match asked {
                    Ok(()) => {
                        log_sync(&format!("account {account}: sign-in accepted again"));
                        state.cleared_by(account, &stop);
                        // The refusal's banner was this account's, and it is
                        // untrue now, however the folders fare after this.
                        say_sync_error(&state, &stop, None);
                        resume_now = true;
                        // What waited for the account to sign in goes now,
                        // not after a cycle in which every folder succeeds:
                        // one folder failing every cycle stranded it.
                        drafts::push_held(&state, account, &stop).await;
                    }
                    Err(e) => {
                        // Refused again, or no answer at all: another hour.
                        log_sync(&format!(
                            "account {account}: sign-in still failing: {}",
                            loggable(&e)
                        ));
                        state.refused_by(account, &stop);
                        continue;
                    }
                }
            }
            // Whichever comes first: the server speaking, or the sweep falling
            // due. An account with no IDLE has no watcher, so `wake_rx` never
            // fires and the timer alone drives it — the old poll, unchanged.
            let until_sweep = sweep_every.saturating_sub(swept.elapsed());
            let wait = if has_idle { until_sweep } else { every };
            let by_wake = if std::mem::take(&mut resume_now) {
                false
            } else {
                tokio::select! {
                    got = wake_rx.recv() => {
                        // The watcher only ends if the loop is gone, but a closed
                        // channel here would otherwise spin.
                        if got.is_none() {
                            tokio::time::sleep(wait).await;
                            false
                        } else {
                            true
                        }
                    }
                    _ = tokio::time::sleep(wait) => false,
                    // The account was removed: stand down rather than run one
                    // more cycle against a server the app no longer owns.
                    _ = stopped(&mut stop) => break,
                }
            };
            // Looked at again after the wait, not only before it: a watcher
            // that met a refusal while the loop slept has stood the account
            // down, and the cycle the timer is about to run would ask the
            // server with the refused password anyway, the queue first.
            if state.signin(account).is_some() {
                continue;
            }
            let cycle = std::time::Instant::now();
            // A wake still takes the sweep if one has come due meanwhile, so a
            // busy mailbox cannot starve the other folders.
            let sweeping = !by_wake || swept.elapsed() >= sweep_every;
            if sweeping {
                swept = std::time::Instant::now();
            }

            // The cycle runs under the account's switch, as the first pass
            // does, so a loop stopped by Sign in again or a removal goes no
            // further; and it ends early once a step meets a refusal, rather
            // than asking again with every step after it.
            let mine = stop.clone();
            let Some(ran) = unless_stopped(&mut stop, async {
                // Deliver first, so the fetch that follows confirms local state
                // rather than contradicting it — the same ordering as startup.
                // Always, on both paths: the person's own changes are the ones
                // they are watching for, and holding them for a sweep would make
                // the app feel slower at exactly the moment it must not.
                let _ = drain_actions(
                    Arc::clone(&state),
                    account,
                    cfg.clone(),
                    has_move,
                    has_uidplus,
                    account_is_gmail(&cfg),
                    &mine,
                )
                .await;
                state.nudge_send(account);
                if state.signin(account).is_some() {
                    return None;
                }
                if sweeping {
                    // Folders were discovered once, at launch. A mailbox made in
                    // webmail — or by a rule on the server — never appeared until
                    // the app was restarted, and mail filed into it was mail
                    // Petrel could not see. The sweep is where "what does the
                    // server have" belongs, and the survey is one LIST.
                    //
                    // It is also where a launch that had no network learns what
                    // the server can do: the drain moves rather than copies from
                    // here on, and the waiting IDLE watchers start.
                    if let Some(caps) = refresh_folders(&state, account, &cfg, &mine).await {
                        has_move = caps.has_move;
                        has_uidplus = caps.has_uidplus;
                        has_idle = caps.has_idle;
                    }
                    // The survey is the sweep's first sign-in: refused there,
                    // the bin's expiry, the reconcile and the sync would each
                    // have been refused again.
                    if state.signin(account).is_some() {
                        return None;
                    }
                    tend_the_bin(&state, account).await;
                    if reconciled.elapsed() >= reconcile_every {
                        reconciled = std::time::Instant::now();
                        reconcile_ghost_placements(&state, account, &cfg).await;
                    }
                }

                // One connection for the whole account, STATUS-gated per folder:
                // a quiet cycle costs a line per folder, not a login per folder.
                let scope = if sweeping {
                    Scope::Everything
                } else {
                    Scope::Inbox
                };
                let report = run_sync_cycle(&state, account, &cfg, false, scope).await;
                // Refused everywhere: the labels would only be refused too.
                if account_is_gmail(&cfg)
                    && !refused_everywhere(&report)
                    && state.signin(account).is_none()
                {
                    // One round trip when nothing changed; live labels when it did.
                    // On both paths: a new message usually arrives with its labels.
                    run_label_sweep(&state, account, &cfg).await;
                    run_thrid_sweep(&state, account, &cfg).await;
                }
                Some(report)
            })
            .await
            else {
                break;
            };
            // Stood down part-way: the drain met a refusal.
            let Some(report) = ran else {
                continue;
            };
            let (fresh, failures) = (report.fresh, report.failures);

            let trouble: Option<String> = if failures > 0 {
                Some(format!("{failures} folder(s) failed"))
            } else {
                None
            };
            if fresh > 0 {
                log_sync(&format!("poll: {fresh} new message(s)"));
                // The list watches this count, so bumping it is what makes
                // new mail appear without the user doing anything.
            }
            // Only a pass that both found nothing and hit nothing clears the
            // banner: a poll that failed halfway is not proof that sync is well.
            // And only a pass that actually asked the server: a cycle with no
            // folders to sync — the folder probe failed, so there are none —
            // proves nothing, and used to clear the sign-in banner the probe
            // had just raised.
            if trouble.is_none() && report.attempted > 0 {
                say_sync_error(&state, &stop, None);
                // Signed in after all: a refusal an IDLE reconnect saw a
                // moment ago does not stand.
                if state.signin(account) == Some(SignIn::Refused) {
                    state.cleared_by(account, &stop);
                }
            } else if report.attempted > 0 && report.failures >= report.attempted {
                // A pass that failed everywhere is the account failing, not a
                // folder. A password revoked after launch used to fail every
                // cycle with nothing on screen but an ageing "last synced":
                // only the startup pass ever raised the banner.
                let raw = report
                    .last_failure
                    .clone()
                    .unwrap_or_else(|| crate::diag::NO_FOLDER_SYNCED.into());
                if is_sign_in_refusal(&raw) {
                    state.refused_by(account, &stop);
                }
                say_sync_error(
                    &state,
                    &stop,
                    Some(friendly_sync_error_for(&cfg.host, &raw)),
                );
            }
            // Signed in, whatever became of the folders: what waited for the
            // account to sign in goes now. Only a cycle in which every folder
            // succeeded used to send it, so one folder failing every time
            // stranded it for the session.
            if report.attempted > report.failures && state.signin(account).is_none() {
                drafts::push_held(&state, account, &stop).await;
            }
            // The two paths are meant to cost very different amounts, and this
            // is where that stops being a claim.
            log_sync(&format!(
                // Tagged with the account, because two of them write to this
                // log and they are not comparable: one had twelve folders and
                // the other a hundred and one, so an untagged "sweep: 14.3s"
                // says nothing about whether that is good or bad. The id
                // rather than the address — a log is not the place for it.
                "account {account} {}: {:.1}s",
                if sweeping { "sweep" } else { "wake" },
                cycle.elapsed().as_secs_f32()
            ));
        }
    });
}

/// Re-surveys the server's folders, so ones made elsewhere appear.
///
/// The same LIST the first pass runs, and the same filtering: labels that
/// are already Petrel tags stay tags, and \Noselect containers are
/// hierarchy rather than mailboxes. Failure is silent by design — the
/// folders already known are still right, and the banner belongs to sync.
///
/// The survey is a probe, so it also records what the server can do and
/// hands that back. A launch with no network never ran its own probe, and
/// what it never learned used to stay unlearned for the whole session: no
/// IDLE, no MOVE, and Gmail not recognised as Gmail.
async fn refresh_folders(
    state: &Arc<AppState>,
    account: i64,
    cfg: &ImapConfig,
    stop: &tokio::sync::watch::Receiver<bool>,
) -> Option<crate::state::ServerCaps> {
    let report = match petrel_providers::imap::probe(cfg, 0).await {
        Ok(report) => report,
        Err(e) => {
            // Silent, but not about a refused password: the sweep stands
            // down on it rather than asking three more times.
            if e.is_sign_in_refused() {
                state.refused_by(account, stop);
            }
            return None;
        }
    };
    let caps = learned_caps(&report, cfg);
    let gmail = caps.is_gmail;
    state.set_caps(account, caps);
    let rows = survey_rows(&report.folders, gmail);
    // In a block of its own: the guard has to be gone before the await
    // below, and a `drop` does not convince the compiler of that.
    let stored = {
        let Ok(mut store) = state.store.lock() else {
            return Some(caps);
        };
        let tag_names: Vec<String> = store
            .tags_for_account(account)
            .map(|ts| ts.into_iter().map(|t| t.name).collect())
            .unwrap_or_default();
        let rows = without_tag_labels(rows, &tag_names, gmail);
        match store.sync_folders(account, &rows) {
            Ok(n) if n > 0 => log_sync(&format!("{n} folder(s) stored")),
            Ok(_) => {}
            Err(e) => log_sync(&format!("folder sync failed: {e}")),
        }
        let _ = store.set_folder_delimiters(account, &survey_delimiters(&report.folders));
        if gmail {
            let _ = store.set_account_kind(account, "gmail");
        }
        store
            .set_all_mail_folders(account, &all_mail_paths(&report.folders, gmail))
            .is_ok()
    };
    // What the launch survey did, for a launch that had no network to do it.
    if stored {
        state.mark_surveyed(account);
    }
    create_waiting_folders(state, account, cfg).await;
    Some(caps)
}

/// What a probe that answered says the server can do.
///
/// Gmail is the provider whose folders are labels, and the only one we can
/// identify from what it advertises before any mail arrives. Recording it is
/// what makes archiving keep the user's other labels instead of clearing
/// them.
fn learned_caps(
    report: &petrel_providers::imap::ProbeReport,
    cfg: &ImapConfig,
) -> crate::state::ServerCaps {
    let c = &report.greeting_capabilities;
    crate::state::ServerCaps {
        has_move: c.move_,
        has_uidplus: c.uidplus,
        has_idle: c.idle,
        is_gmail: cfg.host.contains("gmail")
            || report.folders.iter().any(|f| f.name.starts_with("[Gmail]")),
        known: true,
    }
}

/// A survey's folders as the store takes them: path and the role the server
/// flags.
///
/// \Noselect containers ([Gmail] itself) are hierarchy, not mailboxes:
/// nothing to list, nothing to sync. Outside Gmail a mailbox flagged `\All`
/// is a view of every message, not the archive — archiving into it is a
/// move the server cannot make — so it gets no role. On Gmail it is the
/// archive (`all_mail_paths` says why).
fn survey_rows(
    folders: &[petrel_providers::imap::FolderInfo],
    gmail: bool,
) -> Vec<(String, Option<String>)> {
    folders
        .iter()
        .filter(|f| petrel_providers::imap::selectable(f))
        .map(|f| {
            let role = if !gmail && petrel_providers::imap::is_all_mailbox(f) {
                None
            } else {
                petrel_providers::imap::special_use_role(f).map(|r| r.to_string())
            };
            (f.name.clone(), role)
        })
        .collect()
}

/// Each listed folder's hierarchy delimiter, for `set_folder_delimiters`.
///
/// What nests inside what is the server's to say, and the LIST line says it.
/// Recorded, it is the only separator the store and the rail nest by; until
/// then they read both `/` and `.`, and a top-level `Trash.2019` on a `/`
/// server was emptied with the Trash.
fn survey_delimiters(
    folders: &[petrel_providers::imap::FolderInfo],
) -> Vec<(String, Option<String>)> {
    folders
        .iter()
        .map(|f| (f.name.clone(), f.delimiter.clone()))
        .collect()
}

/// Waits until this account's server is known to IDLE: false when it is
/// known not to, or when the account stands down.
///
/// The watchers start before a launch that had no network knows anything
/// (see the spawn), and a survey that gets through later is what tells
/// them. Two seconds is a wait on an in-memory flag, not on the server.
async fn idle_known(
    state: &AppState,
    account: i64,
    stop: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    loop {
        let caps = state.caps(account);
        if caps.known {
            return caps.has_idle;
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            _ = stopped(stop) => return false,
        }
    }
}

/// The mailboxes a survey found flagged `\All` that are not the archive, by
/// path: none on Gmail, where All Mail is the archive; elsewhere, what
/// `folders_to_sync_from` must leave alone. See `set_all_mail_folders`.
fn all_mail_paths(folders: &[petrel_providers::imap::FolderInfo], gmail: bool) -> Vec<String> {
    if gmail {
        return Vec::new();
    }
    folders
        .iter()
        .filter(|f| petrel_providers::imap::selectable(f))
        .filter(|f| petrel_providers::imap::is_all_mailbox(f))
        .map(|f| f.name.clone())
        .collect()
}

/// Creates on the server the folders made here that it does not have yet.
///
/// Run straight after each survey, which has just said what the server
/// holds: anything made here and still missing from that list is created
/// now, and subscribed. This is the retry a background create never had —
/// one that failed, or never ran because the app quit first, used to be
/// found missing by the next survey and deleted here too. Nothing waiting
/// means no connection at all, which is the ordinary case.
async fn create_waiting_folders(state: &Arc<AppState>, account: i64, cfg: &ImapConfig) {
    let waiting = match state.store.lock() {
        Ok(store) => store.folders_awaiting_server(account).unwrap_or_default(),
        Err(_) => return,
    };
    for (id, path) in waiting {
        match petrel_providers::imap::create_folder(cfg, &path).await {
            Ok(()) => {
                if let Ok(store) = state.store.lock() {
                    let _ = store.confirm_folder_on_server(id);
                }
                log_sync(&format!("created folder {id} on the server"));
            }
            Err(e) => log_sync(&format!(
                "server create of folder {id} failed, next sync retries: {}",
                loggable(&e)
            )),
        }
    }
}

/// Which folders a pass covers.
///
/// An IDLE wake is a report about the folder being watched and nothing else:
/// the server said the inbox changed, not that the other hundred folders did.
/// Sweeping all of them to find that out costs half a minute on a real
/// account, measured, and the mail the person is waiting for sits behind it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scope {
    /// The inbox alone, for a wake. 794ms on the account this was written
    /// against, against 13.9s for all hundred and one folders.
    Inbox,
    /// Every folder, for the sweep that catches what other clients did.
    Everything,
    /// One role folder, when the person opened that mailbox. Those
    /// folders are not on the wake path, and waiting out the
    /// five-minute sweep is what made opening them feel like the
    /// server had not been asked.
    Role(&'static str),
    /// A folder the user made, addressed by row id — same reason as
    /// Role, but user folders have no role to filter on.
    FolderId(i64),
}

/// The targets a scope leaves.
///
/// Its own function so the narrowing can be tested without a server. Getting
/// it wrong in the quiet direction gives an inbox that never syncs, and that
/// is the one failure nobody reports as a bug — it just looks like no mail.
fn narrow(targets: Vec<(String, String, i64)>, scope: Scope) -> Vec<(String, String, i64)> {
    match scope {
        Scope::Everything => targets,
        // By role rather than by position. `folders_to_sync` happens to put
        // the inbox first today, and a filter that trusted that would break
        // silently the day somebody reorders the list.
        Scope::Inbox => targets
            .into_iter()
            .filter(|(role, _, _)| role == "inbox")
            .collect(),
        Scope::Role(want) => targets
            .into_iter()
            .filter(|(role, _, _)| role == want)
            .collect(),
        Scope::FolderId(want) => targets
            .into_iter()
            .filter(|(_, _, id)| *id == want)
            .collect(),
    }
}

/// What one cycle did: what it fetched, what it could not, and the last
/// failure's text, so the banner can say what went wrong rather than that
/// something did.
#[derive(Default)]
struct CycleReport {
    fresh: usize,
    failures: usize,
    attempted: usize,
    last_failure: Option<String>,
}

/// Whether a cycle failed everywhere because the server refused the
/// password at sign-in: the provider's verdict, carried in the failure's
/// text once the error has lost its type.
fn refused_everywhere(report: &CycleReport) -> bool {
    report.attempted > 0
        && report.failures >= report.attempted
        && report
            .last_failure
            .as_deref()
            .is_some_and(is_sign_in_refusal)
}

/// Folders that may be fetched when the person opens them. Inbox has
/// IDLE. Archive is here for the accounts where it is a folder of its own;
/// on Gmail it is All Mail, which `folders_to_sync` leaves out, so opening
/// it there narrows to nothing. Snoozed, outbox and tags have no folder to
/// SELECT.
const ON_OPEN_ROLES: &[&str] = &["sent", "drafts", "spam", "trash", "starred", "archive"];

fn open_sync_scope(view: &str) -> Option<Scope> {
    if let Some(role) = ON_OPEN_ROLES.iter().copied().find(|r| *r == view) {
        return Some(Scope::Role(role));
    }
    view.strip_prefix("folder:")
        .and_then(|rest| rest.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .map(Scope::FolderId)
}

/// How long after an on-open fetch the same mailbox is left alone. A click
/// a moment after the last one has nothing new to ask for, and every ask
/// is a login.
const ON_OPEN_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether a mailbox last fetched at `last` is due again at `now`.
fn on_open_due(last: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    match last {
        Some(at) => now.duration_since(at) >= ON_OPEN_COOLDOWN,
        None => true,
    }
}

fn claim_folder_sync(state: &AppState, key: &str) -> bool {
    let last = state
        .folder_synced_at
        .lock()
        .ok()
        .and_then(|m| m.get(key).copied());
    if !on_open_due(last, std::time::Instant::now()) {
        return false;
    }
    let Ok(mut set) = state.folder_sync_inflight.lock() else {
        return false;
    };
    set.insert(key.to_string())
}

/// `fetched` is whether the server was actually asked; a claim released
/// before that (no store, no config) starts no cooldown.
fn release_folder_sync(state: &AppState, key: &str, fetched: bool) {
    if fetched && let Ok(mut m) = state.folder_synced_at.lock() {
        m.insert(key.to_string(), std::time::Instant::now());
    }
    if let Ok(mut set) = state.folder_sync_inflight.lock() {
        set.remove(key);
    }
}

/// Fetches one folder now, without waiting for the sweep.
///
/// Opening Sent or Drafts used to show only what the last sweep had. A
/// wake is the inbox alone, on purpose; putting every folder back on
/// that path is the half-minute wait this split removed. One folder is
/// the same cost as a wake.
pub(crate) fn spawn_view_sync(state: Arc<AppState>, account: i64, view: &str) {
    let Some(scope) = open_sync_scope(view) else {
        return;
    };
    // Signed out: opening a folder is not a reason for another refused
    // sign-in, and an account with no password has none to offer.
    if state.signin(account).is_some() {
        return;
    }
    // A mailbox with no folder of its own on this account — Archive on
    // Gmail — has nothing to fetch. Nor, until a survey this session has
    // stored what it found, does Archive: the role can sit on a server's view
    // of every message, which the survey marks and sync then leaves alone,
    // and the sweep straight after that survey fetches the real one anyway.
    if scope == Scope::Role("archive") && !state.surveyed(account) {
        return;
    }
    if narrow(folders_to_sync(&state, account), scope).is_empty() {
        return;
    }
    let key = view.to_string();
    if !claim_folder_sync(&state, &key) {
        return;
    }
    let stop = state.stop_signal(account);
    tauri::async_runtime::spawn(async move {
        let cfg = {
            let Ok(store) = state.store() else {
                release_folder_sync(&state, &key, false);
                return;
            };
            crate::config::imap_config_for(&store, account)
        };
        let Some(cfg) = cfg else {
            release_folder_sync(&state, &key, false);
            return;
        };
        let report = run_sync_cycle(&state, account, &cfg, false, scope).await;
        // Refused: the account stands down, so the next folder opened does
        // not sign in again.
        if refused_everywhere(&report) {
            state.refused_by(account, &stop);
        }
        if report.fresh > 0 {
            log_sync(&format!("account {account} {key}: {} new", report.fresh));
            state
                .last_sync_ms
                .store(crate::state::now_ms(), Ordering::Relaxed);
        }
        release_folder_sync(&state, &key, true);
    });
}

/// The folder behind the view on screen, if this account has one to watch.
///
/// The same folders an open fetches: the inbox is watched already, and the
/// views without a folder have nothing to IDLE on.
fn watch_target(state: &AppState, account: i64, open: Option<&(i64, String)>) -> Option<String> {
    let (owner, view) = open?;
    if *owner != account {
        return None;
    }
    let scope = open_sync_scope(view)?;
    narrow(folders_to_sync(state, account), scope)
        .into_iter()
        .next()
        .map(|(_, path, _)| path)
}

/// Keeps the folder on screen as current as the inbox.
///
/// A second IDLE, on whichever folder is open. Polling it was the other way,
/// and it loses on both counts: a login a minute for as long as the folder
/// stays open, and still up to a minute late. IDLE costs nothing while
/// nothing happens, a click elsewhere is DONE, EXAMINE and IDLE on the same
/// socket, and a change is on screen as fast as new mail in the inbox — the
/// wake runs the same one-folder pass that opening the folder does.
///
/// `ceiling` is the inbox watch's, for the same reason: a connection held
/// past RFC 2177's limit gets dropped without anyone being told.
fn spawn_open_folder_watch(
    state: Arc<AppState>,
    account: i64,
    cfg: ImapConfig,
    stop: tokio::sync::watch::Receiver<bool>,
    ceiling: std::time::Duration,
) {
    // Carries the folder that spoke. Capacity one for the inbox watcher's
    // reason: a wake during a pass folds into the single pass after it.
    let (wake_tx, mut wake_rx) = tokio::sync::mpsc::channel::<String>(1);
    {
        let state = Arc::clone(&state);
        let cfg = cfg.clone();
        let mut stop = stop.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let path = tokio::select! {
                    got = wake_rx.recv() => match got {
                        Some(path) => path,
                        None => return,
                    },
                    _ = stopped(&mut stop) => return,
                };
                // By id, so a role folder and one of the person's own are
                // asked for the same way.
                let Some(id) = folders_to_sync(&state, account)
                    .into_iter()
                    .find(|(_, p, _)| *p == path)
                    .map(|(_, _, id)| id)
                else {
                    continue;
                };
                // Signed out, a wake asks nothing: each new message in the
                // folder on screen was one more refused sign-in.
                if state.signin(account).is_some() {
                    continue;
                }
                let cycle = std::time::Instant::now();
                let report =
                    run_sync_cycle(&state, account, &cfg, false, Scope::FolderId(id)).await;
                if refused_everywhere(&report) {
                    state.refused_by(account, &stop);
                }
                log_sync(&format!(
                    "account {account} folder {id} wake: {:.1}s",
                    cycle.elapsed().as_secs_f32()
                ));
            }
        });
    }
    let mut stop = stop;
    tauri::async_runtime::spawn(async move {
        if !idle_known(&state, account, &mut stop).await {
            return;
        }
        let mut open = state.open_view.subscribe();
        let (aim, mut follow) = tokio::sync::watch::channel(None::<String>);
        let mut backoff = std::time::Duration::from_secs(2);
        let backoff_ceiling = std::time::Duration::from_secs(120);
        loop {
            aim.send_replace(watch_target(
                &state,
                account,
                open.borrow_and_update().as_ref(),
            ));
            if aim.borrow().is_none() {
                // Nothing to watch here: no connection until there is.
                tokio::select! {
                    changed = open.changed() => if changed.is_err() { return },
                    _ = stopped(&mut stop) => return,
                }
                continue;
            }
            // Signed out: nothing until the password works. Then aim again,
            // since the folder on screen may have changed meanwhile.
            if state.signin(account).is_some() {
                if !wait_for_signin(&state, account, &mut stop).await {
                    return;
                }
                continue;
            }
            let armed = std::time::Instant::now();
            let watching = {
                let wake_tx = wake_tx.clone();
                let watch =
                    petrel_providers::imap::idle_follow(&cfg, &mut follow, ceiling, |path| {
                        // Full means a pass is already coming.
                        let _ = wake_tx.try_send(path.to_string());
                    });
                tokio::pin!(watch);
                loop {
                    tokio::select! {
                        w = &mut watch => break w,
                        // Re-aimed without leaving the watch: the provider
                        // follows `aim` on the connection it holds.
                        changed = open.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            aim.send_replace(watch_target(&state, account, open.borrow_and_update().as_ref()));
                        }
                        _ = stopped(&mut stop) => return,
                        // Stood down meanwhile: the session goes now, rather
                        // than at its ceiling, waking the folder's cycle with
                        // a refused password on every change until then.
                        _ = crate::signin::until_signed_out(&state, account) => break Ok(()),
                    }
                }
            };
            match watching {
                Ok(()) => backoff = std::time::Duration::from_secs(2),
                Err(e) if e.is_sign_in_refused() => {
                    log_sync(&format!("account {account}: folder watch sign-in refused"));
                    state.refused_by(account, &stop);
                }
                Err(e) => {
                    log_sync(&format!(
                        "folder watch failed after {:.0}s, retrying in {}s: {e}",
                        armed.elapsed().as_secs_f32(),
                        backoff.as_secs()
                    ));
                    // A click elsewhere is a reason to try again sooner: the
                    // folder that failed may be one the server no longer has.
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        changed = open.changed() => if changed.is_err() { return },
                        _ = stopped(&mut stop) => return,
                    }
                    backoff = (backoff * 2).min(backoff_ceiling);
                }
            }
        }
    });
}

/// One sync cycle for one account: every folder, one connection.
///
/// The shape of the whole optimisation. A cycle logs in once, asks one
/// STATUS line per folder, and only selects and fetches the folders where
/// something actually moved — so a quiet cycle over a hundred folders is a
/// hundred cheap lines on one connection, and a relaunch re-downloads
/// nothing it already holds: a folder with a watermark is only ever asked
/// for what is above it. Flag changes made elsewhere ride along via
/// CONDSTORE where the server has it.
async fn run_sync_cycle(
    state: &Arc<AppState>,
    account: i64,
    cfg: &ImapConfig,
    verbose: bool,
    scope: Scope,
) -> CycleReport {
    let targets = narrow(folders_to_sync(state, account), scope);
    if targets.is_empty() {
        return CycleReport::default();
    }
    let window: u32 = std::env::var("PETREL_SYNC_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    // Taken before the store, so the two locks are never held together.
    let last_seen = state
        .folder_seen
        .lock()
        .map(|m| m.clone())
        .unwrap_or_default();
    let passes: Vec<petrel_providers::imap::FolderPass> = {
        let Ok(store) = state.store.lock() else {
            return CycleReport::default();
        };
        targets
            .iter()
            .map(|(_, path, fid)| petrel_providers::imap::FolderPass {
                path: path.clone(),
                // Floored by the last-seen UIDNEXT: moving the newest message
                // out of a folder drops max_uid, and a watermark that falls
                // re-fetches mail the server still holds there — the moved
                // conversation walking straight back into the inbox.
                since_uid: {
                    let held = store.max_uid(*fid).ok().flatten().unwrap_or(0);
                    let next = store.folder_uidnext(*fid).ok().flatten().unwrap_or(0);
                    held.max(next.saturating_sub(1))
                },
                expected_validity: store.folder_validity(*fid).ok().flatten(),
                since_uidnext: store.folder_uidnext(*fid).ok().flatten(),
                since_modseq: store.folder_modseq(*fid).ok().flatten(),
                seed_window: window,
                // Every pass looks, not only the sweep: mail another client
                // moved out of the inbox wakes the inbox's IDLE, and used to
                // stay on screen there until the twenty-minute reconcile.
                removal: Some(petrel_providers::imap::RemovalCheck {
                    held: store
                        .uid_placement_count(*fid)
                        .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
                        .unwrap_or(0),
                    last_seen: last_seen.get(fid).copied(),
                }),
            })
            .collect()
    };

    let mut fresh = 0usize;
    // Messages that genuinely *arrived* — a watermark fetch into the inbox,
    // not a seed window and not backfill. These are what filter rules run on:
    // "on arrival" must never mean "on downloading five years of archive".
    let mut arrivals: Vec<i64> = Vec::new();
    let inbox_folder: Option<i64> = {
        let Ok(store) = state.store.lock() else {
            return CycleReport::default();
        };
        store.folder_for_role(account, "inbox").ok().flatten()
    };
    let outcomes = {
        let st = Arc::clone(state);
        let ids: Vec<i64> = targets.iter().map(|(_, _, id)| *id).collect();
        let arriving: Vec<bool> = passes
            .iter()
            .zip(&ids)
            // Synced before, by either watermark. `since_uid > 0` alone
            // read as "we hold mail here", so the very first message ever to
            // land in an inbox that had been empty at seed time was not an
            // arrival and no rule ever saw it. A recorded UIDNEXT is the
            // honest signal — a first-ever pass has neither, which is what
            // keeps rules off the seed.
            .map(|(p, fid)| {
                (p.since_uid > 0 || p.since_uidnext.is_some()) && Some(*fid) == inbox_folder
            })
            .collect();
        let arrivals = &mut arrivals;
        // Gmail's custom flags are labels, and the label sweep owns them.
        let want_keywords = !account_is_gmail(cfg);
        petrel_providers::imap::sync_pass(cfg, &passes, want_keywords, |index, uid, flags, raw| {
            // Compression happens out here, before the lock: one 20MB
            // attachment message compressed inside it stalled every click
            // and count in the app for the duration (measured at 11s).
            let _ = st.blobs.write(raw);
            let Ok(mut store) = st.store.lock() else {
                return;
            };
            if ingest_fenced(&mut store, &st.blobs, account, ids[index], uid, flags, raw)
                == Some(true)
            {
                fresh += 1;
                st.seeded.fetch_add(1, Ordering::Relaxed);
                if arriving[index] {
                    // The id of what just landed, by its placement.
                    if let Ok(Some(mid)) = store.message_id_at(ids[index], uid) {
                        arrivals.push(mid);
                    }
                }
            }
        })
        .await
    };
    let outcomes = match outcomes {
        Ok(o) => o,
        Err(e) => {
            log_sync(&format!(
                "sync cycle failed before any folder: {}",
                loggable(&e)
            ));
            return CycleReport {
                fresh,
                failures: targets.len(),
                attempted: targets.len(),
                last_failure: Some(e.to_string()),
            };
        }
    };

    use petrel_providers::imap::PassOutcome;
    let mut failures = 0usize;
    let mut last_failure: Option<String> = None;
    let mut server_total = 0usize;
    // Whether anything a list could be showing changed: arrived, left, or
    // was flagged elsewhere.
    let mut moved = fresh > 0;
    for (((_, path, folder_id), pass), outcome) in targets.iter().zip(&passes).zip(&outcomes) {
        // Placements this folder lost to another client, rule or phone.
        let mut left = 0usize;
        match outcome {
            PassOutcome::Unchanged {
                uid_validity,
                highest_modseq,
                uid_next,
                total,
                seen,
                survivors,
            } => {
                server_total += *total as usize;
                if let Ok(mut store) = state.store.lock() {
                    if pass.expected_validity.is_none() {
                        let _ = store.set_folder_validity(*folder_id, *uid_validity);
                    }
                    // A quiet folder with no baselines adopts them, so the
                    // next change is a diff instead of a mystery.
                    if pass.since_modseq.is_none()
                        && let Some(m) = highest_modseq
                    {
                        let _ = store.set_folder_modseq(*folder_id, *m);
                    }
                    if pass.since_uidnext.is_none()
                        && let Some(n) = uid_next
                    {
                        let _ = store.set_folder_uidnext(*folder_id, *n);
                    }
                }
                left = settle_departures(state, *folder_id, *seen, survivors.as_ref());
            }
            PassOutcome::Fetched {
                fetched,
                uid_validity,
                highest_modseq,
                uid_next,
                flag_updates,
                keyword_updates,
                total,
                seen,
                survivors,
            } => {
                server_total += *total as usize;
                let mut reflagged = 0usize;
                let mut retagged = 0usize;
                if let Ok(mut store) = state.store.lock() {
                    if pass.expected_validity.is_none() {
                        let _ = store.set_folder_validity(*folder_id, *uid_validity);
                    }
                    if let Some(m) = highest_modseq {
                        let _ = store.set_folder_modseq(*folder_id, *m);
                    }
                    if let Some(n) = uid_next {
                        let _ = store.set_folder_uidnext(*folder_id, *n);
                    }
                    for (uid, flags) in flag_updates {
                        if store
                            .set_flags_by_uid(*folder_id, *uid, *flags)
                            .unwrap_or(false)
                        {
                            reflagged += 1;
                        }
                    }
                    // Keywords other clients set become tags here, and ones
                    // they removed stop being tags — the inbound half of the
                    // tag story on servers where a tag is an IMAP keyword.
                    if !keyword_updates.is_empty() {
                        retagged = store
                            .apply_keywords(account, *folder_id, keyword_updates)
                            .unwrap_or(0);
                    }
                }
                if verbose || *fetched > 0 || reflagged > 0 || retagged > 0 {
                    let tags = if retagged > 0 {
                        format!(", {retagged} tag change(s)")
                    } else {
                        String::new()
                    };
                    // By id, not by name. A folder's name is the person's
                    // own words — "Legal", "Job hunt", a client's name — and
                    // a log is not the place for it. The id names the row in
                    // their own store if anyone needs to look.
                    log_sync(&format!(
                        "folder {folder_id}: {fetched} fetched, {reflagged} flag update(s){tags}"
                    ));
                }
                left = settle_departures(state, *folder_id, *seen, survivors.as_ref());
                moved |= *fetched > 0 || reflagged > 0 || retagged > 0;
            }
            // A server that answered STATUS always names UIDVALIDITY. One
            // that named nothing did not answer — a session that died
            // mid-pass reads every later folder as empty — and re-mapping on
            // that stripped the server numbers from all but a folder's
            // newest messages. Not a reset; a failure.
            PassOutcome::ValidityChanged { now: None } => {
                log_sync(&format!(
                    "folder {folder_id}: STATUS named no UIDVALIDITY; not treated as a reset"
                ));
                failures += 1;
            }
            PassOutcome::ValidityChanged { now } => {
                log_sync(&format!(
                    "folder {folder_id}: UIDVALIDITY reset ({:?} -> {now:?}); re-mapping",
                    pass.expected_validity
                ));
                if let Ok(mut store) = state.store.lock() {
                    // The modseq domain does not survive a renumbering.
                    let _ = store.clear_folder_modseq(*folder_id);
                }
                // Nor does a count against old numbers.
                if let Ok(mut seen) = state.folder_seen.lock() {
                    seen.remove(folder_id);
                }
                match recover_folder(state, account, cfg, path, *folder_id).await {
                    Ok(_) => {
                        moved = true;
                        // A change queued against the old numbering was
                        // refused by the server's SELECT and waits. Its
                        // number is learned again now, so it goes now,
                        // rather than at the next sweep minutes away.
                        state.nudge_drain(account);
                    }
                    Err(e) => {
                        log_sync(&format!(
                            "folder {folder_id}: recovery failed: {}",
                            loggable(&e)
                        ));
                        last_failure = Some(e.to_string());
                        failures += 1;
                    }
                }
            }
            PassOutcome::Failed { detail } => {
                if verbose {
                    log_sync(&format!("folder {folder_id}: FAILED: {}", loggable(detail)));
                }
                last_failure = Some(detail.clone());
                failures += 1;
            }
        }
        if left > 0 {
            log_sync(&format!(
                "folder {folder_id}: {left} gone from the server since the last look"
            ));
            moved = true;
        }
    }
    state.server_total.store(server_total, Ordering::Relaxed);
    if failures == 0 {
        state.last_sync_ms.store(now_ms(), Ordering::Relaxed);
    }
    if !arrivals.is_empty() {
        apply_rules_to(state, account, &arrivals);
        announce_elsewhere(state, account, &arrivals);
    }
    if moved {
        note_mail_moved(state);
    }
    CycleReport {
        fresh,
        failures,
        attempted: targets.len(),
        last_failure,
    }
}

/// Tells the window the mail moved, so the list on screen looks again.
pub(crate) fn note_mail_moved(state: &AppState) {
    state.mail_gen.fetch_add(1, Ordering::Relaxed);
}

/// A failure as sync.log may carry it.
///
/// A reply the IMAP parser choked on comes back with the reply quoted in the
/// error, as bytes and as text: a FETCH line with its subject and addresses.
/// The cycle has always logged those by their kind alone; backfill, recovery,
/// the Gmail sweeps and the reconcile wrote them out whole (fifth audit #191).
///
/// A refusal quotes what it refused, too. Dovecot answers a move into a
/// folder it lacks with "Mailbox doesn't exist: <name>" and Gmail with
/// "Unknown Mailbox: <name>", and a folder's name is the person's own words.
/// So a NO or a BAD is logged by its kind and its code, `NO [TRYCREATE]`,
/// never its text (review r1, R1-3). The whole of it still reaches the
/// window, where the person reads it.
pub(crate) fn loggable(e: &impl std::fmt::Display) -> String {
    let text = e.to_string();
    if is_imap_parse_error(&text) {
        "imap-parse".into()
    } else {
        refusal_by_code(&text).unwrap_or(text)
    }
}

/// `text` with a NO or BAD response cut down to its kind and code, or `None`
/// when it holds neither. Three shapes reach here. async-imap's is
/// `no response: code: Some(TryCreate), info: Some("…")`, and a code it does
/// not parse is left at the head of the info, in brackets. The other two are
/// Petrel's own, for the commands whose replies it reads itself: the
/// server's tagged reply as sent (`protocol: EXAMINE: NO [NONEXISTENT] …`),
/// and its status by name (`protocol: UID SEARCH: No …`). What comes before
/// the response is Petrel's own words, and stays.
fn refusal_by_code(text: &str) -> Option<String> {
    // The code as a server writes it: an atom at the head of the text.
    let bracketed = |s: &str| {
        let code: String = s
            .strip_prefix('[')?
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        (!code.is_empty()).then(|| code.to_ascii_uppercase())
    };
    let said = |kind: &str, code: Option<String>| match code {
        Some(code) => format!("{kind} [{code}]"),
        None => kind.to_string(),
    };
    for (marker, kind) in [("no response: ", "NO"), ("bad response: ", "BAD")] {
        let Some(at) = text.find(marker) else {
            continue;
        };
        let rest = &text[at + marker.len()..];
        let code = match rest.strip_prefix("code: Some(") {
            Some(parsed) => Some(
                parsed
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect::<String>()
                    .to_ascii_uppercase(),
            ),
            None => rest
                .split_once("info: Some(\"")
                .and_then(|(_, info)| bracketed(info)),
        };
        return Some(format!("{}{}", &text[..at], said(kind, code)));
    }
    let (command, reply) = text.strip_prefix("protocol: ")?.split_once(": ")?;
    let (status, words) = reply.split_once(' ').unwrap_or((reply, ""));
    let kind = if status.eq_ignore_ascii_case("NO") {
        "NO"
    } else if status.eq_ignore_ascii_case("BAD") {
        "BAD"
    } else {
        return None;
    };
    Some(format!(
        "protocol: {command}: {}",
        said(kind, bracketed(words))
    ))
}

/// Drops the placements a pass's search says have left `folder_id`, and
/// keeps the STATUS it saw for the next pass to compare against. Returns how
/// many went.
///
/// Only below the search's bound. A UID at or above it arrived after the
/// look — the drain moving a conversation in a moment ago — and the search
/// not naming it says nothing about it.
fn settle_departures(
    state: &AppState,
    folder_id: i64,
    seen: Option<(u32, u32)>,
    survivors: Option<&petrel_providers::imap::Survivors>,
) -> usize {
    let removed = survivors.map_or(0, |s| {
        let Ok(store) = state.store.lock() else {
            return 0;
        };
        let Ok(held) = store.placement_uids(folder_id) else {
            return 0;
        };
        let mut present: HashSet<u32> = s.uids.iter().copied().collect();
        present.extend(held.into_iter().filter(|u| *u >= s.below));
        store
            .remove_placements_absent(folder_id, &present)
            .unwrap_or(0)
    });
    if let Some(seen) = seen
        && let Ok(mut m) = state.folder_seen.lock()
    {
        m.insert(folder_id, seen);
    }
    removed
}

/// Mends one folder after the server renumbered it (UIDVALIDITY reset).
///
/// The order is the safety: quarantine and re-map by Message-ID first (the
/// store's transaction), then download what could not be matched, and record
/// the new validity *last* — so a crash anywhere in between leaves the old
/// value in place and the next pass simply runs recovery again. Message rows
/// and blobs are never deleted; the worst case is re-downloading, never data.
async fn recover_folder(
    state: &Arc<AppState>,
    account: i64,
    cfg: &ImapConfig,
    name: &str,
    folder_id: i64,
) -> Result<usize, String> {
    let depth: u32 = std::env::var("PETREL_SYNC_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let map = petrel_providers::imap::fetch_id_map(cfg, name, depth)
        .await
        .map_err(|e| format!("id map: {e}"))?;
    let outcome = {
        let mut store = state.store()?;
        store
            .remap_folder_after_reset(folder_id, &map.entries, map.complete)
            .map_err(|e| format!("remap: {e}"))?
    };
    let mut refetched = 0usize;
    if !outcome.to_fetch.is_empty() {
        let st = Arc::clone(state);
        refetched = petrel_providers::imap::fetch_uids_each(
            cfg,
            name,
            &outcome.to_fetch,
            |uid, flags, raw| {
                let _ = st.blobs.write(raw);
                let Ok(mut store) = st.store.lock() else {
                    return;
                };
                let _ = ingest_fenced(&mut store, &st.blobs, account, folder_id, uid, flags, raw);
            },
        )
        .await
        .map_err(|e| format!("refetch: {e}"))?;
    }
    {
        let mut store = state.store()?;
        store
            .set_folder_validity(folder_id, map.uid_validity)
            .map_err(|e| format!("record validity: {e}"))?;
    }
    log_sync(&format!(
        "folder {folder_id}: re-mapped {} placement(s), re-downloaded {refetched}, dropped {}",
        outcome.rematched, outcome.dropped
    ));
    Ok(refetched)
}

/// One-shot sync: fetch recent mail and ingest it. Deliberately not a sync
/// engine — that arrives with the orchestrator; this proves the path end to end
/// inside the app.
/// The folders worth pulling down, inbox first.
///
/// Deliberately not everything the server advertises:
///
/// * **All Mail is excluded.** On a labels provider it holds *every* message,
///   so syncing it would roughly double the store — and since it is what the
///   archive role maps to, it would make the Archive view mean "all your mail"
///   rather than "mail you archived".
/// * **Starred is included**, despite being a flag we already read. We only
///   read the flags of messages we *fetch* — a star on older mail, or on
///   anything archived into All Mail, never arrives, and the Starred view sits
///   empty while the server knows better. It is small by nature: a list of
///   things someone picked out by hand.
/// * **Snoozed is not here to exclude.** Gmail has the feature, but does not
///   expose it over IMAP — there is no such mailbox in the folder list.
/// * **Outbox likewise**: mail that has not reached a server yet is ours alone.
fn folders_to_sync(state: &AppState, account: i64) -> Vec<(String, String, i64)> {
    let Ok(store) = state.store.lock() else {
        return Vec::new();
    };
    folders_to_sync_from(&store, account)
}

/// The lock-free core, for callers already holding the store.
pub(crate) fn folders_to_sync_from(store: &Store, account: i64) -> Vec<(String, String, i64)> {
    // Inbox first so the view the user is looking at fills before the rest.
    const ROLES: [&str; 6] = ["inbox", "sent", "drafts", "spam", "trash", "starred"];
    let Ok(all) = store.folders(account) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String, i64)> = ROLES
        .iter()
        .filter_map(|role| {
            all.iter()
                .find(|f| f.role == *role)
                .map(|f| ((*role).to_string(), f.path.clone(), f.id))
        })
        .collect();
    // The archive, wherever it is a folder like any other. On Gmail the role
    // is All Mail, which is left out above and walked on its own
    // (backfill.rs). Everywhere else it is where archived mail lives: left
    // out, a fresh install never showed an archived message, and one another
    // client archived lost its only placement at the next inbox pass and was
    // tombstoned. The folder the drain files into and the view lists, not
    // merely the first to wear the role — and never a mailbox the server
    // flags `\All`, a view of the whole account that takes the role too.
    if matches!(
        store.placement_policy(account),
        Ok(petrel_engine::actions::PlacementPolicy::Exclusive)
    ) && let Ok(Some(id)) = store.folder_for_role(account, "archive")
        && !store.folder_is_local(id).unwrap_or(false)
        && !store.folder_is_all_mail(id).unwrap_or(false)
        && let Some(f) = all.iter().find(|f| f.id == id)
    {
        out.push(("archive".to_string(), f.path.clone(), f.id));
    }
    // Folders the user made sync too — a folder whose mail never arrives is
    // not a folder, it is a name. After the roles, so the inbox still fills
    // first. Local folders are the exception both ways: the server has never
    // heard of them, so asking it about one is a guaranteed error per cycle.
    for f in all.iter().filter(|f| f.role.is_empty()) {
        if store.folder_is_local(f.id).unwrap_or(false) {
            continue;
        }
        // A mailbox flagged `\All` is a view of every message, here as much
        // as above: it no longer takes the archive role (`settle_roles`),
        // and synced as a folder it would download the account again.
        if store.folder_is_all_mail(f.id).unwrap_or(false) {
            continue;
        }
        out.push((String::new(), f.path.clone(), f.id));
    }
    out
}

/// Drops Gmail labels that are already Petrel tags from the folder survey.
///
/// On Gmail one server object — the label — backs both of Petrel's ideas, a
/// place and a tag. A tag made here becomes a label there (deliberately: tag
/// names sync, so they survive being seen from any other client), and the
/// next survey would bring that same label back as a *folder*, so the thing
/// you made once appears twice pretending to be two things. A label that is
/// a tag stays a tag. Everywhere else folders and tags are different server
/// objects and a shared name is legitimate, so nothing is dropped.
fn without_tag_labels(
    rows: Vec<(String, Option<String>)>,
    tag_names: &[String],
    is_gmail: bool,
) -> Vec<(String, Option<String>)> {
    if !is_gmail {
        return rows;
    }
    rows.into_iter()
        .filter(|(path, role)| {
            // Role-bearing folders (Sent, Trash, Important…) are never tags.
            role.is_some() || !tag_names.iter().any(|t| t.eq_ignore_ascii_case(path))
        })
        .collect()
}

/// Ingests one fetched message, absorbing a parser panic instead of letting
/// it poison the store lock.
///
/// The sanitizer's rule is "salvage, never judge", but a bug in salvage is a
/// panic — and this callback holds the store lock, so before this fence one
/// hostile message did not cost one message, it cost every pane of the app
/// until relaunch (found the hard way: an HTML-only newsletter with an emoji
/// and a byte-walking tag stripper). The panic is still a bug and still gets
/// fixed; it is just no longer an outage while it waits to be found.
pub(crate) fn ingest_fenced(
    store: &mut Store,
    blobs: &petrel_engine::blob::BlobStore,
    account: i64,
    folder_id: i64,
    uid: u32,
    flags: i64,
    raw: &[u8],
) -> Option<bool> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store.ingest_raw(blobs, account, Some(folder_id), Some(uid), raw)
    }));
    match result {
        Ok(Ok(ingested)) => {
            let _ = store.set_message_flags(ingested.message_id, flags);
            // `was_new` is false when the bytes were already here and only a
            // placement was added — how the progress counter avoids counting
            // one message once per folder it appears in.
            Some(ingested.was_new)
        }
        Ok(Err(e)) => {
            log_sync(&format!("ingest uid {uid} failed: {e}"));
            None
        }
        Err(_) => {
            log_sync(&format!(
                "ingest uid {uid} PANICKED — message skipped, bytes not stored; this is a bug worth reporting"
            ));
            None
        }
    }
}

/// Queues new mail in an account that is not on screen, for the next status
/// poll to carry to the window's announcer, as the rules' notices go.
///
/// The window watches the inbox of the account it shows, and heard nothing
/// of the others': mail could sit unannounced in a second account all day,
/// where other clients say every account's. After the rules have run: only
/// what is still in the inbox, and still unread. `arrivals` are messages new
/// to an inbox synced before, so a first sync and the backfill are never said.
pub(crate) fn announce_elsewhere(state: &Arc<AppState>, account: i64, arrivals: &[i64]) {
    let said: Vec<crate::state::Elsewhere> = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        if store.active_account().ok().flatten() == Some(account) {
            return;
        }
        arrivals
            .iter()
            .filter(|&&id| store.message_in_role(id, "inbox").unwrap_or(false))
            .filter_map(|&id| store.thread_message(id).ok().flatten())
            .filter(|m| m.unread)
            .map(|m| crate::state::Elsewhere {
                account,
                who: if m.from_display.is_empty() {
                    m.from_addr
                } else {
                    m.from_display
                },
                subject: m.subject,
            })
            .collect()
    };
    if !said.is_empty()
        && let Ok(mut pending) = state.pending_elsewhere.lock()
    {
        pending.extend(said);
    }
}

/// Keeps the bin's clock running, and empties what has run out.
///
/// Off unless `trashRetentionDays` says otherwise, because deleting
/// somebody's mail on a timer is a promise to opt into rather than a
/// default to discover. The clock itself is maintained either way: turning
/// expiry on a month from now should not then treat everything in the bin
/// as a month old, nor as brand new.
async fn tend_the_bin(state: &Arc<AppState>, account: i64) {
    let now = crate::state::now_ms();
    let days: Option<i64> = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        let _ = store.refresh_trash_clock(account, now);
        store
            .settings()
            .ok()
            .and_then(|s| {
                s.get("trashRetentionDays")
                    .and_then(|v| v.parse::<i64>().ok())
            })
            .filter(|d| *d > 0)
    };
    let Some(days) = days else { return };
    let expired = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        // Each number with its folder's validity, read under this one guard.
        let due = store.trash_expired(account, days, now).unwrap_or_default();
        crate::commands::triage::binned(&store, account, due)
    };
    if expired.is_empty() {
        return;
    }
    log_sync(&format!(
        "trash expiry: {} message(s) older than {days} day(s)",
        expired.len()
    ));
    let _ = crate::commands::triage::destroy_trashed(state, account, expired).await;
}

/// Whether this account's server is Gmail — asked of the account's own
/// configuration rather than of shared state.
///
/// The probe's own answer lives in `AppState::caps`, per account; this is
/// the same question answered from configuration alone, for the places that
/// have a config in hand and no account id.
pub(crate) fn account_is_gmail(cfg: &ImapConfig) -> bool {
    let host = cfg.host.to_ascii_lowercase();
    host.contains("gmail") || host.contains("googlemail") || host.ends_with("google.com")
}

/// Runs the account's filter rules over newly-arrived messages.
///
/// Every enabled rule that matches contributes, in the user's order, and
/// each action goes through the ordinary triage path — locally at once,
/// queued to the server like a hand-made change, drained promptly.
///
/// Three phases, and the shape is the point. Reading a message means pulling
/// its bytes off disk and parsing the MIME, and doing that under the store
/// lock froze every click and count in the app for as long as the batch took
/// — the same trap that moved blob compression out of the ingest lock a few
/// hundred lines up. So: collect addresses under the lock, read and parse
/// with the lock released, and take it again only to apply what matched.
fn apply_rules_to(state: &Arc<AppState>, account: i64, arrivals: &[i64]) {
    // ---- what the rules are, and where the mail is ------------------------
    let (rules, policy, located, gone) = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        let Ok(rules) = store.rules_for_account(account) else {
            return;
        };
        if rules.iter().all(|r| !r.enabled || r.conditions.is_empty()) {
            return;
        }
        // Rules whose folder has since been deleted. Their move is left out
        // of the plan rather than refused when it is made: planned together,
        // a refused last move took the others' move and skip-inbox with it,
        // where each rule on its own had still filed the mail.
        let gone: HashSet<i64> = rules
            .iter()
            .filter(|r| {
                r.actions
                    .move_to
                    .is_some_and(|f| !store.account_owns_folder(account, f).unwrap_or(false))
            })
            .map(|r| r.id)
            .collect();
        let Ok(policy) = store.placement_policy(account) else {
            return;
        };
        let located: Vec<(i64, String, i64)> = arrivals
            .iter()
            .filter_map(|&id| {
                let hash = store.blob_hash_for(id).ok().flatten()?;
                let thread = store.thread_of(id).ok().flatten()?;
                Some((id, hash, thread))
            })
            .collect();
        (rules, policy, located, gone)
    };

    // ---- read, parse and match, with the store free ------------------------
    // Which rules matched each conversation, by their place in `rules`.
    //
    // Per conversation, not per message and rule. Two messages of the same
    // thread arriving in the same pass each matched, and each queued the whole
    // thread's move — telling the server twice to do what it had just been
    // told, and stamping the second action's undo snapshot with the state the
    // first had already changed. And planned as one: two rules, one moving
    // and one skipping the inbox, queued Move then Archive, and the Archive
    // undid the move (`rules::planned_for`).
    let mut matched: Vec<(i64, std::collections::BTreeSet<usize>)> = Vec::new();
    let mut announce: Vec<(String, String)> = Vec::new();
    for (_message_id, hash, thread) in located {
        let Ok(raw) = state.blobs.read(&hash) else {
            continue;
        };
        let Some(parsed) = petrel_mime::parse_message(&raw) else {
            continue;
        };
        // What a condition can see belongs with the matching, not here.
        let envelope = petrel_engine::rules::Envelope::from_message(&parsed, raw.len() as u64);
        let mut announced = false;
        for (index, rule) in rules.iter().enumerate() {
            if !petrel_engine::rules::matches(rule, &envelope) {
                continue;
            }
            let a = &rule.actions;
            if a.notify && !announced {
                // Said through the same announcer ordinary arrivals use:
                // the next status poll carries it out, and the UI applies
                // its own pause and level rules before saying anything.
                // Once per message however many rules ask for it — two
                // rules matching is not two pieces of new mail.
                announced = true;
                let who = parsed
                    .from_display
                    .clone()
                    .filter(|d| !d.is_empty())
                    .or_else(|| parsed.from_addr.clone())
                    .unwrap_or_default();
                announce.push((who, parsed.subject.clone().unwrap_or_default()));
            }
            match matched.iter_mut().find(|(t, _)| *t == thread) {
                Some((_, rules)) => {
                    rules.insert(index);
                }
                None => matched.push((thread, std::iter::once(index).collect())),
            }
        }
    }
    // The order, and the move/skip-inbox interaction, belong with the rules
    // engine where they can be tested without an AppState.
    let mut work: Vec<(i64, ActionKind, Option<i64>, i64)> = Vec::new();
    let mut said_gone: HashSet<i64> = HashSet::new();
    for (thread, indices) in &matched {
        let chosen: Vec<&petrel_engine::rules::Rule> = indices.iter().map(|&i| &rules[i]).collect();
        let actions: Vec<petrel_engine::rules::Actions> = chosen
            .iter()
            .map(|r| {
                let mut a = r.actions.clone();
                if gone.contains(&r.id) {
                    a.move_to = None;
                    // Said out loud, because the rule will go on being wrong
                    // every time it matches until someone edits it. By its
                    // id: the name is the person's own words.
                    if said_gone.insert(r.id) {
                        log_sync(&format!("rule {}: the folder it moves to is gone", r.id));
                    }
                }
                a
            })
            .collect();
        let actions: Vec<&petrel_engine::rules::Actions> = actions.iter().collect();
        for (kind, target, from) in petrel_engine::rules::planned_for(&actions) {
            work.push((*thread, kind, target, chosen[from].id));
        }
    }
    if work.is_empty() && announce.is_empty() {
        return;
    }

    // ---- apply --------------------------------------------------------------
    if !announce.is_empty()
        && let Ok(mut pending) = state.pending_notify.lock()
    {
        pending.extend(announce);
    }
    let mut applied = 0usize;
    {
        let Ok(store) = state.store.lock() else {
            return;
        };
        for (thread, kind, target, rule) in work {
            match store.apply_thread_action(account, thread, kind, target, policy) {
                // A rule naming a folder or tag the user has since deleted is
                // refused by the store rather than half-performed. Said out
                // loud here, because the rule will go on being wrong every
                // time it matches until someone edits it. By its id: the name
                // is the person's own words, as often as not a sender's.
                Err(e) => log_sync(&format!("rule {rule}: {e}")),
                Ok(_) => applied += 1,
            }
        }
    }
    if applied > 0 {
        log_sync(&format!("rules: {applied} action(s) applied on arrival"));
        state.nudge_drain(account);
    }
}

/// Where Gmail keeps every message: the folder holding the archive role,
/// which the survey takes from `\All` — not the English name. Gmail
/// translates it (`[Gmail]/Alle Nachrichten`), and in some countries the
/// prefix is `[Google Mail]`; both sweeps asked for `[Gmail]/All Mail` by
/// name and failed on every cycle there. The All Mail walk already asks
/// this way (backfill.rs). None when the account has none to sweep.
fn gmail_all_mail(state: &AppState, account: i64) -> Option<String> {
    let store = state.store.lock().ok()?;
    let id = store.folder_for_role(account, "archive").ok().flatten()?;
    store.folder_path(id).ok().flatten()
}

/// One incremental Gmail label sweep: where every message lives, which are
/// starred, and — for labels that are Petrel tags — who carries them. With
/// CONDSTORE this costs one round trip when nothing changed, which is why it
/// can run every cycle rather than once at startup: a label applied in
/// Gmail's web UI shows up here within a poll interval.
async fn run_label_sweep(state: &Arc<AppState>, account: i64, cfg: &ImapConfig) {
    let since: Option<u64> = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.settings().ok())
        .and_then(|s| s.get("gmail_labels_modseq").and_then(|v| v.parse().ok()));
    let bound: u32 = std::env::var("PETREL_LABEL_SWEEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000);
    let Some(all_mail) = gmail_all_mail(state, account) else {
        return;
    };
    match petrel_providers::imap::sweep_gmail_labels(cfg, &all_mail, bound, since).await {
        Ok(sweep) => {
            let filed = state
                .store
                .lock()
                .ok()
                .and_then(|s| s.apply_gmail_labels(account, &sweep.labels).ok())
                .unwrap_or(0);
            if !sweep.labels.is_empty() {
                log_sync(&format!(
                    "labels: {} reported, {filed} refiled",
                    sweep.labels.len()
                ));
            }
            if let (Some(m), Ok(store)) = (sweep.modseq, state.store.lock()) {
                let _ = store.set_setting("gmail_labels_modseq", &m.to_string());
            }
            if filed > 0 {
                number_swept_inbox_placements(state, account, cfg).await;
                // A label added or taken away in Gmail is a move here.
                note_mail_moved(state);
            }
        }
        // Not fatal: without it, filing falls back to the folder each
        // message arrived from, which is what it was before.
        Err(e) => log_sync(&format!("label sweep failed: {}", loggable(&e))),
    }
}

/// Numbers the inbox placements the label sweep just made.
///
/// The sweep reads All Mail, so a message it files into the inbox is placed
/// by Message-ID and learns no INBOX number. An unnumbered placement is one
/// the drain has to resolve by searching, and one the folder sweep can never
/// prune — so the inbox is listed as identities and the store is told, which
/// both numbers what is there and drops what is not.
///
/// Only after a sweep that filed something: a full inbox listing is a line
/// per message, cheap beside a body but not worth a round trip every poll
/// when nothing moved.
async fn number_swept_inbox_placements(state: &Arc<AppState>, account: i64, cfg: &ImapConfig) {
    let inbox: Option<(i64, String)> = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        let id = store.folder_for_role(account, "inbox").ok().flatten();
        // Nothing to number means nothing to list: a sweep that refiled
        // messages the inbox already knew by number leaves no work here.
        if let Some(id) = id
            && store.unnumbered_placement_count(id).unwrap_or(0) == 0
        {
            return;
        }
        let path = store
            .folders(account)
            .ok()
            .and_then(|all| all.into_iter().find(|f| Some(f.id) == id).map(|f| f.path));
        id.zip(path)
    };
    let Some((folder_id, path)) = inbox else {
        return;
    };
    // The whole folder, because the store treats the listing as complete and
    // drops what it does not find: a partial one would unfile live mail.
    let listing = match petrel_providers::imap::fetch_id_map_range(cfg, &path, 1, u32::MAX).await {
        Ok(l) => l,
        Err(e) => {
            log_sync(&format!("inbox identity listing failed: {}", loggable(&e)));
            return;
        }
    };
    let Ok(store) = state.store.lock() else {
        return;
    };
    match store.reconcile_unaddressed_placements(folder_id, &listing) {
        Ok(out) if out.rematched > 0 || out.dropped > 0 => log_sync(&format!(
            "labels: {} inbox placement(s) numbered, {} dropped",
            out.rematched, out.dropped
        )),
        Ok(_) => {}
        Err(e) => log_sync(&format!(
            "inbox placement reconcile failed: {}",
            loggable(&e)
        )),
    }
}

/// Gmail's own conversation ids, swept the way labels are.
///
/// JWZ threading works from References headers, and mail that arrives
/// without them threads alone — a Gmail inbox counted ~655 conversations
/// where Gmail's UI said ~271. X-GM-THRID is Gmail's answer; where known it
/// is authoritative, and each sweep regroups whatever it learned.
async fn run_thrid_sweep(state: &Arc<AppState>, account: i64, cfg: &ImapConfig) {
    let since: Option<u64> = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.settings().ok())
        .and_then(|s| s.get("gmail_thrid_modseq").and_then(|v| v.parse().ok()));
    let bound: u32 = std::env::var("PETREL_LABEL_SWEEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000);
    let Some(all_mail) = gmail_all_mail(state, account) else {
        return;
    };
    match petrel_providers::imap::sweep_gmail_thrids(cfg, &all_mail, bound, since).await {
        Ok(sweep) => {
            let (applied, regrouped) = {
                let Ok(store) = state.store.lock() else {
                    return;
                };
                let folder = store.folder_for_role(account, "archive").ok().flatten();
                let applied = folder
                    .and_then(|fid| store.apply_gm_thrids(fid, &sweep.thrids).ok())
                    .unwrap_or(0);
                let regrouped = if applied > 0 {
                    store.regroup_gmail_threads(account).unwrap_or(0)
                } else {
                    0
                };
                (applied, regrouped)
            };
            if applied > 0 {
                log_sync(&format!(
                    "threads: {} reported, {applied} learned, {regrouped} rethreaded",
                    sweep.thrids.len()
                ));
            }
            if regrouped > 0 {
                note_mail_moved(state);
            }
            if let (Some(m), Ok(store)) = (sweep.modseq, state.store.lock()) {
                let _ = store.set_setting("gmail_thrid_modseq", &m.to_string());
            }
        }
        // Not fatal: threading falls back to what References could prove.
        Err(e) => log_sync(&format!("thread sweep failed: {}", loggable(&e))),
    }
}

/// Drops placements the server no longer backs.
///
/// The windowed sync only ever adds and updates: a message moved out of a
/// folder on the server — by our own drain, a rule, or another client — left
/// its old placement behind forever, so the conversation stood in both its
/// folder and the inbox. STATUS is the cheap tell: when a folder's server
/// count falls below the store's UID-bearing placement count, something we
/// hold is gone, and one SEARCH names the survivors. Server counts at or
/// above ours are the fetch's business, not this sweep's — new mail is not a
/// ghost. Equal counts can mask one ghost plus one unfetched arrival; the
/// next arrival or move tips the balance and the sweep catches it then.
async fn reconcile_ghost_placements(
    state: &Arc<AppState>,
    account: i64,
    cfg: &petrel_providers::imap::ImapConfig,
) {
    let candidates: Vec<(i64, String, i64)> = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        let Ok(folders) = store.folders(account) else {
            return;
        };
        folders
            .into_iter()
            .filter_map(|f| {
                let n = store.uid_placement_count(f.id).ok()?;
                (n > 0).then_some((f.id, f.path, n))
            })
            .collect()
    };
    if candidates.is_empty() {
        return;
    }
    let paths: Vec<String> = candidates.iter().map(|c| c.1.clone()).collect();
    let Ok(counts) = petrel_providers::imap::folder_counts(cfg, &paths).await else {
        return;
    };
    for (folder_id, path, local) in candidates {
        let Some((_, server)) = counts.iter().find(|(p, _)| *p == path) else {
            continue;
        };
        if i64::from(*server) == local {
            continue;
        }
        let listed = match petrel_providers::imap::folder_uids(cfg, &path).await {
            Ok(listed) => listed,
            Err(e) => {
                log_sync(&format!(
                    "folder {folder_id}: reconcile sweep failed: {}",
                    loggable(&e)
                ));
                continue;
            }
        };
        // Only under the numbering the store's UIDs were read in. This runs
        // before the cycle that notices a renumbering, and at launch after a
        // recovery that may have failed: comparing the server's new UIDs
        // with stored old ones made every placement whose number was not
        // reused look gone, and they were removed — mail the server still
        // held, hidden and out of search, and back only 200 at a time. A
        // changed numbering is the recovery's to mend; this leaves it alone.
        let held_validity = state
            .store
            .lock()
            .ok()
            .and_then(|s| s.folder_validity(folder_id).ok().flatten());
        if let (Some(held), Some(now)) = (held_validity, listed.uid_validity)
            && held != now
        {
            log_sync(&format!(
                "folder {folder_id}: numbering changed; reconcile left to recovery"
            ));
            continue;
        }
        let present: std::collections::HashSet<u32> = listed.uids.into_iter().collect();
        // Outward: placements the server no longer backs go.
        let removed = state
            .store
            .lock()
            .ok()
            .and_then(|s| s.remove_placements_absent(folder_id, &present).ok())
            .unwrap_or(0);
        if removed > 0 {
            log_sync(&format!(
                "folder {folder_id}: {removed} placement(s) the server no longer holds removed"
            ));
            note_mail_moved(state);
        }
        // Inward: server UIDs the store never placed. The windowed sync can
        // close a watermark over a gap — a draft revision saved by webmail
        // landed between a backfill's endpoint and the forward window and
        // was skipped forever, watermark shut behind it. Only once the
        // backfill has finished its walk: before that, "missing" is most of
        // the folder and belongs to the backfill, not to this sweep.
        let (stored, backfilled) = {
            let Ok(store) = state.store.lock() else {
                continue;
            };
            let stored: std::collections::HashSet<u32> = store
                .placement_uids(folder_id)
                .unwrap_or_default()
                .into_iter()
                .collect();
            (
                stored,
                store.backfill_floor(folder_id).ok().flatten() == Some(1),
            )
        };
        if !backfilled {
            continue;
        }
        let mut missing: Vec<u32> = present.difference(&stored).copied().collect();
        missing.sort_unstable();
        if missing.is_empty() {
            continue;
        }
        // Bounded: a legitimate gap is a handful; thousands means something
        // larger is wrong and one cycle should not fetch a mailbox.
        let overflow = missing.len().saturating_sub(200);
        missing.truncate(200);
        let fetched = petrel_providers::imap::fetch_uids_each(
            cfg,
            &path,
            &missing,
            |uid, flags, raw| {
                // Compressed before the lock, as every other fetch does: one
                // large message compressed inside it stalls the whole app.
                let _ = state.blobs.write(raw);
                // Read outside the lock and inside a fence. A message that
                // panics the parser is skipped by the ordinary fetch, which
                // leaves the folder one short — so this sweep fetched it
                // again every cycle, and parsing it here, unfenced and under
                // the store lock, poisoned the lock: every command failed
                // from then on, at every launch.
                let Ok(mid) = std::panic::catch_unwind(|| {
                    petrel_mime::parse_message(raw).and_then(|p| p.message_id)
                }) else {
                    log_sync(&format!(
                        "folder {folder_id}: uid {uid} PANICKED the parser — message skipped; this is a bug worth reporting"
                    ));
                    return;
                };
                let Ok(mut store) = state.store.lock() else { return };
                // Two copies of one Message-ID, both live on the server right
                // now — this sweep only fetches UIDs the server just named.
                // Dedupe would fold this one into the copy already held and
                // throw its content away; a draft edited apart on the server,
                // or a double-delivered message, is two rows there and stays
                // two rows here.
                if let Some(mid) = mid.as_deref()
                    && let Ok(Some(existing)) = store.message_by_msgid(account, mid)
                    && matches!(store.placement_uid(existing, folder_id), Ok(Some(Some(held))) if held != i64::from(uid))
                {
                    let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        store.ingest_raw_second_copy(&state.blobs, account, Some(folder_id), uid, raw)
                    }));
                    match second {
                        Ok(Ok(ingested)) => {
                            let _ = store.set_message_flags(ingested.message_id, flags);
                            log_sync(&format!(
                                "folder {folder_id}: uid {uid} is a second live copy of a stored message; kept as its own"
                            ));
                        }
                        Ok(Err(e)) => log_sync(&format!("folder {folder_id}: second copy uid {uid} failed: {}", loggable(&e))),
                        Err(_) => log_sync(&format!(
                            "folder {folder_id}: second copy uid {uid} PANICKED — message skipped; this is a bug worth reporting"
                        )),
                    }
                    return;
                }
                let _ = ingest_fenced(&mut store, &state.blobs, account, folder_id, uid, flags, raw);
            },
        )
        .await
        .unwrap_or(0);
        if fetched > 0 {
            note_mail_moved(state);
        }
        if fetched > 0 || overflow > 0 {
            log_sync(&format!(
                "folder {folder_id}: {fetched} message(s) the store was missing fetched{}",
                if overflow > 0 {
                    format!(" ({overflow} more next cycle)")
                } else {
                    String::new()
                }
            ));
        }
    }
}

#[cfg(test)]
mod folder_survey_tests {
    use super::without_tag_labels;

    fn rows(v: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        v.iter()
            .map(|(p, r)| (p.to_string(), r.map(|r| r.to_string())))
            .collect()
    }

    #[test]
    fn a_tag_made_here_does_not_come_back_as_a_folder() {
        // The round trip that motivated this: tag "test" → Gmail label
        // "test" → next survey → a folder named "test", the same thing
        // twice pretending to be two.
        let out = without_tag_labels(
            rows(&[("INBOX", Some("inbox")), ("test", None), ("Unwanted", None)]),
            &["test".to_string()],
            true,
        );
        let paths: Vec<&str> = out.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["INBOX", "Unwanted"]);
    }

    #[test]
    fn role_folders_and_other_providers_keep_shared_names() {
        // A Namecheap folder and a tag sharing a name are two real, distinct
        // things — only on Gmail is one object behind both.
        let out = without_tag_labels(
            rows(&[("Receipts", None)]),
            &["Receipts".to_string()],
            false,
        );
        assert_eq!(out.len(), 1);
        // And a role-bearing folder is never a tag, whatever it is called.
        let out = without_tag_labels(
            rows(&[("Starred", Some("starred"))]),
            &["starred".to_string()],
            true,
        );
        assert_eq!(out.len(), 1);
    }
}

#[cfg(test)]
mod scope_tests {
    use super::{Scope, narrow, open_sync_scope};

    /// The shape `folders_to_sync` returns: (role, path, id), roles first —
    /// as a classic account's survey leaves it, where the plain `Archive`
    /// wears the role and a nested year does not.
    fn targets() -> Vec<(String, String, i64)> {
        [
            ("inbox", "INBOX", 1),
            ("sent", "Sent", 2),
            ("drafts", "Drafts", 6),
            ("spam", "Spam", 7),
            ("trash", "Trash", 3),
            ("starred", "Starred", 8),
            ("archive", "Archive", 4),
            ("", "Contracts", 9),
            ("", "Archive/2026", 5),
        ]
        .into_iter()
        .map(|(r, p, id)| (r.to_string(), p.to_string(), id))
        .collect()
    }

    #[test]
    fn a_wake_takes_the_inbox_and_nothing_else() {
        let out = narrow(targets(), Scope::Inbox);
        let paths: Vec<&str> = out.iter().map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["INBOX"]);
    }

    #[test]
    fn a_sweep_takes_everything() {
        assert_eq!(narrow(targets(), Scope::Everything).len(), 9);
    }

    #[test]
    fn opening_drafts_takes_that_folder_and_nothing_else() {
        let out = narrow(targets(), Scope::Role("drafts"));
        let paths: Vec<&str> = out.iter().map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["Drafts"]);
    }

    #[test]
    fn opening_sent_takes_that_folder_and_nothing_else() {
        let out = narrow(targets(), Scope::Role("sent"));
        let paths: Vec<&str> = out.iter().map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["Sent"]);
    }

    #[test]
    fn opening_a_user_folder_takes_that_id_and_nothing_else() {
        let out = narrow(targets(), Scope::FolderId(9));
        let paths: Vec<&str> = out.iter().map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["Contracts"]);
    }

    #[test]
    fn opening_the_inbox_or_a_folderless_view_does_not_ask_the_server() {
        // Inbox has IDLE; the others have no folder to SELECT.
        assert_eq!(open_sync_scope("inbox"), None);
        assert_eq!(open_sync_scope("outbox"), None);
        assert_eq!(open_sync_scope("snoozed"), None);
        assert_eq!(open_sync_scope("tag:urgent"), None);
    }

    #[test]
    fn opening_archive_takes_the_archive_and_not_the_years_under_it() {
        assert_eq!(open_sync_scope("archive"), Some(Scope::Role("archive")));
        let out = narrow(targets(), Scope::Role("archive"));
        let paths: Vec<&str> = out.iter().map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["Archive"]);
    }

    #[test]
    fn opening_archive_where_it_is_not_synced_narrows_to_nothing() {
        // Gmail: All Mail is not in the list, so there is nothing to fetch.
        let gmail: Vec<(String, String, i64)> = targets()
            .into_iter()
            .filter(|(role, _, _)| role != "archive")
            .collect();
        assert!(narrow(gmail, Scope::Role("archive")).is_empty());
    }

    #[test]
    fn an_opened_mailbox_is_left_alone_within_the_cooldown() {
        use super::{ON_OPEN_COOLDOWN, on_open_due};
        // A point safely after the clock's start, so the subtractions below
        // cannot underflow on a fresh process.
        let now = std::time::Instant::now() + ON_OPEN_COOLDOWN * 2;
        assert!(on_open_due(None, now), "never fetched: due");
        assert!(!on_open_due(Some(now), now), "just fetched: not due");
        assert!(!on_open_due(Some(now - ON_OPEN_COOLDOWN / 2), now));
        assert!(on_open_due(Some(now - ON_OPEN_COOLDOWN), now));
    }

    #[test]
    fn opening_a_listed_mailbox_names_that_scope() {
        assert_eq!(open_sync_scope("sent"), Some(Scope::Role("sent")));
        assert_eq!(open_sync_scope("drafts"), Some(Scope::Role("drafts")));
        assert_eq!(open_sync_scope("spam"), Some(Scope::Role("spam")));
        assert_eq!(open_sync_scope("trash"), Some(Scope::Role("trash")));
        assert_eq!(open_sync_scope("starred"), Some(Scope::Role("starred")));
        assert_eq!(open_sync_scope("folder:9"), Some(Scope::FolderId(9)));
        assert_eq!(open_sync_scope("folder:0"), None);
        assert_eq!(open_sync_scope("folder:nope"), None);
    }

    #[test]
    fn the_inbox_is_found_by_role_not_by_position() {
        // The same folders with the inbox last. A filter that took the first
        // row would sync Sent on every wake and never the inbox — and the
        // symptom would be "no new mail", not an error anybody could chase.
        let mut shuffled = targets();
        shuffled.rotate_left(1);
        let out = narrow(shuffled, Scope::Inbox);
        let paths: Vec<&str> = out.iter().map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["INBOX"]);
    }

    #[test]
    fn an_account_with_no_inbox_role_narrows_to_nothing() {
        // Rather than to everything: a wake that quietly swept all hundred
        // folders would put the cost straight back.
        let none: Vec<(String, String, i64)> = vec![(String::new(), "Archive".to_string(), 4)];
        assert!(narrow(none, Scope::Inbox).is_empty());
    }
}

/// The list itself, from a survey stored the way a real one is. The fixture
/// above gave Archive an empty role, a state the survey never leaves it in,
/// and that is how a classic account's archive went unsynced unnoticed.
#[cfg(test)]
mod sync_list_tests {
    use super::folders_to_sync_from;
    use petrel_engine::store::Store;

    fn surveyed(survey: &[(&str, Option<&str>)]) -> (Store, i64) {
        let mut store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let folders: Vec<(String, Option<String>)> = survey
            .iter()
            .map(|(path, role)| ((*path).to_string(), role.map(String::from)))
            .collect();
        store.sync_folders(account, &folders).unwrap();
        (store, account)
    }

    fn listed(store: &Store, account: i64) -> Vec<(String, String)> {
        folders_to_sync_from(store, account)
            .into_iter()
            .map(|(role, path, _)| (role, path))
            .collect()
    }

    fn pair(role: &str, path: &str) -> (String, String) {
        (role.to_string(), path.to_string())
    }

    #[test]
    fn a_plain_archive_on_a_classic_account_is_synced() {
        // Namecheap: no \Archive flag; the survey adopts the plain folder.
        let (store, a) = surveyed(&[("INBOX", Some("inbox")), ("Archive", None)]);
        assert!(listed(&store, a).contains(&pair("archive", "Archive")));
    }

    #[test]
    fn a_special_use_archive_is_synced() {
        // iCloud, Fastmail, Dovecot: the server flags it \Archive.
        let (store, a) = surveyed(&[("INBOX", Some("inbox")), ("Archive", Some("archive"))]);
        assert!(listed(&store, a).contains(&pair("archive", "Archive")));
    }

    #[test]
    fn gmails_all_mail_is_not_synced() {
        let (store, a) = surveyed(&[
            ("INBOX", Some("inbox")),
            ("[Gmail]/All Mail", Some("archive")),
        ]);
        store.set_account_kind(a, "gmail").unwrap();
        let list = listed(&store, a);
        assert!(
            list.iter().all(|(_, path)| path != "[Gmail]/All Mail"),
            "All Mail holds every message; syncing it doubles the store: {list:?}"
        );
    }

    #[test]
    fn the_archive_follows_the_roles_and_precedes_the_persons_folders() {
        let (store, a) = surveyed(&[
            ("INBOX", Some("inbox")),
            ("Sent", Some("sent")),
            ("Drafts", Some("drafts")),
            ("Junk", None),
            ("Trash", None),
            ("Archive", None),
            ("Archive/2026", None),
            ("Contracts", None),
        ]);
        assert_eq!(
            listed(&store, a),
            vec![
                pair("inbox", "INBOX"),
                pair("sent", "Sent"),
                pair("drafts", "Drafts"),
                pair("spam", "Junk"),
                pair("trash", "Trash"),
                pair("archive", "Archive"),
                // A year under it is a place, not the mailbox, and syncs as
                // one of the person's folders — once.
                pair("", "Archive/2026"),
                pair("", "Contracts"),
            ]
        );
    }

    #[test]
    fn a_local_archive_is_not_asked_about() {
        let (mut store, a) = surveyed(&[("INBOX", Some("inbox")), ("Archive", Some("archive"))]);
        let id = store.folder_for_role(a, "archive").unwrap().unwrap();
        store.mark_folder_local(id).unwrap();
        assert!(
            listed(&store, a).iter().all(|(role, _)| role != "archive"),
            "the server has never heard of a local folder"
        );
    }

    #[test]
    fn a_mailbox_flagged_all_is_never_synced_as_the_archive() {
        // Dovecot's stock set plus its documented `virtual/All`, and a plain
        // Archive. The survey hands the role to `virtual/All`; syncing that
        // would download the whole account again and list it all as Archive.
        let (store, a) = surveyed(&[
            ("INBOX", Some("inbox")),
            ("Sent", Some("sent")),
            ("virtual/All", Some("archive")),
            ("Archive", None),
        ]);
        store
            .set_all_mail_folders(a, &["virtual/All".to_string()])
            .unwrap();
        let list = listed(&store, a);
        assert!(
            list.iter().all(|(_, path)| path != "virtual/All"),
            "{list:?}"
        );
        // Everything else as it was: the plain Archive syncs as a folder.
        assert!(list.contains(&pair("", "Archive")), "{list:?}");
    }

    #[test]
    fn the_all_mark_follows_the_survey_and_leaves_other_marks_alone() {
        let (mut store, a) = surveyed(&[("INBOX", Some("inbox")), ("All", Some("archive"))]);
        let id = store.folder_for_role(a, "archive").unwrap().unwrap();
        store.mark_folder_local(id).unwrap();
        store.set_all_mail_folders(a, &["All".to_string()]).unwrap();
        assert!(store.folder_is_all_mail(id).unwrap());
        assert!(
            store.folder_is_local(id).unwrap(),
            "the local mark survives"
        );
        // A survey that no longer flags it takes the mark away.
        store.set_all_mail_folders(a, &[]).unwrap();
        assert!(!store.folder_is_all_mail(id).unwrap());
        assert!(store.folder_is_local(id).unwrap());
    }

    #[test]
    fn the_survey_names_the_all_mailboxes_everywhere_but_gmail() {
        use super::all_mail_paths;
        use petrel_providers::imap::FolderInfo;
        let folder = |name: &str, attrs: &[&str]| FolderInfo {
            name: name.into(),
            delimiter: Some("/".into()),
            attributes: attrs.iter().map(|s| s.to_string()).collect(),
        };
        let found = [
            folder("INBOX", &[]),
            folder("Archive", &["\\Archive"]),
            folder("virtual/All", &["\\All"]),
            folder("virtual", &["\\Noselect"]),
        ];
        assert_eq!(
            all_mail_paths(&found, false),
            vec!["virtual/All".to_string()]
        );
        // Gmail's All Mail is the archive, and is walked as one.
        let gmail = [folder("[Gmail]/All Mail", &["\\All"])];
        assert!(all_mail_paths(&gmail, true).is_empty());
    }

    #[test]
    fn a_server_flagging_several_folders_per_role_is_synced_one_per_role() {
        // cPanel's Dovecot once Apple Mail and Outlook have both been
        // pointed at it. The sync asked for the empty Apple folders, first
        // in the rail, while triage filed into the real ones.
        let (store, a) = surveyed(&[
            ("INBOX", Some("inbox")),
            ("Deleted Messages", Some("trash")),
            ("Trash", Some("trash")),
            ("Junk", Some("spam")),
            ("Spam", Some("spam")),
            ("Sent Messages", Some("sent")),
            ("Sent", Some("sent")),
            ("Sent Items", Some("sent")),
        ]);
        let list = folders_to_sync_from(&store, a);
        for (role, path) in [("trash", "Trash"), ("spam", "Spam"), ("sent", "Sent")] {
            let synced: Vec<(String, i64)> = list
                .iter()
                .filter(|(r, _, _)| r == role)
                .map(|(_, p, id)| (p.clone(), *id))
                .collect();
            let filed = store.folder_for_role(a, role).unwrap().unwrap();
            assert_eq!(synced, vec![(path.to_string(), filed)], "{role}: {list:?}");
        }
        // The others sync too, as folders: their mail is still the person's.
        for path in ["Deleted Messages", "Junk", "Sent Messages", "Sent Items"] {
            assert!(
                list.iter().any(|(r, p, _)| r.is_empty() && p == path),
                "{path}: {list:?}"
            );
        }
    }

    #[test]
    fn a_mailbox_flagged_all_is_not_synced_as_a_folder_either() {
        // Once it stops holding the archive role it is role-less, and the
        // folder loop must not pick it up instead.
        let survey = [
            ("INBOX", Some("inbox")),
            ("virtual/All", Some("archive")),
            ("Archive", None),
        ];
        let (mut store, a) = surveyed(&survey);
        store
            .set_all_mail_folders(a, &["virtual/All".to_string()])
            .unwrap();
        let rows: Vec<(String, Option<String>)> = survey
            .iter()
            .map(|(p, r)| (p.to_string(), r.map(String::from)))
            .collect();
        store.sync_folders(a, &rows).unwrap();
        let list = listed(&store, a);
        assert!(list.contains(&pair("archive", "Archive")), "{list:?}");
        assert!(
            list.iter().all(|(_, path)| path != "virtual/All"),
            "{list:?}"
        );
    }

    #[test]
    fn a_survey_gives_all_mail_no_role_outside_gmail() {
        use super::survey_rows;
        use petrel_providers::imap::FolderInfo;
        let folder = |name: &str, attrs: &[&str]| FolderInfo {
            name: name.into(),
            delimiter: Some("/".into()),
            attributes: attrs.iter().map(|s| s.to_string()).collect(),
        };
        let found = [
            folder("INBOX", &[]),
            folder("Archive", &["\\Archive"]),
            folder("virtual/All", &["\\All"]),
            folder("virtual", &["\\Noselect"]),
        ];
        assert_eq!(
            survey_rows(&found, false),
            vec![
                ("INBOX".to_string(), Some("inbox".to_string())),
                ("Archive".to_string(), Some("archive".to_string())),
                ("virtual/All".to_string(), None),
            ]
        );
        // On Gmail, All Mail is the archive.
        let gmail = [folder("[Gmail]/All Mail", &["\\All"])];
        assert_eq!(
            survey_rows(&gmail, true),
            vec![("[Gmail]/All Mail".to_string(), Some("archive".to_string()))]
        );
    }

    #[test]
    fn the_archive_synced_is_the_one_the_drain_files_into() {
        // Two folders wearing the role, as a server flagging both \Archive
        // and \All would leave them. One is synced: the one mail goes to.
        let (store, a) = surveyed(&[
            ("INBOX", Some("inbox")),
            ("Archive", Some("archive")),
            ("All", Some("archive")),
        ]);
        let target = store.folder_for_role(a, "archive").unwrap().unwrap();
        let archives: Vec<i64> = folders_to_sync_from(&store, a)
            .into_iter()
            .filter(|(role, _, _)| role == "archive")
            .map(|(_, _, id)| id)
            .collect();
        assert_eq!(archives, vec![target]);
    }
}

#[cfg(test)]
mod departure_tests {
    use super::{settle_departures, watch_target};
    use petrel_providers::imap::Survivors;

    fn raw(n: u32) -> Vec<u8> {
        format!(
            "From: a@example.com\r\nTo: b@example.com\r\nSubject: m{n}\r\n\
             Message-ID: <m{n}@x>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain\r\n\r\nbody {n}\r\n"
        )
        .into_bytes()
    }

    /// A state holding INBOX, Sent and one folder of the person's own,
    /// returning that folder's id.
    fn with_folders(state: &crate::state::AppState) -> i64 {
        let account = state.account_id;
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Sent".into(), Some("sent".into())),
                    ("Formation".into(), None),
                ],
            )
            .unwrap();
        store
            .folders(account)
            .unwrap()
            .into_iter()
            .find(|f| f.path == "Formation")
            .unwrap()
            .id
    }

    #[test]
    fn the_watch_follows_folders_the_server_has_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        let own = with_folders(&state);
        let account = state.account_id;
        let at = |view: &str| watch_target(&state, account, Some(&(account, view.to_string())));

        assert_eq!(at("sent").as_deref(), Some("Sent"));
        assert_eq!(at(&format!("folder:{own}")).as_deref(), Some("Formation"));
        // The inbox has its own watch; these have no folder to IDLE on.
        assert_eq!(at("inbox"), None);
        assert_eq!(at("tag:work"), None);
        assert_eq!(at("snoozed"), None);
        // Nothing said yet, or another account's view: no connection here.
        assert_eq!(watch_target(&state, account, None), None);
        assert_eq!(
            watch_target(&state, account, Some(&(account + 1, "sent".into()))),
            None
        );
    }

    #[test]
    fn what_the_search_did_not_name_goes_and_what_came_after_it_stays() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        let own = with_folders(&state);
        {
            let mut store = state.store.lock().unwrap();
            for uid in [1, 2, 3, 9] {
                store
                    .ingest_raw(
                        &state.blobs,
                        state.account_id,
                        Some(own),
                        Some(uid),
                        &raw(uid),
                    )
                    .unwrap();
            }
        }

        // Another client took 2 away. 9 was placed by the drain after the
        // search, above its bound, and must not be read as gone.
        let survivors = Survivors {
            uids: vec![1, 3],
            below: 5,
        };
        let removed = settle_departures(&state, own, Some((2, 5)), Some(&survivors));
        assert_eq!(removed, 1);
        let mut held = state.store.lock().unwrap().placement_uids(own).unwrap();
        held.sort_unstable();
        assert_eq!(held, vec![1, 3, 9]);
        assert_eq!(state.folder_seen.lock().unwrap().get(&own), Some(&(2, 5)));

        // A pass whose search was needed and did not happen reports no
        // baseline: the old one stays, so the next pass asks again.
        assert_eq!(settle_departures(&state, own, None, None), 0);
        assert_eq!(state.folder_seen.lock().unwrap().get(&own), Some(&(2, 5)));
    }
}

#[cfg(test)]
mod elsewhere_tests {
    use super::announce_elsewhere;
    use crate::state::Elsewhere;

    fn raw(from: &str, subject: &str, id: &str) -> Vec<u8> {
        format!(
            "From: {from}\r\nTo: me@example.com\r\nSubject: {subject}\r\n\
             Date: Mon, 7 Sep 2026 09:00:00 +0000\r\nMessage-ID: <{id}@example.com>\r\n\
             Content-Type: text/plain\r\n\r\nbody\r\n"
        )
        .into_bytes()
    }

    /// Mail arriving in an account that is not on screen. The window watches
    /// the inbox of the account it shows and heard nothing of the others';
    /// other clients announce every account's new mail. What the rules filed
    /// elsewhere, and what was already read, is not news.
    #[test]
    fn new_mail_in_another_accounts_inbox_is_queued_to_be_said() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        let shown = state.account_id;
        let (other, arrivals) = {
            let store = state.store().unwrap();
            let other = store.ensure_test_account().unwrap();
            store.set_active_account(shown).unwrap();
            let inbox = store.ensure_folder(other, "inbox", "INBOX").unwrap();
            let receipts = store.ensure_named_folder(other, "Receipts").unwrap();
            drop(store);
            let mut store = state.store().unwrap();
            let mut ids = Vec::new();
            for (folder, uid, from, subject, read) in [
                (
                    inbox,
                    1,
                    "Riley Chen <riley@example.com>",
                    "Lunch on Friday",
                    false,
                ),
                (inbox, 2, "dana@example.com", "Read already", true),
                (
                    receipts,
                    3,
                    "Shop <shop@example.com>",
                    "Filed by a rule",
                    false,
                ),
            ] {
                let got = store
                    .ingest_raw(
                        &state.blobs,
                        other,
                        Some(folder),
                        Some(uid),
                        &raw(from, subject, &uid.to_string()),
                    )
                    .unwrap();
                if read {
                    store
                        .set_message_flags(got.message_id, petrel_engine::store::flags::SEEN)
                        .unwrap();
                }
                ids.push(got.message_id);
            }
            (other, ids)
        };

        announce_elsewhere(&state, other, &arrivals);
        let said: Vec<Elsewhere> = std::mem::take(&mut *state.pending_elsewhere.lock().unwrap());
        assert_eq!(
            said,
            vec![Elsewhere {
                account: other,
                who: "Riley Chen".into(),
                subject: "Lunch on Friday".into(),
            }]
        );

        // The account on screen says its own: its inbox list is watched.
        state.store().unwrap().set_active_account(other).unwrap();
        announce_elsewhere(&state, other, &arrivals);
        assert!(state.pending_elsewhere.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod loggable_tests {
    use super::loggable;

    /// Review r1, R1-3. A server's refusal quotes what it refused, and that
    /// is a folder's name: Dovecot's "Mailbox doesn't exist: <name>", Gmail's
    /// "Unknown Mailbox: <name>". A real account logged one such line 112
    /// times (`drain::refusal_tests`). A NO or a BAD is logged by its kind
    /// and its code, in every shape a refusal reaches the log in.
    #[test]
    fn a_refusal_is_logged_by_its_kind_and_code() {
        for (error, logged) in [
            // async-imap's, with the code it parsed.
            (
                "imap: no response: code: Some(TryCreate), info: Some(\"Mailbox doesn't exist: Larkspur Holdings (0.001 + 0.000 secs).\")",
                "imap: NO [TRYCREATE]",
            ),
            // A code it does not parse stays at the head of the text.
            (
                "imap: no response: code: None, info: Some(\"[NONEXISTENT] Unknown Mailbox: Larkspur Holdings (Failure)\")",
                "imap: NO [NONEXISTENT]",
            ),
            // A failed folder's detail, as the sync pass words it.
            (
                "no response: code: None, info: Some(\"Mailbox doesn't exist: Larkspur Holdings\")",
                "NO",
            ),
            (
                "imap: bad response: code: None, info: Some(\"Error in IMAP command CREATE: Invalid mailbox name: Larkspur/\")",
                "imap: BAD",
            ),
            (
                "protocol: created, but the server refused to subscribe: no response: code: None, info: Some(\"Larkspur Holdings\")",
                "protocol: created, but the server refused to subscribe: NO",
            ),
            // Petrel's own exchanges: the tagged reply as the server sent
            // it, and its status by name.
            (
                "protocol: EXAMINE: NO [NONEXISTENT] Mailbox doesn't exist: Larkspur Holdings",
                "protocol: EXAMINE: NO [NONEXISTENT]",
            ),
            (
                "protocol: UID SEARCH: No Mailbox doesn't exist: Larkspur Holdings",
                "protocol: UID SEARCH: NO",
            ),
            (
                "protocol: UID: Bad Error in IMAP command: Larkspur Holdings",
                "protocol: UID: BAD",
            ),
        ] {
            assert_eq!(loggable(&error), logged, "{error}");
        }
    }

    /// What is not a refusal says nothing of the person's mail, and is kept
    /// whole: "No route to host" is not a NO.
    #[test]
    fn anything_else_is_logged_as_it_stands() {
        for error in [
            "network: No route to host (os error 65)",
            "the folder was renumbered (UIDVALIDITY 1 is now 2); nothing was done",
            "protocol: there is no Message-ID to search for",
            "protocol: connection closed mid-command",
            "tls: the TLS handshake did not finish within 30s",
        ] {
            assert_eq!(loggable(&error), error);
        }
        assert_eq!(
            loggable(&"imap: parse: Error { input: [42, 32], code: TakeWhile1 } during parsing"),
            "imap-parse"
        );
    }
}

#[cfg(test)]
mod rules_on_arrival_tests {
    use super::apply_rules_to;
    use petrel_engine::rules::{Actions, Condition, Op};

    fn condition(field: &str, value: &str) -> Condition {
        Condition {
            field: field.into(),
            header: None,
            op: Op::Contains,
            value: value.into(),
        }
    }

    /// One rule moves the vendor's mail to Vendors, another skips the inbox
    /// for anything about an invoice. Planned rule by rule, the Archive the
    /// second queued after the first's Move took the mail out of Vendors, and
    /// it ended in Archive with nothing to say why.
    #[test]
    fn a_move_and_another_rules_skip_inbox_file_the_mail_where_it_was_moved() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        let account = state.account_id;
        let (id, vendors) = {
            let mut store = state.store.lock().unwrap();
            store
                .sync_folders(
                    account,
                    &[
                        ("INBOX".into(), Some("inbox".into())),
                        ("Archive".into(), Some("archive".into())),
                        ("Vendors".into(), None),
                    ],
                )
                .unwrap();
            let inbox = store.folder_for_role(account, "inbox").unwrap().unwrap();
            let vendors = store.ensure_named_folder(account, "Vendors").unwrap();
            let raw = b"From: Dana <dana@vendor.example>\r\nTo: me@example.com\r\n\
                Subject: Invoice 12 for September\r\nMessage-ID: <inv12@vendor.example>\r\n\
                Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\r\nattached\r\n";
            let id = store
                .ingest_raw(&state.blobs, account, Some(inbox), Some(1), raw)
                .unwrap()
                .message_id;
            store
                .save_rule(
                    account,
                    None,
                    "Vendor mail",
                    true,
                    &[condition("from", "vendor.example")],
                    &Actions {
                        move_to: Some(vendors),
                        ..Actions::default()
                    },
                )
                .unwrap();
            store
                .save_rule(
                    account,
                    None,
                    "Invoices",
                    true,
                    &[condition("subject", "invoice")],
                    &Actions {
                        skip_inbox: true,
                        ..Actions::default()
                    },
                )
                .unwrap();
            (id, vendors)
        };

        apply_rules_to(&state, account, &[id]);

        let store = state.store.lock().unwrap();
        assert_eq!(store.folders_of(id).unwrap(), vec![vendors]);
        assert_eq!(
            store.pending_actions(account).unwrap().len(),
            1,
            "one move, and no archive after it"
        );
    }

    /// A later rule whose folder has since been deleted does not cancel the
    /// earlier rule's move: planned together, its refused move was the one
    /// chosen, and the mail went nowhere.
    #[test]
    fn a_rule_whose_folder_is_gone_does_not_cancel_another_rules_move() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        let account = state.account_id;
        let (id, vendors) = {
            let mut store = state.store.lock().unwrap();
            store
                .sync_folders(account, &[("INBOX".into(), Some("inbox".into()))])
                .unwrap();
            let inbox = store.folder_for_role(account, "inbox").unwrap().unwrap();
            let vendors = store.ensure_named_folder(account, "Vendors").unwrap();
            let doomed = store.ensure_named_folder(account, "Old invoices").unwrap();
            let raw = b"From: Dana <dana@vendor.example>\r\nTo: me@example.com\r\n\
                Subject: Invoice 13 for October\r\nMessage-ID: <inv13@vendor.example>\r\n\
                Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\r\nattached\r\n";
            let id = store
                .ingest_raw(&state.blobs, account, Some(inbox), Some(1), raw)
                .unwrap()
                .message_id;
            for (name, field, value, folder) in [
                ("Vendor mail", "from", "vendor.example", vendors),
                ("Invoices", "subject", "invoice", doomed),
            ] {
                store
                    .save_rule(
                        account,
                        None,
                        name,
                        true,
                        &[condition(field, value)],
                        &Actions {
                            move_to: Some(folder),
                            ..Actions::default()
                        },
                    )
                    .unwrap();
            }
            store.remove_folder(doomed).unwrap();
            (id, vendors)
        };

        apply_rules_to(&state, account, &[id]);

        let store = state.store.lock().unwrap();
        assert_eq!(store.folders_of(id).unwrap(), vec![vendors]);
    }
}
