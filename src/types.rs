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
    /// Plan of the venue the seating chart is drawn on: an absolute URL, or a
    /// `/uploads/…` path produced by [`crate::api::upload_image`]. Empty for an
    /// event whose chart is a blank sheet.
    pub venue_map: String,
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
    /// Telephone number in E.164, or empty when none was imported. Only the
    /// server ever writes this: it is the only side carrying the metadata
    /// needed to normalise a number correctly.
    pub phone: String,
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
    /// RFC 3339 timestamp of when the host last sent this invitation, empty
    /// when never. It is kept in the database rather than in the browser so a
    /// host who starts the list on a phone and finishes on a laptop sees one
    /// consistent view of their progress — and so does every replica.
    pub invite_sent_at: String,
}

/// What importing one `.vcf` produced, reported back to the admin panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImportSummary {
    /// Contacts the file turned out to contain.
    pub found: usize,
    /// Guests actually added; the remainder were already on the list.
    pub added: usize,
    /// How many of those carried no number that could be normalised. Those
    /// guests are still invited — the host just has to reach them some other
    /// way, so it is worth saying so rather than silently dropping the number.
    pub without_phone: usize,
}

/// An event plus its full guest list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventAdminView {
    pub id: i64,
    pub event: EventInput,
    pub guests: Vec<GuestDto>,
}

// ---------------------------------------------------------------------------
// Seating
// ---------------------------------------------------------------------------

/// Where one arriving person stands on the seating chart.
///
/// There is one of these per *person*, not per guest: a guest who confirmed a
/// party of three is three placements, numbered `0..3` by `seat_index`. The
/// chart is a picture of who is in the room, and a family of three occupies
/// three chairs.
///
/// Coordinates are fractions of the venue map's own rectangle rather than
/// pixels, so a chart arranged on a desktop still reads correctly on a phone,
/// at a different zoom level, or after the map image is replaced with a
/// higher-resolution scan. Values outside `0..1` are legitimate: they are
/// people who have not been placed on the map yet and are still waiting in the
/// tray beside it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SeatPlacement {
    pub guest_id: i64,
    pub seat_index: i64,
    pub x: f64,
    pub y: f64,
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
// Formatting helpers
//
// Anything whose wording depends on the language lives in `crate::i18n`; what
// stays here is locale-neutral.
// ---------------------------------------------------------------------------

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
    fn timestamps_are_shortened_for_display() {
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
