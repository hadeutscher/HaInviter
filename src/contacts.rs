//! Contact import: vCard parsing, telephone-number normalisation, and the
//! share links built from the result.
//!
//! Guests arrive as contact cards. Selecting several people in a phone's
//! address book and sharing them produces one `.vcf` holding a `VCARD` per
//! contact, and reading that is considerably kinder than asking the host to
//! retype a hundred names and numbers.
//!
//! Only the share links at the bottom of this module are shared by both builds;
//! the admin panel needs them to render a row. Everything above them — the
//! parser and the E.164 normalisation — is server-only, because nothing in the
//! browser ever reads a vCard, and because normalising a number correctly needs
//! libphonenumber's metadata, which has no business in a bundle that is opened
//! over mobile data.

// ---------------------------------------------------------------------------
// Contacts
// ---------------------------------------------------------------------------

/// One contact recovered from a vCard.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "server")]
pub struct Contact {
    pub name: String,
    /// The number exactly as the card spelled it. Normalising it is the
    /// server's job, because only the server has the metadata to do it.
    pub phone: String,
}

/// Unfolds a vCard's physical lines into logical ones.
///
/// A long value may be continued on the next line by starting it with a space
/// or a tab; that byte is part of the folding, not of the value.
#[cfg(feature = "server")]
fn unfold(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = out.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        out.push(line.to_owned());
    }
    out
}

/// One `NAME;PARAM=VALUE:value` line, already split up.
#[cfg(feature = "server")]
struct Property<'a> {
    /// Upper-cased, with any `item1.` grouping prefix removed.
    name: String,
    /// Upper-cased parameters, as written.
    params: Vec<String>,
    value: &'a str,
}

/// Splits a property line at the colon that separates its value.
///
/// The colon has to be found rather than searched for naively: a parameter may
/// be a quoted string containing one.
#[cfg(feature = "server")]
fn split_property(line: &str) -> Option<Property<'_>> {
    let mut quoted = false;
    let mut at = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                at = Some(i);
                break;
            }
            _ => {}
        }
    }
    let at = at?;
    let (head, rest) = line.split_at(at);
    let mut parts = head.split(';');
    let raw_name = parts.next()?;
    Some(Property {
        // `item1.TEL` is a grouping prefix; it carries nothing we need.
        name: raw_name.rsplit('.').next()?.trim().to_ascii_uppercase(),
        params: parts.map(|p| p.trim().to_ascii_uppercase()).collect(),
        value: &rest[1..],
    })
}

/// Decodes a quoted-printable value, which is how most Android address books
/// still export anything that is not ASCII — Hebrew names, in particular.
///
/// An undecodable escape is passed through rather than dropped: a mangled
/// character in a name is recoverable by eye, a missing guest is not.
#[cfg(feature = "server")]
fn decode_quoted_printable(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'='
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Undoes vCard's backslash escaping of separators and newlines.
#[cfg(feature = "server")]
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

/// Assembles a display name from a structured `N:Family;Given;…` value.
#[cfg(feature = "server")]
fn name_from_structured(value: &str) -> String {
    let parts: Vec<String> = value.split(';').map(unescape).collect();
    let family = parts.first().map(String::as_str).unwrap_or("").trim();
    let given = parts.get(1).map(String::as_str).unwrap_or("").trim();
    format!("{given} {family}").trim().to_owned()
}

/// How much we would rather have this `TEL` than another one on the same card.
///
/// A mobile number is the only one worth having here — the link it ends up in
/// opens a WhatsApp chat, and a landline has no WhatsApp account.
#[cfg(feature = "server")]
fn tel_rank(params: &[String]) -> u8 {
    let has = |needle: &str| params.iter().any(|p| p.contains(needle));
    if has("CELL") || has("MOBILE") {
        3
    } else if has("IPHONE") {
        2
    } else if has("FAX") {
        0
    } else {
        1
    }
}

/// Reads every `VCARD` in `raw`.
///
/// A card with no usable name is dropped; a card with a name but no number is
/// kept, because that guest is still invited — the host just cannot send to
/// them in one tap.
#[cfg(feature = "server")]
pub fn parse_vcards(raw: &str) -> Vec<Contact> {
    let lines = unfold(raw);
    let mut out: Vec<Contact> = Vec::new();

    let mut open = false;
    let mut formatted = String::new();
    let mut structured = String::new();
    let mut best: Option<(u8, String)> = None;

    let mut i = 0;
    while i < lines.len() {
        // Quoted-printable has a soft line break of its own — a trailing `=`,
        // with no leading whitespace on the continuation — so it has to be
        // joined here rather than during unfolding.
        let mut logical = lines[i].clone();
        i += 1;
        if logical.to_ascii_uppercase().contains("QUOTED-PRINTABLE") {
            while logical.ends_with('=') && i < lines.len() {
                logical.pop();
                logical.push_str(&lines[i]);
                i += 1;
            }
        }

        let trimmed = logical.trim();
        if trimmed.eq_ignore_ascii_case("BEGIN:VCARD") {
            open = true;
            formatted.clear();
            structured.clear();
            best = None;
            continue;
        }
        if trimmed.eq_ignore_ascii_case("END:VCARD") {
            let name = if formatted.is_empty() {
                structured.clone()
            } else {
                formatted.clone()
            };
            if !name.is_empty() {
                out.push(Contact {
                    name,
                    phone: best.as_ref().map(|(_, v)| v.clone()).unwrap_or_default(),
                });
            }
            open = false;
            continue;
        }
        if !open {
            continue;
        }

        let Some(property) = split_property(&logical) else {
            continue;
        };
        let decoded = if property
            .params
            .iter()
            .any(|p| p.contains("QUOTED-PRINTABLE"))
        {
            decode_quoted_printable(property.value)
        } else {
            property.value.to_owned()
        };

        match property.name.as_str() {
            "FN" => formatted = unescape(decoded.trim()).trim().to_owned(),
            "N" => {
                if structured.is_empty() {
                    structured = name_from_structured(&decoded);
                }
            }
            "TEL" => {
                let value = decoded.trim().to_owned();
                if value.is_empty() {
                    continue;
                }
                let rank = tel_rank(&property.params);
                // Strictly greater, so the first number of the best rank wins.
                if best.as_ref().is_none_or(|(seen, _)| rank > *seen) {
                    best = Some((rank, value));
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Telephone numbers
// ---------------------------------------------------------------------------

/// Normalises a number as written on a contact card into E.164.
///
/// `region` is a CLDR region code such as `IL`, and may be empty — in which
/// case only numbers already written in international form are accepted. That
/// is deliberate: guessing a country for a bare `050-…` would silently invent
/// a wrong number, and a guest who never receives their invitation is a worse
/// outcome than one the host has to fix by hand.
///
/// Returns `None` for anything that cannot be read as a number at all, which the
/// caller stores as an empty phone rather than rejecting the guest outright.
///
/// Note what is *not* applied here: `phonenumber::is_valid`, the check that a
/// number falls inside an allocated range. The bundled metadata reports every
/// Israeli mobile number — the whole `+972 5x` space — as invalid, while
/// accepting Israeli landlines and every other country's mobiles. Gating on it
/// would reject essentially every guest of a Hebrew deployment. Parsing is the
/// part worth having a library for anyway: country codes, national prefixes and
/// the dozen ways a person writes a number into an address book. A number that
/// parses is good enough to put in a link, and the asymmetry favours accepting:
/// a wrong number costs the host one correction, a refused right one costs them
/// a guest.
#[cfg(feature = "server")]
pub fn to_e164(raw: &str, region: &str) -> Option<String> {
    use phonenumber::Mode;

    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let country = match region.trim() {
        "" => None,
        code => Some(code.to_ascii_uppercase().parse().ok()?),
    };
    let parsed = phonenumber::parse(country, raw).ok()?;
    Some(parsed.format().mode(Mode::E164).to_string())
}

/// The region used to read numbers that are not in international form, from
/// `HAINVITER_DEFAULT_REGION`. Empty — the default — accepts only `+…` numbers.
#[cfg(feature = "server")]
pub fn default_region() -> String {
    std::env::var("HAINVITER_DEFAULT_REGION")
        .unwrap_or_default()
        .trim()
        .to_owned()
}

// ---------------------------------------------------------------------------
// Share links
// ---------------------------------------------------------------------------

/// Fills the invitation message template for one guest.
///
/// The same substitution serves the admin panel's buttons and the CSV export,
/// so the message a host sends by tapping a row and the one they send out of a
/// spreadsheet are word for word the same.
pub fn invite_message(template: &str, name: &str, link: &str) -> String {
    template.replace("{name}", name).replace("{link}", link)
}

/// The full invitation URL for a guest token, given a public origin.
pub fn invite_link(base_url: &str, guest_token: &str) -> String {
    format!("{}/i/{guest_token}", base_url.trim_end_matches('/'))
}

/// Builds a `wa.me` link carrying a pre-filled message.
///
/// With a number, this opens that guest's chat; without one it opens WhatsApp's
/// own contact picker with the message already written, which is what makes the
/// button useful for a guest whose card carried no mobile number.
pub fn wa_me(phone_e164: &str, text: &str) -> String {
    let digits: String = phone_e164.chars().filter(char::is_ascii_digit).collect();
    let encoded = urlencoding::encode(text);
    if digits.is_empty() {
        format!("https://wa.me/?text={encoded}")
    } else {
        format!("https://wa.me/{digits}?text={encoded}")
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    const SIMPLE: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Dana Cohen\r\nTEL;TYPE=CELL:+972 50-123-4567\r\nEND:VCARD\r\n";

    #[test]
    fn reads_a_single_card() {
        assert_eq!(
            parse_vcards(SIMPLE),
            vec![Contact {
                name: "Dana Cohen".to_owned(),
                phone: "+972 50-123-4567".to_owned(),
            }]
        );
    }

    #[test]
    fn reads_every_card_in_one_file() {
        let raw = format!("{SIMPLE}BEGIN:VCARD\nFN:Roni Levi\nTEL:03-1234567\nEND:VCARD\n");
        let parsed = parse_vcards(&raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].name, "Roni Levi");
    }

    #[test]
    fn falls_back_to_the_structured_name() {
        let raw = "BEGIN:VCARD\nN:Cohen;Dana;;;\nTEL:0501234567\nEND:VCARD\n";
        assert_eq!(parse_vcards(raw)[0].name, "Dana Cohen");
    }

    #[test]
    fn prefers_the_mobile_number() {
        let raw = "BEGIN:VCARD\nFN:Dana\nTEL;TYPE=HOME:03-1111111\nTEL;TYPE=CELL:050-2222222\nEND:VCARD\n";
        assert_eq!(parse_vcards(raw)[0].phone, "050-2222222");
    }

    #[test]
    fn never_prefers_a_fax() {
        let raw = "BEGIN:VCARD\nFN:Dana\nTEL;TYPE=FAX:03-1111111\nTEL:050-2222222\nEND:VCARD\n";
        assert_eq!(parse_vcards(raw)[0].phone, "050-2222222");
    }

    #[test]
    fn unfolds_continuation_lines() {
        let raw = "BEGIN:VCARD\nFN:Dana\n  Cohen\nTEL:050\nEND:VCARD\n";
        assert_eq!(parse_vcards(raw)[0].name, "Dana Cohen");
    }

    #[test]
    fn decodes_quoted_printable_hebrew() {
        // "דנה" as most Android address books export it.
        let raw = "BEGIN:VCARD\nFN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=D7=93=D7=A0=D7=94\nTEL:050\nEND:VCARD\n";
        assert_eq!(parse_vcards(raw)[0].name, "דנה");
    }

    #[test]
    fn joins_quoted_printable_soft_breaks() {
        let raw =
            "BEGIN:VCARD\nFN;ENCODING=QUOTED-PRINTABLE:=D7=93=\n=D7=A0=D7=94\nTEL:050\nEND:VCARD\n";
        assert_eq!(parse_vcards(raw)[0].name, "דנה");
    }

    #[test]
    fn ignores_a_grouping_prefix() {
        let raw = "BEGIN:VCARD\nitem1.FN:Dana\nitem1.TEL:050\nEND:VCARD\n";
        assert_eq!(parse_vcards(raw)[0].name, "Dana");
    }

    #[test]
    fn keeps_a_guest_who_has_no_number() {
        let raw = "BEGIN:VCARD\nFN:Dana\nEND:VCARD\n";
        assert_eq!(parse_vcards(raw)[0].phone, "");
    }

    #[test]
    fn drops_a_card_with_no_name() {
        assert!(parse_vcards("BEGIN:VCARD\nTEL:050\nEND:VCARD\n").is_empty());
    }

    #[test]
    fn unescapes_separators_in_a_name() {
        let raw = "BEGIN:VCARD\nFN:Cohen\\, Dana\nEND:VCARD\n";
        assert_eq!(parse_vcards(raw)[0].name, "Cohen, Dana");
    }

    #[test]
    fn survives_junk_outside_a_card() {
        assert!(parse_vcards("nonsense\nFN:Nobody\n").is_empty());
    }

    #[test]
    fn normalises_an_international_number_without_a_region() {
        assert_eq!(
            to_e164("+972 50-123-4567", "").as_deref(),
            Some("+972501234567")
        );
    }

    #[test]
    fn reads_a_national_number_when_a_region_is_configured() {
        assert_eq!(
            to_e164("050-123-4567", "IL").as_deref(),
            Some("+972501234567")
        );
    }

    #[test]
    fn refuses_a_national_number_with_no_region_to_read_it_against() {
        assert_eq!(to_e164("050-123-4567", ""), None);
    }

    /// The case that made `is_valid` unusable here — see [`to_e164`].
    #[test]
    fn accepts_israeli_mobile_numbers() {
        for raw in ["+972501234567", "+972541234567", "+972581234567"] {
            assert_eq!(
                to_e164(raw, "").as_deref(),
                Some(raw),
                "{raw} should survive"
            );
        }
    }

    #[test]
    fn refuses_something_that_is_not_a_number() {
        assert_eq!(to_e164("banana", "IL"), None);
    }

    #[test]
    fn wa_me_links_to_a_chat_when_a_number_is_known() {
        assert_eq!(
            wa_me("+972501234567", "hi"),
            "https://wa.me/972501234567?text=hi"
        );
    }

    #[test]
    fn wa_me_opens_the_picker_without_a_number() {
        assert_eq!(wa_me("", "a b"), "https://wa.me/?text=a%20b");
    }
}
