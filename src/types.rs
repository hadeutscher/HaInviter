//! Data-transfer objects shared by the WASM client and the native server.
//!
//! Everything here must compile for `wasm32-unknown-unknown` as well as the
//! host target, so this module must not reference the database, the filesystem
//! or any other server-only facility.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// RSVP state
// ---------------------------------------------------------------------------

/// Where a guest stands on an invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Rsvp {
    /// Invited, but has not opened or answered their link yet.
    #[default]
    Pending,
    /// Coming.
    Attending,
    /// Not coming.
    Declined,
}

// The database mapping is only ever exercised by the server build; the client
// only ever moves an already-decoded `Rsvp` around.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
impl Rsvp {
    /// Stable identifier used in the database and in CSV exports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Attending => "attending",
            Self::Declined => "declined",
        }
    }

    /// Parses the database representation, defaulting to [`Rsvp::Pending`] for
    /// anything unrecognised so a bad row can never take the server down.
    pub fn from_db(s: &str) -> Self {
        match s {
            "attending" => Self::Attending,
            "declined" => Self::Declined,
            _ => Self::Pending,
        }
    }

    /// Human-readable label for the admin panel.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "Awaiting reply",
            Self::Attending => "Attending",
            Self::Declined => "Not attending",
        }
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Every field of an event the admin can edit. Used both as the create/update
/// payload and as the render model for the invitation page.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EventInput {
    pub title: String,
    /// Who is inviting, e.g. "Dana & Yuval". Shown above the title.
    pub hosts: String,
    pub description: String,
    /// Absolute URL, or a `/uploads/…` path produced by [`crate::api::upload_cover`].
    pub cover_image: String,
    pub location: String,
    /// Optional map link for the location.
    pub location_url: String,
    /// Local wall-clock start, `YYYY-MM-DDTHH:MM` (the value an
    /// `<input type="datetime-local">` produces). Empty when unset.
    pub starts_at: String,
    /// Last day guests can answer, `YYYY-MM-DD`. Empty when unset.
    pub rsvp_deadline: String,
    /// Whether guests may bring additional people (up to their own cap).
    pub allow_plus_ones: bool,
}

/// One row of the admin panel's event list, with response tallies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventSummary {
    pub id: i64,
    pub title: String,
    pub starts_at: String,
    pub location: String,
    pub guest_count: i64,
    pub attending: i64,
    pub declined: i64,
    pub pending: i64,
    /// Total number of people expected (sum of party sizes of attendees).
    pub head_count: i64,
}

// ---------------------------------------------------------------------------
// Guests
// ---------------------------------------------------------------------------

/// A single invited guest, as shown in the admin panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuestDto {
    pub id: i64,
    pub name: String,
    /// Secret token; `/i/{token}` is this guest's personal invitation link.
    pub token: String,
    /// Largest party this guest may confirm (1 = themselves only).
    pub max_party_size: i64,
    pub status: Rsvp,
    /// Confirmed party size; 0 unless the guest is attending.
    pub party_size: i64,
    pub note: String,
    /// RFC 3339 timestamp of the last reply, empty when never answered.
    pub responded_at: String,
}

/// An event plus its full guest list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventAdminView {
    pub id: i64,
    pub event: EventInput,
    pub guests: Vec<GuestDto>,
}

/// Everything the invitation page needs, resolved from a guest token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteView {
    pub guest_name: String,
    pub max_party_size: i64,
    pub status: Rsvp,
    pub party_size: i64,
    pub note: String,
    pub event: EventInput,
    /// True once the RSVP deadline has passed; the form is then read-only.
    pub closed: bool,
}

/// A guest's answer, submitted from the invitation page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RsvpSubmission {
    pub attending: bool,
    pub party_size: i64,
    pub note: String,
}

// ---------------------------------------------------------------------------
// Formatting helpers (shared, so the client and the CSV export agree)
// ---------------------------------------------------------------------------

/// Formats a `YYYY-MM-DDTHH:MM` local timestamp as
/// `"Saturday, 12 September 2026 at 19:00"`.
///
/// Unparseable input is returned unchanged rather than hidden, so a typo in the
/// admin panel is visible instead of silently blanking the invitation.
pub fn format_event_datetime(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(trimmed, fmt) {
            return dt.format("%A, %-d %B %Y at %H:%M").to_string();
        }
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return d.format("%A, %-d %B %Y").to_string();
    }
    trimmed.to_owned()
}

/// Formats a `YYYY-MM-DD` date as `"12 September 2026"`.
pub fn format_date(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .map(|d| d.format("%-d %B %Y").to_string())
        .unwrap_or_else(|_| trimmed.to_owned())
}

/// Turns an RFC 3339 timestamp into `"2026-09-12 19:04"` for table display.
pub fn format_timestamp(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed.find('T') {
        Some(t) if trimmed.len() >= t + 6 => {
            format!("{} {}", &trimmed[..t], &trimmed[t + 1..t + 6])
        }
        _ => trimmed.to_owned(),
    }
}

/// Splits a pasted, line-separated guest list into `(name, party_size)` pairs.
///
/// Each line is `Name` or `Name, 3` — a trailing comma-separated integer sets
/// that guest's party cap. Blank lines and duplicate names within the paste are
/// dropped. `default_party_size` applies to lines without an explicit number.
///
/// Lives here rather than in the server so the parsing rules documented in the
/// admin UI and the rules actually applied are the same code.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub fn parse_guest_list(raw: &str, default_party_size: i64) -> Vec<(String, i64)> {
    let mut out: Vec<(String, i64)> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (name, size) = match line.rsplit_once(',') {
            Some((head, tail)) => match tail.trim().parse::<i64>() {
                Ok(n) if n >= 1 => (head.trim(), n),
                _ => (line, default_party_size),
            },
            None => (line, default_party_size),
        };
        if name.is_empty() {
            continue;
        }
        if out
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
        {
            continue;
        }
        out.push((name.to_owned(), size.clamp(1, 50)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_list_takes_one_name_per_line() {
        let parsed = parse_guest_list("Dana\n\n  Yuval  \n", 1);
        assert_eq!(
            parsed,
            vec![("Dana".to_owned(), 1), ("Yuval".to_owned(), 1)]
        );
    }

    #[test]
    fn guest_list_reads_a_trailing_party_size() {
        let parsed = parse_guest_list("The Cohen family, 4\nRoni", 2);
        assert_eq!(
            parsed,
            vec![("The Cohen family".to_owned(), 4), ("Roni".to_owned(), 2)]
        );
    }

    #[test]
    fn guest_list_keeps_commas_that_are_not_counts() {
        // A name containing a comma must survive intact rather than being split.
        let parsed = parse_guest_list("Cohen, Dana\n", 1);
        assert_eq!(parsed, vec![("Cohen, Dana".to_owned(), 1)]);
    }

    #[test]
    fn guest_list_drops_duplicates_case_insensitively() {
        let parsed = parse_guest_list("Dana\ndana\nDANA, 3", 1);
        assert_eq!(parsed, vec![("Dana".to_owned(), 1)]);
    }

    #[test]
    fn guest_list_clamps_absurd_party_sizes() {
        assert_eq!(
            parse_guest_list("Dana, 900", 1),
            vec![("Dana".to_owned(), 50)]
        );
        assert_eq!(parse_guest_list("Dana", 0), vec![("Dana".to_owned(), 1)]);
    }

    #[test]
    fn event_datetime_is_spelled_out() {
        assert_eq!(
            format_event_datetime("2026-09-12T19:30"),
            "Saturday, 12 September 2026 at 19:30"
        );
        assert_eq!(format_event_datetime("  "), "");
        // Anything unparseable is shown as typed rather than silently blanked.
        assert_eq!(format_event_datetime("next spring"), "next spring");
    }

    #[test]
    fn dates_and_timestamps_are_shortened_for_display() {
        assert_eq!(format_date("2026-09-12"), "12 September 2026");
        assert_eq!(
            format_timestamp("2026-09-12T19:04:31.123+00:00"),
            "2026-09-12 19:04"
        );
        assert_eq!(format_timestamp(""), "");
    }

    #[test]
    fn unknown_rsvp_states_fall_back_to_pending() {
        assert_eq!(Rsvp::from_db("attending"), Rsvp::Attending);
        assert_eq!(Rsvp::from_db("declined"), Rsvp::Declined);
        assert_eq!(Rsvp::from_db("nonsense"), Rsvp::Pending);
    }
}
