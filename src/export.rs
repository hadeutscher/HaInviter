//! CSV export of guest responses.

use crate::types::GuestDto;

/// Renders an event's guest list as a CSV document.
///
/// The output starts with a UTF-8 BOM so Excel opens non-ASCII guest names
/// correctly, which is the whole point of downloading the file.
///
/// `invite_template` is the locale's invitation message; it is rendered per
/// guest into a ready `wa.me` link, so a host who would rather work down a
/// spreadsheet than the admin panel sends exactly the same wording.
pub fn responses_csv(guests: &[GuestDto], base_url: &str, invite_template: &str) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(
        "guest_id,name,phone,status,party_size,max_party_size,note,responded_at,invite_link,\
         whatsapp_link,invite_sent_at\r\n",
    );
    for g in guests {
        let link = crate::contacts::invite_link(base_url, &g.token);
        let message = crate::contacts::invite_message(invite_template, &g.name, &link);
        let row = [
            g.id.to_string(),
            g.name.clone(),
            g.phone.clone(),
            g.status.as_str().to_owned(),
            g.party_size.to_string(),
            g.max_party_size.to_string(),
            g.note.clone(),
            g.responded_at.clone(),
            link,
            crate::contacts::wa_me(&g.phone, &message),
            g.invite_sent_at.clone(),
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

    const TEMPLATE: &str = "Hi {name}! Your invitation: {link}";

    fn guest(name: &str, note: &str) -> GuestDto {
        GuestDto {
            id: 7,
            name: name.to_owned(),
            phone: "+972501234567".to_owned(),
            token: "abc123".to_owned(),
            max_party_size: 2,
            status: Rsvp::Attending,
            party_size: 2,
            note: note.to_owned(),
            responded_at: "2026-09-12T19:04:00+00:00".to_owned(),
            invite_sent_at: String::new(),
        }
    }

    #[test]
    fn csv_starts_with_a_bom_and_a_header() {
        let csv = responses_csv(&[], "https://x.test", TEMPLATE);
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("guest_id,name,phone,status"));
    }

    #[test]
    fn csv_quotes_fields_containing_separators() {
        let csv = responses_csv(
            &[guest("Cohen, Dana", "says \"hi\"")],
            "https://x.test/",
            TEMPLATE,
        );
        assert!(csv.contains("\"Cohen, Dana\""));
        assert!(csv.contains("\"says \"\"hi\"\"\""));
    }

    #[test]
    fn csv_neutralises_spreadsheet_formulas() {
        // A name beginning with '=' must not be evaluated when the file is opened.
        let csv = responses_csv(&[guest("=1+1", "")], "https://x.test", TEMPLATE);
        assert!(csv.contains("\"'=1+1\""));
    }

    #[test]
    fn csv_carries_a_ready_whatsapp_link() {
        let csv = responses_csv(&[guest("Dana", "")], "https://x.test", TEMPLATE);
        // The number loses its '+' in a wa.me path, and the message is encoded.
        assert!(csv.contains("https://wa.me/972501234567?text="));
        assert!(csv.contains("Dana"));
    }

    #[test]
    fn csv_includes_the_full_invitation_link() {
        let csv = responses_csv(&[guest("Dana", "")], "https://x.test/", TEMPLATE);
        assert!(csv.contains("https://x.test/i/abc123"));
    }

    #[test]
    fn slugs_are_filename_safe() {
        assert_eq!(slug("Dana & Yuval's Wedding!"), "dana-yuval-s-wedding");
        assert_eq!(slug("***"), "event");
    }
}
