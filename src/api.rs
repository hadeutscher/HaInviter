//! Server functions: the entire client/server boundary.
//!
//! Every admin call takes the admin token as its first argument and is refused
//! without it — there is no session, no cookie and nothing to steal from the
//! browser beyond the link itself. Guest calls are authorised by the guest's own
//! token in exactly the same way.
//!
//! Only the signatures of these functions are compiled into the WASM bundle; the
//! bodies exist on the server alone.

use crate::{
    i18n::Locale,
    types::{EventAdminView, EventInput, EventSummary, ImportSummary, InviteView, RsvpSubmission},
};
use dioxus::prelude::*;

/// Largest cover image we accept, in bytes.
pub const MAX_COVER_BYTES: usize = 8 * 1024 * 1024;

/// Largest `.vcf` we accept, in bytes. A whole phone address book exported at
/// once is a few hundred kilobytes; this leaves generous room above that while
/// still refusing something that is plainly not a contact file.
pub const MAX_VCF_BYTES: usize = 2 * 1024 * 1024;

/// The active locale's strings, for messages that reach the user.
#[cfg(feature = "server")]
fn s() -> &'static crate::i18n::Strings {
    crate::i18n::from_env().strings()
}

/// Rejects anything that is not the admin token.
#[cfg(feature = "server")]
fn guard(token: &str) -> Result<(), ServerFnError> {
    if crate::auth::is_admin(token) {
        Ok(())
    } else {
        // Deliberately vague: the caller either has the link or does not.
        Err(ServerFnError::new(s().err_not_authorised))
    }
}

/// Checks out a database connection, as a `ServerFnError` on failure.
#[cfg(feature = "server")]
async fn conn() -> Result<deadpool_postgres::Object, ServerFnError> {
    crate::db::client().await.map_err(ServerFnError::new)
}

// ---------------------------------------------------------------------------
// Public
// ---------------------------------------------------------------------------

/// The locale this deployment serves.
///
/// The client needs it before its first render, or server-rendered markup and
/// the hydrated client would disagree on every string.
#[server(endpoint = "locale")]
pub async fn locale() -> Result<Locale, ServerFnError> {
    Ok(crate::i18n::from_env())
}

/// The externally reachable origin (e.g. `https://invites.example.com`), used to
/// build shareable invitation links. Empty when `HAINVITER_BASE_URL` is unset, in
/// which case the admin panel uses the browser's own origin instead.
#[server(endpoint = "base_url")]
pub async fn base_url() -> Result<String, ServerFnError> {
    Ok(std::env::var("HAINVITER_BASE_URL")
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_owned())
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[server(endpoint = "list_events")]
pub async fn list_events(token: String) -> Result<Vec<EventSummary>, ServerFnError> {
    guard(&token)?;
    let client = conn().await?;
    crate::db::list_events(&**client)
        .await
        .map_err(ServerFnError::new)
}

#[server(endpoint = "create_event")]
pub async fn create_event(token: String, input: EventInput) -> Result<i64, ServerFnError> {
    guard(&token)?;
    let input = sanitise(input);
    if input.title.is_empty() {
        return Err(ServerFnError::new(s().err_event_needs_title));
    }
    let client = conn().await?;
    let id = crate::db::create_event(&**client, &input)
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record(
        "event_created",
        serde_json::json!({ "event_id": id, "title": input.title }),
    );
    Ok(id)
}

#[server(endpoint = "update_event")]
pub async fn update_event(token: String, id: i64, input: EventInput) -> Result<(), ServerFnError> {
    guard(&token)?;
    let input = sanitise(input);
    if input.title.is_empty() {
        return Err(ServerFnError::new(s().err_event_needs_title));
    }
    let client = conn().await?;
    let changed = crate::db::update_event(&**client, id, &input)
        .await
        .map_err(ServerFnError::new)?;
    if changed == 0 {
        return Err(ServerFnError::new(s().err_no_such_event));
    }
    crate::audit::record(
        "event_updated",
        serde_json::json!({ "event_id": id, "title": input.title }),
    );
    Ok(())
}

#[server(endpoint = "delete_event")]
pub async fn delete_event(token: String, id: i64) -> Result<(), ServerFnError> {
    guard(&token)?;
    let client = conn().await?;
    // Read the title and guest list first so the audit trail says what was
    // removed. Guests cascade away with the event, but their replies stay in
    // `rsvp_log`.
    let title = crate::db::event_title(&**client, id)
        .await
        .map_err(ServerFnError::new)?
        .unwrap_or_else(|| "<unknown>".to_owned());
    let guests = crate::db::list_guests(&**client, id)
        .await
        .map_err(ServerFnError::new)?;
    crate::db::delete_event(&**client, id)
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record(
        "event_deleted",
        serde_json::json!({
            "event_id": id,
            "title": title,
            "guests_removed": guests.len(),
            "responses_removed": guests.iter().filter(|g| !g.responded_at.is_empty()).count(),
        }),
    );
    Ok(())
}

#[server(endpoint = "get_event")]
pub async fn get_event(token: String, id: i64) -> Result<EventAdminView, ServerFnError> {
    guard(&token)?;
    let client = conn().await?;
    crate::db::event_admin_view(&**client, id)
        .await
        .map_err(ServerFnError::new)?
        .ok_or_else(|| ServerFnError::new(s().err_no_such_event))
}

// ---------------------------------------------------------------------------
// Guests
// ---------------------------------------------------------------------------

/// Adds guests from a line-separated list (one `Name` or `Name, 3` per line).
///
/// Returns the number actually added; names already on the list are skipped so
/// pasting an updated list twice is safe.
#[server(endpoint = "add_guests")]
pub async fn add_guests(
    token: String,
    event_id: i64,
    raw: String,
    default_party_size: i64,
) -> Result<usize, ServerFnError> {
    guard(&token)?;
    let parsed = crate::types::parse_guest_list(&raw, default_party_size.clamp(1, 50));
    if parsed.is_empty() {
        return Err(ServerFnError::new(s().err_no_names));
    }
    let rows: Vec<crate::db::NewGuest> = parsed
        .into_iter()
        .map(|(name, max_party_size)| crate::db::NewGuest {
            name,
            max_party_size,
            token: crate::auth::new_token(),
            // Pasted names carry no number; importing contacts later fills it in.
            phone: String::new(),
        })
        .collect();
    let names: Vec<&String> = rows.iter().map(|g| &g.name).collect();
    let mut client = conn().await?;
    let added = crate::db::add_guests(&mut client, event_id, &rows)
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record(
        "guests_added",
        serde_json::json!({ "event_id": event_id, "added": added, "names": names }),
    );
    Ok(added)
}

/// Adds guests from a `.vcf` shared out of a phone's address book.
///
/// A contact whose number cannot be normalised is still added — losing the
/// number costs the host one tap, losing the guest costs them a guest.
#[server(endpoint = "import_contacts")]
pub async fn import_contacts(
    token: String,
    event_id: i64,
    bytes: Vec<u8>,
    default_party_size: i64,
) -> Result<ImportSummary, ServerFnError> {
    guard(&token)?;
    if bytes.is_empty() {
        return Err(ServerFnError::new(s().err_file_empty));
    }
    if bytes.len() > MAX_VCF_BYTES {
        return Err(ServerFnError::new(
            s().field_cover_too_large
                .replace("{}", &(MAX_VCF_BYTES / (1024 * 1024)).to_string()),
        ));
    }

    // Lossy on purpose: a card written in some other encoding should cost one
    // mangled character, not the whole import.
    let text = String::from_utf8_lossy(&bytes);
    let contacts = crate::contacts::parse_vcards(&text);
    if contacts.is_empty() {
        return Err(ServerFnError::new(s().err_no_contacts));
    }

    let region = crate::contacts::default_region();
    let max_party_size = default_party_size.clamp(1, 50);
    let mut without_phone = 0usize;
    let mut rows: Vec<crate::db::NewGuest> = Vec::with_capacity(contacts.len());
    for contact in &contacts {
        let phone = crate::contacts::to_e164(&contact.phone, &region).unwrap_or_default();
        if phone.is_empty() {
            without_phone += 1;
        }
        rows.push(crate::db::NewGuest {
            name: contact.name.clone(),
            max_party_size,
            token: crate::auth::new_token(),
            phone,
        });
    }

    let mut client = conn().await?;
    let added = crate::db::add_guests(&mut client, event_id, &rows)
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record(
        "contacts_imported",
        serde_json::json!({
            "event_id": event_id,
            "found": contacts.len(),
            "added": added,
            "without_phone": without_phone,
        }),
    );
    Ok(ImportSummary {
        found: contacts.len(),
        added,
        without_phone,
    })
}

#[server(endpoint = "update_guest")]
pub async fn update_guest(
    token: String,
    guest_id: i64,
    name: String,
    max_party_size: i64,
    phone: String,
) -> Result<String, ServerFnError> {
    guard(&token)?;
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(ServerFnError::new(s().err_guest_needs_name));
    }
    let max_party_size = max_party_size.clamp(1, 50);
    // A number typed by hand is normalised exactly like an imported one, so the
    // two cannot drift into different formats in the same column. Something
    // unparseable is rejected rather than stored, because a wrong number in a
    // link is worse than no link at all.
    let phone = match phone.trim() {
        "" => String::new(),
        raw => crate::contacts::to_e164(raw, &crate::contacts::default_region())
            .ok_or_else(|| ServerFnError::new(s().err_bad_phone))?,
    };
    let client = conn().await?;
    crate::db::update_guest(&**client, guest_id, &name, max_party_size, &phone)
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record(
        "guest_updated",
        serde_json::json!({
            "guest_id": guest_id,
            "name": name,
            "max_party_size": max_party_size,
            "has_phone": !phone.is_empty(),
        }),
    );
    Ok(phone)
}

/// Records whether the host has sent this guest their invitation.
///
/// The sending itself happens in WhatsApp — or wherever the share sheet leads —
/// so this is the host's own bookkeeping rather than a delivery receipt. It is
/// stored rather than kept in the browser so the same list read from a phone
/// and from a laptop agrees with itself.
#[server(endpoint = "mark_invite_sent")]
pub async fn mark_invite_sent(
    token: String,
    guest_id: i64,
    sent: bool,
) -> Result<String, ServerFnError> {
    guard(&token)?;
    let at = if sent {
        crate::db::now()
    } else {
        String::new()
    };
    let client = conn().await?;
    crate::db::set_invite_sent(&**client, guest_id, &at)
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record(
        "invite_marked_sent",
        serde_json::json!({ "guest_id": guest_id, "sent": sent }),
    );
    Ok(at)
}

#[server(endpoint = "delete_guest")]
pub async fn delete_guest(token: String, guest_id: i64) -> Result<(), ServerFnError> {
    guard(&token)?;
    let client = conn().await?;
    crate::db::delete_guest(&**client, guest_id)
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record("guest_deleted", serde_json::json!({ "guest_id": guest_id }));
    Ok(())
}

/// Clears a guest's answer so they are asked again. Their previous reply stays in
/// the audit log and in `rsvp_log`.
#[server(endpoint = "reset_guest")]
pub async fn reset_guest(token: String, guest_id: i64) -> Result<(), ServerFnError> {
    guard(&token)?;
    let client = conn().await?;
    crate::db::reset_guest(&**client, guest_id)
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record("guest_reset", serde_json::json!({ "guest_id": guest_id }));
    Ok(())
}

/// Issues a new invitation link for a guest, invalidating the old one.
#[server(endpoint = "reissue_invite")]
pub async fn reissue_invite(token: String, guest_id: i64) -> Result<String, ServerFnError> {
    guard(&token)?;
    let fresh = crate::auth::new_token();
    let client = conn().await?;
    let changed = crate::db::reissue_guest_token(&**client, guest_id, &fresh)
        .await
        .map_err(ServerFnError::new)?;
    if changed == 0 {
        return Err(ServerFnError::new(s().err_no_such_guest));
    }
    crate::audit::record(
        "guest_link_reissued",
        serde_json::json!({ "guest_id": guest_id }),
    );
    Ok(fresh)
}

// ---------------------------------------------------------------------------
// Cover images
// ---------------------------------------------------------------------------

/// Stores an uploaded cover image and returns the path to serve it from
/// (`/uploads/…`).
///
/// The bytes go into the database rather than onto local disk, so every replica
/// can serve the image and no shared filesystem is needed. The file type comes
/// from the bytes themselves, never from the supplied file name.
#[server(endpoint = "upload_cover")]
pub async fn upload_cover(
    token: String,
    filename: String,
    bytes: Vec<u8>,
) -> Result<String, ServerFnError> {
    guard(&token)?;
    if bytes.is_empty() {
        return Err(ServerFnError::new(s().err_file_empty));
    }
    if bytes.len() > MAX_COVER_BYTES {
        return Err(ServerFnError::new(
            s().field_cover_too_large
                .replace("{}", &(MAX_COVER_BYTES / (1024 * 1024)).to_string()),
        ));
    }
    let (ext, content_type) =
        sniff_image(&bytes).ok_or_else(|| ServerFnError::new(s().err_not_an_image))?;

    // The stored id is random, so an upload can neither overwrite an existing
    // image nor be guessed from the outside.
    let name = format!("{}.{ext}", crate::auth::new_token());
    let client = conn().await?;
    crate::db::store_cover(&**client, &name, content_type, &bytes)
        .await
        .map_err(|e| ServerFnError::new(format!("{}: {e}", s().err_store_failed)))?;

    crate::audit::record(
        "cover_uploaded",
        serde_json::json!({
            "stored_as": name,
            "original_name": filename,
            "bytes": bytes.len(),
        }),
    );
    Ok(format!("/uploads/{name}"))
}

/// Identifies an image from its magic bytes, returning `(extension, MIME type)`.
/// Anything unrecognised is rejected by the caller.
#[cfg(feature = "server")]
fn sniff_image(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.starts_with(PNG) {
        return Some(("png", "image/png"));
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(("jpg", "image/jpeg"));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(("gif", "image/gif"));
    }
    if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(("webp", "image/webp"));
    }
    None
}

// ---------------------------------------------------------------------------
// Invitations (guest side)
// ---------------------------------------------------------------------------

/// Resolves a guest's personal token into their invitation.
#[server(endpoint = "get_invite")]
pub async fn get_invite(guest_token: String) -> Result<InviteView, ServerFnError> {
    let client = conn().await?;
    let found = crate::db::invite_by_token(&**client, &guest_token)
        .await
        .map_err(ServerFnError::new)?;
    let (_, _, mut view) = found.ok_or_else(|| ServerFnError::new(s().err_invite_not_found))?;
    view.closed = rsvp_closed(&view.event.rsvp_deadline);
    Ok(view)
}

/// Records a guest's answer. Returns the invitation as it now stands, so the
/// page shows exactly what was stored rather than what was typed.
#[server(endpoint = "submit_rsvp")]
pub async fn submit_rsvp(
    guest_token: String,
    submission: RsvpSubmission,
) -> Result<InviteView, ServerFnError> {
    use crate::types::Rsvp;

    let mut client = conn().await?;
    let found = crate::db::invite_by_token(&**client, &guest_token)
        .await
        .map_err(ServerFnError::new)?;
    let (guest_id, event_id, view) =
        found.ok_or_else(|| ServerFnError::new(s().err_invite_not_found))?;

    if rsvp_closed(&view.event.rsvp_deadline) {
        return Err(ServerFnError::new(s().err_replies_closed));
    }

    let status = if submission.attending {
        Rsvp::Attending
    } else {
        Rsvp::Declined
    };
    let party_size = if submission.attending {
        submission.party_size.clamp(1, view.max_party_size.max(1))
    } else {
        0
    };
    let note: String = submission.note.trim().chars().take(2000).collect();

    let reply = crate::db::RsvpRecord {
        guest_id,
        event_id,
        event_title: view.event.title.clone(),
        guest_name: view.guest_name.clone(),
        status,
        party_size,
        note: note.clone(),
    };
    crate::db::record_rsvp(&mut client, &reply)
        .await
        .map_err(ServerFnError::new)?;

    // Printed as well as recorded: a reply must be recoverable from container
    // logs alone, even if everything else were lost.
    println!(
        "rsvp {} ({}) for event {} {:?}: {} party={} note={:?}",
        reply.guest_name,
        guest_id,
        event_id,
        reply.event_title,
        status.as_str(),
        party_size,
        note
    );
    crate::audit::record(
        "rsvp",
        serde_json::json!({
            "event_id": event_id,
            "event_title": reply.event_title,
            "guest_id": guest_id,
            "guest_name": reply.guest_name,
            "status": status.as_str(),
            "party_size": party_size,
            "note": note,
        }),
    );

    let refreshed = crate::db::invite_by_token(&**client, &guest_token)
        .await
        .map_err(ServerFnError::new)?;
    let (_, _, mut view) = refreshed.ok_or_else(|| ServerFnError::new(s().err_invite_not_found))?;
    view.closed = rsvp_closed(&view.event.rsvp_deadline);
    Ok(view)
}

// ---------------------------------------------------------------------------
// Server-side helpers
// ---------------------------------------------------------------------------

/// Whether an RSVP deadline (`YYYY-MM-DD`, empty for none) has passed. The whole
/// deadline day counts as open.
#[cfg(feature = "server")]
fn rsvp_closed(deadline: &str) -> bool {
    let deadline = deadline.trim();
    if deadline.is_empty() {
        return false;
    }
    match chrono::NaiveDate::parse_from_str(deadline, "%Y-%m-%d") {
        Ok(d) => chrono::Utc::now().date_naive() > d,
        // An unparseable deadline must not lock guests out.
        Err(_) => false,
    }
}

/// Trims every field of an event and drops control characters from the one-line
/// fields, so a stray newline cannot break the layout of an invitation.
#[cfg(feature = "server")]
fn sanitise(mut input: EventInput) -> EventInput {
    fn one_line(s: &str) -> String {
        s.trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(500)
            .collect()
    }
    input.title = one_line(&input.title);
    input.hosts = one_line(&input.hosts);
    input.cover_image = one_line(&input.cover_image);
    input.location = one_line(&input.location);
    input.location_url = one_line(&input.location_url);
    input.starts_at = one_line(&input.starts_at);
    input.rsvp_deadline = one_line(&input.rsvp_deadline);
    input.description = input.description.trim().chars().take(4000).collect();
    input
}
