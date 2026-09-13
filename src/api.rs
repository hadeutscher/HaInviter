//! Server functions: the entire client/server boundary.
//!
//! Every admin call takes the admin token as its first argument and is refused
//! without it — there is no session, no cookie and nothing to steal from the
//! browser beyond the link itself. Guest calls are authorised by the guest's own
//! token in exactly the same way.
//!
//! Only the signatures of these functions are compiled into the WASM bundle; the
//! bodies exist on the server alone.

use crate::types::{EventAdminView, EventInput, EventSummary, InviteView, RsvpSubmission};
use dioxus::prelude::*;

/// Largest cover image we accept, in bytes.
#[cfg(feature = "server")]
const MAX_COVER_BYTES: usize = 8 * 1024 * 1024;

/// Rejects anything that is not the admin token.
#[cfg(feature = "server")]
fn guard(token: &str) -> Result<(), ServerFnError> {
    if crate::auth::is_admin(token) {
        Ok(())
    } else {
        // Deliberately vague: the caller either has the link or does not.
        Err(ServerFnError::new("not authorised"))
    }
}

// ---------------------------------------------------------------------------
// Public
// ---------------------------------------------------------------------------

/// The externally reachable origin (e.g. `https://invites.example.com`), used to
/// build shareable invitation links. Empty when `HAINVITER_BASE_URL` is unset, in
/// which case the browser's own origin is used instead.
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
    crate::db::with_db(|c| crate::db::list_events(c))
        .await
        .map_err(ServerFnError::new)
}

#[server(endpoint = "create_event")]
pub async fn create_event(token: String, input: EventInput) -> Result<i64, ServerFnError> {
    guard(&token)?;
    let input = sanitise(input);
    if input.title.is_empty() {
        return Err(ServerFnError::new("an event needs a title"));
    }
    let title = input.title.clone();
    let id = crate::db::with_db(move |c| crate::db::create_event(c, &input))
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record(
        "event_created",
        serde_json::json!({ "event_id": id, "title": title }),
    );
    Ok(id)
}

#[server(endpoint = "update_event")]
pub async fn update_event(token: String, id: i64, input: EventInput) -> Result<(), ServerFnError> {
    guard(&token)?;
    let input = sanitise(input);
    if input.title.is_empty() {
        return Err(ServerFnError::new("an event needs a title"));
    }
    let title = input.title.clone();
    let changed = crate::db::with_db(move |c| crate::db::update_event(c, id, &input))
        .await
        .map_err(ServerFnError::new)?;
    if changed == 0 {
        return Err(ServerFnError::new("no such event"));
    }
    crate::audit::record(
        "event_updated",
        serde_json::json!({ "event_id": id, "title": title }),
    );
    Ok(())
}

#[server(endpoint = "delete_event")]
pub async fn delete_event(token: String, id: i64) -> Result<(), ServerFnError> {
    guard(&token)?;
    // Read the title first so the audit trail says what was removed. Guests
    // cascade away with the event, but their replies stay in `rsvp_log`.
    let title = crate::db::with_db(move |c| crate::db::event_title(c, id))
        .await
        .map_err(ServerFnError::new)?
        .unwrap_or_else(|| "<unknown>".to_owned());
    let guests = crate::db::with_db(move |c| crate::db::list_guests(c, id))
        .await
        .map_err(ServerFnError::new)?;
    crate::db::with_db(move |c| crate::db::delete_event(c, id))
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
    crate::db::with_db(move |c| crate::db::event_admin_view(c, id))
        .await
        .map_err(ServerFnError::new)?
        .ok_or_else(|| ServerFnError::new("no such event"))
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
        return Err(ServerFnError::new("no names found in that list"));
    }
    let rows: Vec<(String, i64, String)> = parsed
        .into_iter()
        .map(|(name, size)| (name, size, crate::auth::new_token()))
        .collect();
    let names: Vec<String> = rows.iter().map(|(n, _, _)| n.clone()).collect();
    let added = crate::db::with_db(move |c| crate::db::add_guests(c, event_id, &rows))
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record(
        "guests_added",
        serde_json::json!({ "event_id": event_id, "added": added, "names": names }),
    );
    Ok(added)
}

#[server(endpoint = "update_guest")]
pub async fn update_guest(
    token: String,
    guest_id: i64,
    name: String,
    max_party_size: i64,
) -> Result<(), ServerFnError> {
    guard(&token)?;
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(ServerFnError::new("a guest needs a name"));
    }
    let max_party_size = max_party_size.clamp(1, 50);
    let logged = name.clone();
    crate::db::with_db(move |c| crate::db::update_guest(c, guest_id, &name, max_party_size))
        .await
        .map_err(ServerFnError::new)?;
    crate::audit::record(
        "guest_updated",
        serde_json::json!({ "guest_id": guest_id, "name": logged, "max_party_size": max_party_size }),
    );
    Ok(())
}

#[server(endpoint = "delete_guest")]
pub async fn delete_guest(token: String, guest_id: i64) -> Result<(), ServerFnError> {
    guard(&token)?;
    crate::db::with_db(move |c| crate::db::delete_guest(c, guest_id))
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
    crate::db::with_db(move |c| crate::db::reset_guest(c, guest_id))
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
    let stored = fresh.clone();
    let changed = crate::db::with_db(move |c| crate::db::reissue_guest_token(c, guest_id, &stored))
        .await
        .map_err(ServerFnError::new)?;
    if changed == 0 {
        return Err(ServerFnError::new("no such guest"));
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

/// Stores an uploaded cover image on the data volume and returns the path to
/// serve it from (`/uploads/…`).
///
/// The file type comes from the bytes themselves, never from the supplied file
/// name, and the stored name is random — an upload can neither overwrite an
/// existing file nor escape the uploads directory.
#[server(endpoint = "upload_cover")]
pub async fn upload_cover(
    token: String,
    filename: String,
    bytes: Vec<u8>,
) -> Result<String, ServerFnError> {
    guard(&token)?;
    if bytes.is_empty() {
        return Err(ServerFnError::new("that file is empty"));
    }
    if bytes.len() > MAX_COVER_BYTES {
        return Err(ServerFnError::new(format!(
            "cover images must be under {} MB",
            MAX_COVER_BYTES / (1024 * 1024)
        )));
    }
    let ext = sniff_image(&bytes)
        .ok_or_else(|| ServerFnError::new("that does not look like a JPEG, PNG, GIF or WebP"))?;

    let dir = crate::db::uploads_dir();
    let name = format!("{}.{ext}", crate::auth::new_token());
    let path = dir.join(&name);
    let size = bytes.len();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, &bytes)
    })
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?
    .map_err(|e| ServerFnError::new(format!("could not store the image: {e}")))?;

    crate::audit::record(
        "cover_uploaded",
        serde_json::json!({ "stored_as": name, "original_name": filename, "bytes": size }),
    );
    Ok(format!("/uploads/{name}"))
}

/// Identifies an image from its magic bytes, returning the extension to store it
/// under. Anything unrecognised is rejected by the caller.
#[cfg(feature = "server")]
fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.starts_with(PNG) {
        return Some("png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

// ---------------------------------------------------------------------------
// Invitations (guest side)
// ---------------------------------------------------------------------------

/// Resolves a guest's personal token into their invitation.
#[server(endpoint = "get_invite")]
pub async fn get_invite(guest_token: String) -> Result<InviteView, ServerFnError> {
    let found = crate::db::with_db(move |c| crate::db::invite_by_token(c, &guest_token))
        .await
        .map_err(ServerFnError::new)?;
    let (_, _, mut view) = found.ok_or_else(|| ServerFnError::new("invitation not found"))?;
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

    let lookup = guest_token.clone();
    let found = crate::db::with_db(move |c| crate::db::invite_by_token(c, &lookup))
        .await
        .map_err(ServerFnError::new)?;
    let (guest_id, event_id, view) =
        found.ok_or_else(|| ServerFnError::new("invitation not found"))?;

    if rsvp_closed(&view.event.rsvp_deadline) {
        return Err(ServerFnError::new(
            "replies for this event have closed — please contact the hosts directly",
        ));
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
    let title = reply.event_title.clone();
    let name = reply.guest_name.clone();
    crate::db::with_db(move |c| crate::db::record_rsvp(c, &reply))
        .await
        .map_err(ServerFnError::new)?;

    // Printed as well as recorded: a reply must be recoverable from container
    // logs alone, even if the volume were lost.
    println!(
        "rsvp {} ({}) for event {} \"{}\": {} party={} note={:?}",
        name,
        guest_id,
        event_id,
        title,
        status.as_str(),
        party_size,
        note
    );
    crate::audit::record(
        "rsvp",
        serde_json::json!({
            "event_id": event_id,
            "event_title": title,
            "guest_id": guest_id,
            "guest_name": name,
            "status": status.as_str(),
            "party_size": party_size,
            "note": note,
        }),
    );

    let refreshed = crate::db::with_db(move |c| crate::db::invite_by_token(c, &guest_token))
        .await
        .map_err(ServerFnError::new)?;
    let (_, _, mut view) = refreshed.ok_or_else(|| ServerFnError::new("invitation not found"))?;
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
