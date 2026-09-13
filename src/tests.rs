//! End-to-end exercise of the server half: the database, the server functions,
//! the authorisation check and the audit log, in the order a real event goes
//! through them.
//!
//! Everything shares one process-wide database handle, so this is deliberately a
//! single test rather than several racing for it.

use crate::{
    api, audit, db, export,
    types::{EventInput, Rsvp, RsvpSubmission},
};

const ADMIN: &str = "test-admin-token";

#[tokio::test]
async fn an_event_can_be_created_invited_to_and_answered() {
    let dir = std::env::temp_dir().join(format!("hainviter-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    // SAFETY: set before anything reads the environment, and this test owns the
    // process.
    unsafe {
        std::env::set_var("HAINVITER_DATA_DIR", &dir);
        std::env::set_var("HAINVITER_ADMIN_TOKEN", ADMIN);
    }

    db::init(&db::db_path()).expect("database should initialise");
    let admin = ADMIN.to_owned();

    // ── An event ───────────────────────────────────────────────────────────
    let event_id = api::create_event(
        admin.clone(),
        EventInput {
            title: "  Dana & Yuval's Wedding  ".to_owned(),
            location: "Tel Aviv".to_owned(),
            starts_at: "2026-09-12T19:30".to_owned(),
            allow_plus_ones: true,
            ..EventInput::default()
        },
    )
    .await
    .expect("event should be created");

    let listed = api::list_events(admin.clone()).await.expect("list");
    assert_eq!(listed.len(), 1);
    // Fields are trimmed on the way in.
    assert_eq!(listed[0].title, "Dana & Yuval's Wedding");

    // An event with no title is refused.
    assert!(
        api::create_event(admin.clone(), EventInput::default())
            .await
            .is_err()
    );

    // ── A guest list ───────────────────────────────────────────────────────
    let added = api::add_guests(
        admin.clone(),
        event_id,
        "Dana, 2\nYuval\n\nDana\n".to_owned(),
        1,
    )
    .await
    .expect("guests should be added");
    // "Dana" appears twice in the paste and is added once.
    assert_eq!(added, 2);

    // Re-pasting the same list adds nobody.
    let again = api::add_guests(admin.clone(), event_id, "Dana\nYuval".to_owned(), 1)
        .await
        .expect("second paste should be accepted");
    assert_eq!(again, 0);

    let view = api::get_event(admin.clone(), event_id)
        .await
        .expect("event");
    assert_eq!(view.guests.len(), 2);
    let dana = view
        .guests
        .iter()
        .find(|g| g.name == "Dana")
        .expect("Dana should be invited")
        .clone();
    assert_eq!(dana.max_party_size, 2);
    assert_eq!(dana.status, Rsvp::Pending);
    assert_eq!(dana.token.len(), 32);

    // ── The invitation ─────────────────────────────────────────────────────
    let invite = api::get_invite(dana.token.clone())
        .await
        .expect("invitation should resolve");
    assert_eq!(invite.guest_name, "Dana");
    assert_eq!(invite.event.title, "Dana & Yuval's Wedding");
    assert!(!invite.closed);

    assert!(
        api::get_invite("0".repeat(32)).await.is_err(),
        "an unknown token must not resolve"
    );

    // ── The reply ──────────────────────────────────────────────────────────
    let answered = api::submit_rsvp(
        dana.token.clone(),
        RsvpSubmission {
            attending: true,
            // More than her invitation covers: must be clamped, not rejected.
            party_size: 9,
            note: "  no nuts please  ".to_owned(),
        },
    )
    .await
    .expect("reply should be recorded");
    assert_eq!(answered.status, Rsvp::Attending);
    assert_eq!(answered.party_size, 2);
    assert_eq!(answered.note, "no nuts please");

    let summary = &api::list_events(admin.clone()).await.expect("list")[0];
    assert_eq!(summary.attending, 1);
    assert_eq!(summary.pending, 1);
    assert_eq!(summary.head_count, 2);

    // Declining clears the party size.
    let declined = api::submit_rsvp(
        dana.token.clone(),
        RsvpSubmission {
            attending: false,
            party_size: 2,
            note: String::new(),
        },
    )
    .await
    .expect("a guest may change their mind");
    assert_eq!(declined.status, Rsvp::Declined);
    assert_eq!(declined.party_size, 0);

    // ── Authorisation ──────────────────────────────────────────────────────
    for wrong in ["", "nope", &"f".repeat(32)] {
        assert!(
            api::list_events(wrong.to_owned()).await.is_err(),
            "a wrong admin token must be refused"
        );
    }
    assert!(
        api::delete_event("nope".to_owned(), event_id)
            .await
            .is_err(),
        "a wrong admin token must not delete anything"
    );
    assert_eq!(
        api::list_events(admin.clone()).await.expect("list").len(),
        1
    );

    // ── Rotating a link invalidates the old one ────────────────────────────
    let fresh = api::reissue_invite(admin.clone(), dana.id)
        .await
        .expect("a new link should be issued");
    assert_ne!(fresh, dana.token);
    assert!(api::get_invite(dana.token.clone()).await.is_err());
    assert!(api::get_invite(fresh.clone()).await.is_ok());

    // ── The export ─────────────────────────────────────────────────────────
    let view = api::get_event(admin.clone(), event_id)
        .await
        .expect("event");
    let csv = export::responses_csv(&view.guests, "https://invites.test");
    assert!(csv.contains("Dana,declined"));
    assert!(csv.contains(&format!("https://invites.test/i/{fresh}")));

    // ── The audit trail ────────────────────────────────────────────────────
    let log = std::fs::read_to_string(audit::log_path()).expect("audit log should exist");
    assert!(log.contains(r#""kind":"event_created""#));
    assert!(log.contains(r#""kind":"guests_added""#));
    assert!(log.contains(r#""kind":"rsvp""#));
    assert!(log.contains("no nuts please"));
    // Both of Dana's answers are kept, not just the latest.
    assert_eq!(log.matches(r#""kind":"rsvp""#).count(), 2);

    // Deleting the event keeps the reply history.
    api::delete_event(admin.clone(), event_id)
        .await
        .expect("event should be deleted");
    assert!(
        api::list_events(admin.clone())
            .await
            .expect("list")
            .is_empty()
    );
    let history: i64 =
        db::with_db(|c| c.query_row("SELECT COUNT(*) FROM rsvp_log", [], |r| r.get(0)))
            .await
            .expect("reply history should still be readable");
    assert_eq!(history, 2);

    let _ = std::fs::remove_dir_all(&dir);
}
