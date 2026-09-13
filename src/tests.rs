//! End-to-end exercise of the server half: the database, the server functions,
//! the authorisation check and the audit log, in the order a real event goes
//! through them.
//!
//! Needs a PostgreSQL database, named by `HAINVITER_TEST_DATABASE_URL` —
//! deliberately *not* the variable the application reads, because the first
//! thing this test does is drop every table in the schema.
//!
//! Everything shares one process-wide pool, so this is a single test rather than
//! several racing for it.

use crate::{
    api, audit, db, export,
    i18n::Locale,
    types::{EventInput, Rsvp, RsvpSubmission},
};

const ADMIN: &str = "test-admin-token";

#[tokio::test]
async fn an_event_can_be_created_invited_to_and_answered() {
    let Some(url) = std::env::var("HAINVITER_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    else {
        // Loud rather than silent, and fatal where a database is guaranteed, so
        // a broken CI service cannot look like a pass.
        if std::env::var("HAINVITER_REQUIRE_DB").is_ok() {
            panic!("HAINVITER_TEST_DATABASE_URL must be set when HAINVITER_REQUIRE_DB is");
        }
        eprintln!(
            "\n  SKIPPED: set HAINVITER_TEST_DATABASE_URL to a scratch PostgreSQL database to \
             run the end-to-end test (its schema is dropped).\n"
        );
        return;
    };

    let dir = std::env::temp_dir().join(format!("hainviter-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");

    // SAFETY: set before anything reads the environment, and this test owns the
    // process.
    unsafe {
        std::env::set_var("HAINVITER_DATA_DIR", &dir);
        std::env::set_var("HAINVITER_ADMIN_TOKEN", ADMIN);
        std::env::set_var("HAINVITER_LOCALE", "en-US");
    }

    reset_schema(&url).await;
    db::init(&url).await.expect("database should initialise");
    {
        let client = db::client().await.expect("a connection");
        crate::auth::resolve(&**client)
            .await
            .expect("admin token should resolve");
    }
    let admin = ADMIN.to_owned();

    assert_eq!(api::locale().await.expect("locale"), Locale::EnUs);

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
    assert_eq!(listed[0].guest_count, 0);

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

    // Re-pasting the same list adds nobody — and must not error on the unique
    // index either.
    let again = api::add_guests(admin.clone(), event_id, "Dana\nYuval".to_owned(), 1)
        .await
        .expect("second paste should be accepted");
    assert_eq!(again, 0);

    // Case differences are the same name.
    let mixed_case = api::add_guests(admin.clone(), event_id, "DANA".to_owned(), 1)
        .await
        .expect("third paste should be accepted");
    assert_eq!(mixed_case, 0);

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

    // ── Cover images live in the database ──────────────────────────────────
    let png: Vec<u8> = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        .into_iter()
        .chain(*b"not really a png body")
        .collect();
    let path = api::upload_cover(admin.clone(), "cover.png".to_owned(), png.clone())
        .await
        .expect("a PNG should be accepted");
    assert!(path.starts_with("/uploads/") && path.ends_with(".png"));
    let stored_id = path.trim_start_matches("/uploads/").to_owned();
    {
        let client = db::client().await.expect("a connection");
        let (content_type, bytes) = db::load_cover(&**client, &stored_id)
            .await
            .expect("load")
            .expect("the image should be there");
        assert_eq!(content_type, "image/png");
        assert_eq!(bytes, png);
    }
    // Anything that is not an image is refused on its bytes, not its name.
    assert!(
        api::upload_cover(admin.clone(), "evil.png".to_owned(), b"#!/bin/sh".to_vec())
            .await
            .is_err()
    );

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
    let log = std::fs::read_to_string(audit::log_path().expect("a configured log path"))
        .expect("audit log should exist");
    assert!(log.contains(r#""kind":"event_created""#));
    assert!(log.contains(r#""kind":"guests_added""#));
    assert!(log.contains(r#""kind":"cover_uploaded""#));
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
    {
        let client = db::client().await.expect("a connection");
        let history: i64 = client
            .query_one("SELECT COUNT(*) FROM rsvp_log", &[])
            .await
            .expect("reply history should still be readable")
            .get(0);
        assert_eq!(history, 2);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Drops and recreates the schema so the test starts from nothing.
async fn reset_schema(url: &str) {
    let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .expect("the test database should be reachable");
    let handle = tokio::spawn(connection);
    client
        .batch_execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .await
        .expect("schema should be resettable");
    drop(client);
    let _ = handle.await;
}
