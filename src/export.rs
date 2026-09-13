//! CSV export of guest responses.

use crate::types::GuestDto;

/// Renders an event's guest list as a CSV document.
///
/// The output starts with a UTF-8 BOM so Excel opens non-ASCII guest names
/// correctly, which is the whole point of downloading the file.
pub fn responses_csv(guests: &[GuestDto], base_url: &str) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(
        "guest_id,name,status,party_size,max_party_size,note,responded_at,invite_link\r\n",
    );
    for g in guests {
        let link = format!("{}/i/{}", base_url.trim_end_matches('/'), g.token);
        let row = [
            g.id.to_string(),
            g.name.clone(),
            g.status.as_str().to_owned(),
            g.party_size.to_string(),
            g.max_party_size.to_string(),
            g.note.clone(),
            g.responded_at.clone(),
            link,
        ];
        out.push_str(&row.map(|f| escape(&f)).join(","));
        out.push_str("\r\n");
    }
    out
}

/// A filename-safe slug of an event title, for `Content-Disposition`.
pub fn slug(title: &str) -> String {
    let mut s: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "event".to_owned() } else { s }
}

/// Quotes a CSV field, and neutralises leading characters that spreadsheets
/// would otherwise treat as the start of a formula.
fn escape(field: &str) -> String {
    let needs_guard = field.starts_with(['=', '+', '-', '@']);
    let needs_quotes =
        needs_guard || field.contains([',', '"', '\n', '\r']) || field != field.trim();
    if !needs_quotes {
        return field.to_owned();
    }
    let mut out = String::with_capacity(field.len() + 4);
    out.push('"');
    if needs_guard {
        out.push('\'');
    }
    for c in field.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Rsvp;

    fn guest(name: &str, note: &str) -> GuestDto {
        GuestDto {
            id: 7,
            name: name.to_owned(),
            token: "abc123".to_owned(),
            max_party_size: 2,
            status: Rsvp::Attending,
            party_size: 2,
            note: note.to_owned(),
            responded_at: "2026-09-12T19:04:00+00:00".to_owned(),
        }
    }

    #[test]
    fn csv_starts_with_a_bom_and_a_header() {
        let csv = responses_csv(&[], "https://x.test");
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("guest_id,name,status"));
    }

    #[test]
    fn csv_quotes_fields_containing_separators() {
        let csv = responses_csv(&[guest("Cohen, Dana", "says \"hi\"")], "https://x.test/");
        assert!(csv.contains("\"Cohen, Dana\""));
        assert!(csv.contains("\"says \"\"hi\"\"\""));
    }

    #[test]
    fn csv_neutralises_spreadsheet_formulas() {
        // A name beginning with '=' must not be evaluated when the file is opened.
        let csv = responses_csv(&[guest("=1+1", "")], "https://x.test");
        assert!(csv.contains("\"'=1+1\""));
    }

    #[test]
    fn csv_includes_the_full_invitation_link() {
        let csv = responses_csv(&[guest("Dana", "")], "https://x.test/");
        assert!(csv.contains("https://x.test/i/abc123"));
    }

    #[test]
    fn slugs_are_filename_safe() {
        assert_eq!(slug("Dana & Yuval's Wedding!"), "dana-yuval-s-wedding");
        assert_eq!(slug("***"), "event");
    }
}
