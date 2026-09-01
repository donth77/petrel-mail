//! Calendar invitations: the card's data, and the answer sent back.

use crate::diag::{data_dir, log_sync};
use crate::state::active_account;
use crate::state::{AppState, note_ui_touch, now_ms};
use petrel_engine::store::DraftEnvelope;
use petrel_mime::ical::{IcalTime, Invitation};
use std::sync::Arc;
use tauri::State;

/// When an event happens, in a form the renderer can display without
/// guessing: a UTC instant it can localise, a wall-clock time it can only
/// show beside its zone's name, or a bare all-day date.
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimeView {
    Utc { ms: i64 },
    Local { raw: String, tzid: Option<String> },
    Date { date: String },
}

#[derive(serde::Serialize)]
pub struct AttendeeView {
    pub name: Option<String>,
    pub email: Option<String>,
    pub partstat: Option<String>,
}

/// Everything the invitation card shows.
#[derive(serde::Serialize)]
pub struct InvitationView {
    /// REQUEST, CANCEL, REPLY — absent for a plain event attachment.
    pub method: Option<String>,
    pub summary: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub organizer_name: Option<String>,
    pub organizer_email: Option<String>,
    pub attendees: Vec<AttendeeView>,
    pub start: Option<TimeView>,
    pub end: Option<TimeView>,
    pub recurring: bool,
    pub status: Option<String>,
    /// This account's own PARTSTAT among the attendees, when listed.
    pub my_partstat: Option<String>,
    /// Whether the buttons make sense: a REQUEST, with an organizer to
    /// answer, addressed to this account.
    pub can_respond: bool,
    /// What was already answered from here: accepted, tentative, declined.
    pub responded: Option<String>,
}

fn time_view(t: &IcalTime) -> TimeView {
    match t {
        IcalTime::Utc(ms) => TimeView::Utc { ms: *ms },
        IcalTime::Local { raw, tzid } => TimeView::Local {
            raw: raw.clone(),
            tzid: tzid.clone(),
        },
        IcalTime::Date(d) => TimeView::Date { date: d.clone() },
    }
}

fn load_invitation(state: &Arc<AppState>, message_id: i64) -> Result<Invitation, String> {
    let hash = {
        let store = state.store()?;
        store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message has no stored body")?
    };
    let raw = state.blobs.read(&hash).map_err(|e| e.to_string())?;
    petrel_mime::ical::invitation_in(&raw).ok_or_else(|| "no calendar part".into())
}

/// The invitation a message carries, if any.
#[tauri::command]
pub fn invitation(
    message_id: i64,
    state: State<Arc<AppState>>,
) -> Result<Option<InvitationView>, String> {
    note_ui_touch(&state);
    let Ok(inv) = load_invitation(&state, message_id) else {
        return Ok(None);
    };
    let (my_email, responded) = {
        let store = state.store()?;
        let account = active_account(&store)?;
        let email = store
            .accounts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|a| a.id == account)
            .map(|a| a.email);
        let responded = store
            .thread_detail(-message_id)
            .ok()
            .and_then(|msgs| msgs.into_iter().find(|m| m.id == message_id))
            .and_then(|m| m.invite_response);
        (email, responded)
    };
    let mine = |addr: &Option<String>| matches!((addr, &my_email), (Some(a), Some(me)) if a.eq_ignore_ascii_case(me));
    let my_partstat = inv
        .attendees
        .iter()
        .find(|a| mine(&a.email))
        .and_then(|a| a.partstat.clone());
    let is_attendee = inv.attendees.iter().any(|a| mine(&a.email));
    let can_respond = inv.method.as_deref() == Some("REQUEST")
        && inv.organizer.as_ref().is_some_and(|o| o.email.is_some())
        && is_attendee;
    Ok(Some(InvitationView {
        method: inv.method.clone(),
        summary: inv.summary.clone(),
        location: inv.location.clone(),
        description: inv.description.clone(),
        organizer_name: inv.organizer.as_ref().and_then(|o| o.name.clone()),
        organizer_email: inv.organizer.as_ref().and_then(|o| o.email.clone()),
        attendees: inv
            .attendees
            .iter()
            .map(|a| AttendeeView {
                name: a.name.clone(),
                email: a.email.clone(),
                partstat: a.partstat.clone(),
            })
            .collect(),
        start: inv.start.as_ref().map(time_view),
        end: inv.end.as_ref().map(time_view),
        recurring: inv.recurring,
        status: inv.status.clone(),
        my_partstat,
        can_respond,
        responded,
    }))
}

/// Answers an invitation: ACCEPTED, TENTATIVE or DECLINED, as a
/// METHOD:REPLY sent to the organizer through the outbox like any mail.
#[tauri::command]
pub fn respond_invitation(
    message_id: i64,
    response: String,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    note_ui_touch(&state);
    let (partstat, verb) = match response.as_str() {
        "accepted" => ("ACCEPTED", "Accepted"),
        "tentative" => ("TENTATIVE", "Tentatively accepted"),
        "declined" => ("DECLINED", "Declined"),
        other => return Err(format!("not a response: {other}")),
    };
    let inv = load_invitation(&state, message_id)?;
    if inv.method.as_deref() != Some("REQUEST") {
        return Err("this event asks for no answer".into());
    }
    let organizer = inv
        .organizer
        .as_ref()
        .and_then(|o| o.email.clone())
        .ok_or("the invitation names no organizer")?;
    let uid = inv.uid.clone().ok_or("the invitation carries no UID")?;

    let (account, me, my_name, in_reply_to) = {
        let store = state.store()?;
        let account = active_account(&store)?;
        let summary = store
            .accounts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|a| a.id == account)
            .ok_or("no active account")?;
        let name = store
            .identity(account)
            .map(|i| i.display_name)
            .unwrap_or_default();
        let irt = store.msgid_header_of(message_id).ok().flatten();
        (account, summary.email, name, irt)
    };

    let ics = build_reply_ics(&inv, &uid, &organizer, &me, &my_name, partstat);
    let dir = data_dir().join("staged");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!(
        "{}-invite-reply.ics",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, ics.as_bytes()).map_err(|e| e.to_string())?;

    let event = inv.summary.as_deref().unwrap_or("the event");
    let subject = format!("{verb}: {event}");
    let draft_id = {
        let store = state.store()?;
        let envelope = DraftEnvelope {
            in_reply_to: in_reply_to.clone(),
            references: in_reply_to.into_iter().collect(),
            attachments: vec![path.to_string_lossy().into_owned()],
        };
        let id = store
            .save_draft_full(
                account, None, &organizer, "", &subject, &subject, "", &envelope,
            )
            .map_err(|e| e.to_string())?;
        store
            .schedule_send(id, reply_send_at(now_ms()))
            .map_err(|e| e.to_string())?;
        store
            .set_invite_response(message_id, &response)
            .map_err(|e| e.to_string())?;
        id
    };
    state.send_signal.notify_one();
    log_sync(&format!(
        "invitation {message_id} answered {partstat}; reply queued as draft {draft_id}"
    ));
    Ok(())
}

/// When an invitation reply becomes due.
///
/// `Some`, never `None`, and that is the whole point of it having a name.
/// `schedule_send(_, None)` *clears* a schedule — it is the call the outbox
/// uses to pull a message back — and `due_sends` only returns rows whose
/// `send_after_ms IS NOT NULL`. Answering an invitation with `None` therefore
/// wrote a perfectly good reply into Drafts and left it there, while the
/// organizer waited for an answer the release notes promised had been sent.
/// That is what this did until 2026-08-28.
///
/// Due immediately rather than after an undo window: the reply is a
/// consequence of pressing Accept, not a message being composed, and there is
/// no undo affordance on an invitation to hang a countdown from.
fn reply_send_at(now_ms: i64) -> Option<i64> {
    Some(now_ms)
}

/// The METHOD:REPLY calendar the organizer's system ingests.
fn build_reply_ics(
    inv: &Invitation,
    uid: &str,
    organizer: &str,
    me: &str,
    my_name: &str,
    partstat: &str,
) -> String {
    let mut lines: Vec<String> = vec![
        "BEGIN:VCALENDAR".into(),
        "PRODID:-//Petrel//Petrel Mail//EN".into(),
        "VERSION:2.0".into(),
        "METHOD:REPLY".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{uid}"),
        format!("SEQUENCE:{}", inv.sequence),
        format!("DTSTAMP:{}", utc_stamp_now()),
        format!("ORGANIZER:mailto:{organizer}"),
    ];
    if my_name.trim().is_empty() {
        lines.push(format!("ATTENDEE;PARTSTAT={partstat}:mailto:{me}"));
    } else {
        lines.push(format!(
            "ATTENDEE;PARTSTAT={partstat};CN={}:mailto:{me}",
            escape_text(my_name)
        ));
    }
    // The times restated when the invitation fixed them as instants —
    // some processors match on them as well as the UID.
    if let Some(IcalTime::Utc(ms)) = inv.start {
        lines.push(format!("DTSTART:{}", utc_stamp(ms)));
    }
    if let Some(IcalTime::Utc(ms)) = inv.end {
        lines.push(format!("DTEND:{}", utc_stamp(ms)));
    }
    if let Some(s) = &inv.summary {
        lines.push(format!("SUMMARY:{}", escape_text(s)));
    }
    lines.push("END:VEVENT".into());
    lines.push("END:VCALENDAR".into());
    lines
        .into_iter()
        .map(|l| fold_line(&l))
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n"
}

/// `, ; \ and newlines` escaped the way RFC 5545 §3.3.11 spells.
fn escape_text(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            other => out.push(other),
        }
    }
    out
}

/// Content lines fold at 75 octets with a space continuation (§3.1).
fn fold_line(line: &str) -> String {
    if line.len() <= 74 {
        return line.to_string();
    }
    let mut out = String::new();
    let mut count = 0usize;
    for c in line.chars() {
        let w = c.len_utf8();
        if count + w > 74 {
            out.push_str("\r\n ");
            count = 1; // the leading space
        }
        out.push(c);
        count += w;
    }
    out
}

fn utc_stamp_now() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    utc_stamp(ms)
}

/// Epoch milliseconds → `YYYYMMDDTHHMMSSZ`, civil-from-days.
fn utc_stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        y,
        m,
        d,
        sod / 3_600,
        (sod % 3_600) / 60,
        sod % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_round_trip_the_parser() {
        // 2026-02-22 23:30:00 UTC — the instant the parser test pins.
        assert_eq!(utc_stamp(1_771_803_000_000), "20260222T233000Z");
    }

    /// The bug this pins: a reply that is written, queued, and never sent.
    ///
    /// Against a real store rather than by inspecting the Option, because the
    /// failure was never in the value — it was in what `due_sends` does with
    /// it. `schedule_send(_, None)` leaves `send_after_ms` NULL and the outbox
    /// only picks up rows where it IS NOT NULL, so the old code's reply sat in
    /// Drafts forever. Swap `reply_send_at` back to `None` and this fails.
    #[test]
    fn an_answered_invitation_is_actually_due_to_send() {
        use petrel_engine::store::{AccountServers, Store};

        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("p.db")).expect("store");
        let account = store
            .add_account("imap", "you@example.com", "You", &AccountServers::default())
            .expect("account");
        let draft = store
            .save_draft(
                account,
                None,
                "organizer@example.com",
                "Accepted: Standup",
                "",
                "",
            )
            .expect("draft");

        let now = 1_771_803_000_000;
        store
            .schedule_send(draft, reply_send_at(now))
            .expect("schedule");

        let due = store.due_sends(account, now).expect("due");
        assert!(
            due.iter().any(|d| d.id == draft),
            "the reply must be due to send; queued with {:?} it was not",
            reply_send_at(now)
        );
    }

    #[test]
    fn long_lines_fold_and_text_escapes() {
        let long = format!("SUMMARY:{}", "x".repeat(200));
        let folded = fold_line(&long);
        assert!(folded.lines().all(|l| l.len() <= 75));
        assert!(folded.contains("\r\n "));
        assert_eq!(escape_text("a,b;c\nd"), "a\\,b\\;c\\nd");
    }
}
