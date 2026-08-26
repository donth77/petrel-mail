//! The invitation parser against the two shapes real invites take:
//! Google-style METHOD:REQUEST, and the method-less event attachment a
//! booking tool sends.

use petrel_mime::ical::{IcalTime, invitation_in, parse_invitation};

fn google_style() -> String {
    // Folded ATTENDEE lines, quoted CN with a comma, METHOD at calendar level.
    [
        "BEGIN:VCALENDAR",
        "PRODID:-//Test//Test//EN",
        "VERSION:2.0",
        "METHOD:REQUEST",
        "BEGIN:VEVENT",
        "DTSTART:20260222T233000Z",
        "DTEND:20260223T000000Z",
        "DTSTAMP:20260210T101112Z",
        "ORGANIZER;CN=\"Wu, Dana\":mailto:dana@example.com",
        "UID:abc123@example.com",
        "ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=",
        " TRUE;CN=Sam Ortiz;X-NUM-GUESTS=0:mailto:sam@example.net",
        "DESCRIPTION:Bring\\, please:\\n- the draft\\n- the numbers",
        "LOCATION:",
        "SEQUENCE:2",
        "STATUS:CONFIRMED",
        "SUMMARY:Planning 1:1",
        "END:VEVENT",
        "END:VCALENDAR",
    ]
    .join("\r\n")
}

#[test]
fn a_google_request_reads_completely() {
    let inv = parse_invitation(&google_style()).expect("parsed");
    assert_eq!(inv.method.as_deref(), Some("REQUEST"));
    assert_eq!(inv.uid.as_deref(), Some("abc123@example.com"));
    assert_eq!(inv.sequence, 2);
    // The summary contains a colon; the value must not be cut there.
    assert_eq!(inv.summary.as_deref(), Some("Planning 1:1"));
    // An empty LOCATION is absent, not "".
    assert_eq!(inv.location, None);
    assert_eq!(
        inv.description.as_deref(),
        Some("Bring, please:\n- the draft\n- the numbers")
    );
    let org = inv.organizer.expect("organizer");
    // The quoted CN kept its comma, and the quotes themselves are gone.
    assert_eq!(org.name.as_deref(), Some("Wu, Dana"));
    assert_eq!(org.email.as_deref(), Some("dana@example.com"));
    // The folded attendee line rejoined: RSVP=TRUE split across lines.
    assert_eq!(inv.attendees.len(), 1);
    assert_eq!(inv.attendees[0].email.as_deref(), Some("sam@example.net"));
    assert_eq!(inv.attendees[0].partstat.as_deref(), Some("NEEDS-ACTION"));
    assert_eq!(inv.status.as_deref(), Some("CONFIRMED"));
    // 2026-02-22 23:30 UTC, checked against a known epoch.
    assert_eq!(inv.start, Some(IcalTime::Utc(1_771_803_000_000)));
    assert!(!inv.recurring);
}

#[test]
fn a_method_less_event_attachment_reads_without_buttons_material() {
    // The Ruby-icalendar shape: VTIMEZONE first (whose DTSTART lines must
    // not leak into the event), TZID-local times, no METHOD, no ORGANIZER.
    let ics = [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:icalendar-ruby",
        "BEGIN:VTIMEZONE",
        "TZID:America/New_York",
        "BEGIN:DAYLIGHT",
        "DTSTART:20250309T030000",
        "RRULE:FREQ=YEARLY;BYDAY=2SU;BYMONTH=3",
        "TZNAME:EDT",
        "END:DAYLIGHT",
        "END:VTIMEZONE",
        "BEGIN:VEVENT",
        "DTSTAMP:20250811T152637Z",
        "UID:5e2f283c@example",
        "DTSTART;TZID=America/New_York:20250820T113000",
        "DTEND;TZID=America/New_York:20250820T115000",
        "LOCATION:Video call",
        "SUMMARY:Interview",
        "END:VEVENT",
        "END:VCALENDAR",
    ]
    .join("\r\n");
    let inv = parse_invitation(&ics).expect("parsed");
    assert_eq!(inv.method, None, "no METHOD: a plain event, no reply");
    assert_eq!(inv.organizer, None);
    // The event's own start, not the timezone definition's.
    assert_eq!(
        inv.start,
        Some(IcalTime::Local {
            raw: "20250820T113000".into(),
            tzid: Some("America/New_York".into()),
        })
    );
    // The VTIMEZONE's RRULE is the zone's business, not the event's.
    assert!(!inv.recurring);
    assert_eq!(inv.summary.as_deref(), Some("Interview"));
}

#[test]
fn all_day_dates_stay_dates() {
    let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20260901\r\nSUMMARY:Holiday\r\nEND:VEVENT\r\nEND:VCALENDAR";
    let inv = parse_invitation(ics).expect("parsed");
    assert_eq!(inv.start, Some(IcalTime::Date("20260901".into())));
}

#[test]
fn the_calendar_part_is_found_wherever_it_hides() {
    // As a bare alternative part with method=, the Google way.
    let alternative = format!(
        "From: a@example.com\r\nTo: b@example.com\r\nSubject: invite\r\n\
         MIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=X\r\n\r\n\
         --X\r\nContent-Type: text/plain\r\n\r\nsee attached\r\n\
         --X\r\nContent-Type: text/calendar; charset=UTF-8; method=REQUEST\r\n\r\n{}\r\n--X--\r\n",
        google_style()
    );
    let inv = invitation_in(alternative.as_bytes()).expect("found in alternative");
    assert_eq!(inv.method.as_deref(), Some("REQUEST"));

    // As a base64 .ics attachment, the booking-tool way.
    use std::fmt::Write as _;
    let b64 = {
        // Tiny local base64 to avoid a dev-dependency.
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let data = google_style();
        let mut out = String::new();
        for chunk in data.as_bytes().chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            let _ = write!(
                out,
                "{}{}{}{}",
                T[(n >> 18) as usize & 63] as char,
                T[(n >> 12) as usize & 63] as char,
                if chunk.len() > 1 {
                    T[(n >> 6) as usize & 63] as char
                } else {
                    '='
                },
                if chunk.len() > 2 {
                    T[n as usize & 63] as char
                } else {
                    '='
                },
            );
        }
        out
    };
    let attached = format!(
        "From: a@example.com\r\nTo: b@example.com\r\nSubject: invite\r\n\
         MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=Y\r\n\r\n\
         --Y\r\nContent-Type: text/plain\r\n\r\nsee attached\r\n\
         --Y\r\nContent-Type: text/calendar; name=\"invite.ics\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         Content-Disposition: attachment; filename=\"invite.ics\"\r\n\r\n{b64}\r\n--Y--\r\n"
    );
    let inv = invitation_in(attached.as_bytes()).expect("found as attachment");
    assert_eq!(inv.summary.as_deref(), Some("Planning 1:1"));
}
