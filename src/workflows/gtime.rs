//! LDAP GeneralizedTime → human-readable local time.
//!
//! The entry form's meta block (createTimestamp / modifyTimestamp) shows
//! server-maintained timestamps. The wire form is `20260728110322Z`, which is
//! unreadable at a glance, so it is rendered as `2026-07-28 13:03:22` in the
//! operator's local time.
//!
//! **Local offset capture.** The `time` crate refuses to read the process's local
//! UTC offset once the process is multi-threaded (it cannot do so soundly), and
//! edaptor spawns an LDAP worker thread. So [`init_local_offset`] must be called
//! from `main` *before* any thread is spawned; it caches the offset for the rest
//! of the run. When it was never called, or the platform could not supply an
//! offset, formatting falls back to UTC and says so.

use std::sync::OnceLock;

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// The process's local UTC offset, captured once at startup. `None` inside means
/// the platform could not supply one; a missing cell means nobody called
/// [`init_local_offset`] (the case in unit tests, which then get UTC).
static LOCAL_OFFSET: OnceLock<Option<UtcOffset>> = OnceLock::new();

/// Capture the local UTC offset for the rest of the process. Call this as the
/// first thing in `main`, before any thread is spawned — see the module docs.
/// Calling it twice is harmless; the first capture wins.
pub fn init_local_offset() {
    let _ = LOCAL_OFFSET.set(UtcOffset::current_local_offset().ok());
}

/// The captured offset, or `None` when unavailable / never captured.
fn local_offset() -> Option<UtcOffset> {
    LOCAL_OFFSET.get().copied().flatten()
}

/// Render an LDAP GeneralizedTime as `YYYY-MM-DD HH:MM:SS` in local time (or
/// `… UTC` when no local offset is available).
///
/// Anything that does not parse is returned verbatim: a surprising value is shown
/// to the operator rather than hidden behind a placeholder.
pub fn format_generalized_time(raw: &str) -> String {
    match parse_generalized_time(raw) {
        Some(dt) => match local_offset() {
            Some(off) => render(dt.to_offset(off), ""),
            None => render(dt.to_offset(UtcOffset::UTC), " UTC"),
        },
        None => raw.to_string(),
    }
}

fn render(dt: OffsetDateTime, suffix: &str) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}{suffix}",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
    )
}

/// Parse the GeneralizedTime forms OpenLDAP emits: `YYYYMMDDHHMMSS` followed by
/// an optional fraction (`.` or `,` plus digits) and a zone that is either `Z` or
/// `±hh[mm]`. Coarser forms (no seconds, no zone) are rejected — they are legal
/// LDAP but never produced by the servers edaptor talks to, and guessing at a
/// zone would be worse than showing the raw value.
fn parse_generalized_time(raw: &str) -> Option<OffsetDateTime> {
    if raw.len() < 15 || !raw.is_char_boundary(14) {
        return None;
    }
    let (stamp, mut rest) = raw.split_at(14);
    if !stamp.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = stamp[0..4].parse().ok()?;
    let month = Month::try_from(stamp[4..6].parse::<u8>().ok()?).ok()?;
    let day: u8 = stamp[6..8].parse().ok()?;
    let hour: u8 = stamp[8..10].parse().ok()?;
    let minute: u8 = stamp[10..12].parse().ok()?;
    let second: u8 = stamp[12..14].parse().ok()?;

    // Optional fractional seconds — parsed only to be skipped; second resolution
    // is all the form shows.
    if let Some(after_dot) = rest.strip_prefix(['.', ',']) {
        let digits = after_dot.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        rest = &after_dot[digits..];
    }

    let offset = parse_zone(rest)?;
    let date = Date::from_calendar_date(year, month, day).ok()?;
    let time = Time::from_hms(hour, minute, second).ok()?;
    Some(PrimitiveDateTime::new(date, time).assume_offset(offset))
}

/// `Z`, `+hhmm`, `-hhmm`, `+hh`, `-hh` → a [`UtcOffset`].
fn parse_zone(zone: &str) -> Option<UtcOffset> {
    if zone == "Z" {
        return Some(UtcOffset::UTC);
    }
    let mut chars = zone.chars();
    let sign: i8 = match chars.next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits = chars.as_str();
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (hours, minutes) = match digits.len() {
        2 => (digits.parse::<i8>().ok()?, 0),
        4 => (
            digits[0..2].parse::<i8>().ok()?,
            digits[2..4].parse::<i8>().ok()?,
        ),
        _ => return None,
    };
    UtcOffset::from_hms(sign * hours, sign * minutes, 0).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests never call `init_local_offset`, so `local_offset()` is `None`
    // and formatting takes the UTC path — deterministic regardless of the
    // machine's timezone.

    #[test]
    fn formats_a_zulu_timestamp_as_utc_when_no_local_offset() {
        assert_eq!(
            format_generalized_time("20260728110322Z"),
            "2026-07-28 11:03:22 UTC"
        );
    }

    #[test]
    fn drops_fractional_seconds() {
        assert_eq!(
            format_generalized_time("20260728110322.439475Z"),
            "2026-07-28 11:03:22 UTC"
        );
        assert_eq!(
            format_generalized_time("20260728110322,5Z"),
            "2026-07-28 11:03:22 UTC"
        );
    }

    #[test]
    fn normalises_an_offset_zone_to_utc() {
        // 13:03:22+02:00 is 11:03:22 UTC.
        assert_eq!(
            format_generalized_time("20260728130322+0200"),
            "2026-07-28 11:03:22 UTC"
        );
        // A bare-hour zone is legal too.
        assert_eq!(
            format_generalized_time("20260728130322+02"),
            "2026-07-28 11:03:22 UTC"
        );
        // Negative offsets cross the date boundary correctly.
        assert_eq!(
            format_generalized_time("20260728230322-0500"),
            "2026-07-29 04:03:22 UTC"
        );
    }

    #[test]
    fn unparsable_values_are_returned_verbatim() {
        for raw in [
            "",
            "not a time",
            "20260728110322",   // no zone
            "202607281103Z",    // no seconds
            "20260732110322Z",  // day 32
            "20261328110322Z",  // month 13
            "20260728250322Z",  // hour 25
            "20260728110322.Z", // empty fraction
            "20260728110322+2", // one-digit zone
            "20260728110322*",  // junk zone
        ] {
            assert_eq!(format_generalized_time(raw), raw, "input {raw:?}");
        }
    }

    #[test]
    fn renders_in_the_captured_local_offset_when_there_is_one() {
        // `format_generalized_time` reads the cached offset; drive the same
        // rendering path with an explicit one to prove the conversion (the cache
        // itself is process-global and set once, so it cannot be exercised here).
        let dt = parse_generalized_time("20260728110322Z").expect("parses");
        let plus_two = UtcOffset::from_hms(2, 0, 0).unwrap();
        assert_eq!(render(dt.to_offset(plus_two), ""), "2026-07-28 13:03:22");
    }
}
