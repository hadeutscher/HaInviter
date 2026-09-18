//! PostgreSQL persistence.
//!
//! Postgres rather than an embedded database so the application can run more
//! than one replica: nothing of consequence lives in the process or on its
//! local disk. That includes uploaded images — cover photographs and venue
//! plans — which are stored here as `bytea` precisely so no shared filesystem
//! is needed, and the admin token, which every replica must agree on.
//!
//! Timestamps are stored as RFC 3339 text. They are only ever displayed,
//! exported or compared for ordering — and ISO 8601 sorts lexicographically —
//! so the extra ceremony of `timestamptz` would buy nothing here.

use crate::types::{
    EventAdminView, EventInput, EventSummary, GuestDto, InviteView, Rsvp, SeatPlacement,
};
use deadpool_postgres::{Config, Pool, Runtime};
use std::sync::OnceLock;
use tokio_postgres::{GenericClient, NoTls};

static POOL: OnceLock<Pool> = OnceLock::new();

/// Identifies the migration advisory lock. Several replicas start at once, and
/// exactly one of them may apply a migration.
const MIGRATION_LOCK: i64 = 0x4841_494E_5654_5201;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Migrations, applied in order; the highest applied index is recorded in
/// `schema_version`, so adding one is append-only.
const MIGRATIONS: &[&str] = &[
    r"
    CREATE TABLE events (
        id              BIGSERIAL PRIMARY KEY,
        title           TEXT    NOT NULL,
        hosts           TEXT    NOT NULL DEFAULT '',
        description     TEXT    NOT NULL DEFAULT '',
        cover_image     TEXT    NOT NULL DEFAULT '',
        location        TEXT    NOT NULL DEFAULT '',
        location_url    TEXT    NOT NULL DEFAULT '',
        starts_at       TEXT    NOT NULL DEFAULT '',
        rsvp_deadline   TEXT    NOT NULL DEFAULT '',
        allow_plus_ones BOOLEAN NOT NULL DEFAULT TRUE,
        created_at      TEXT    NOT NULL
    );

    CREATE TABLE guests (
        id              BIGSERIAL PRIMARY KEY,
        event_id        BIGINT  NOT NULL REFERENCES events (id) ON DELETE CASCADE,
        name            TEXT    NOT NULL,
        token           TEXT    NOT NULL UNIQUE,
        max_party_size  BIGINT  NOT NULL DEFAULT 1,
        status          TEXT    NOT NULL DEFAULT 'pending',
        party_size      BIGINT  NOT NULL DEFAULT 0,
        note            TEXT    NOT NULL DEFAULT '',
        responded_at    TEXT    NOT NULL DEFAULT '',
        created_at      TEXT    NOT NULL
    );
    CREATE INDEX guests_event_idx ON guests (event_id);
    CREATE UNIQUE INDEX guests_event_name_idx ON guests (event_id, lower(name));

    -- Append-only history of every reply ever received. Deliberately carries no
    -- foreign key: deleting a guest or an event must never erase the record of
    -- what they answered.
    CREATE TABLE rsvp_log (
        id          BIGSERIAL PRIMARY KEY,
        at          TEXT    NOT NULL,
        event_id    BIGINT  NOT NULL,
        event_title TEXT    NOT NULL,
        guest_id    BIGINT  NOT NULL,
        guest_name  TEXT    NOT NULL,
        status      TEXT    NOT NULL,
        party_size  BIGINT  NOT NULL,
        note        TEXT    NOT NULL
    );
    CREATE INDEX rsvp_log_event_idx ON rsvp_log (event_id);

    -- Cover images live in the database so that replicas need no shared
    -- filesystem. They are a handful of photographs, not a media library.
    CREATE TABLE cover_images (
        id           TEXT NOT NULL PRIMARY KEY,
        content_type TEXT NOT NULL,
        bytes        BYTEA NOT NULL,
        created_at   TEXT NOT NULL
    );

    -- Process-wide settings that must be identical across replicas; currently
    -- just the generated admin token.
    CREATE TABLE settings (
        key   TEXT NOT NULL PRIMARY KEY,
        value TEXT NOT NULL
    );
    ",
    r"
    -- Contact details imported from vCards, and the host's own record of who
    -- they have already sent to. Both default to empty, so an existing guest
    -- list needs no backfill.
    ALTER TABLE guests ADD COLUMN phone          TEXT NOT NULL DEFAULT '';
    ALTER TABLE guests ADD COLUMN invite_sent_at TEXT NOT NULL DEFAULT '';

    -- Uploaded images are no longer only cover photographs: an event also
    -- carries a plan of its venue for the seating chart to be drawn on.
    ALTER TABLE cover_images RENAME TO images;
    ALTER TABLE events ADD COLUMN venue_map TEXT NOT NULL DEFAULT '';

    -- One row per arriving person, not per guest: a party of three occupies
    -- three chairs and so has three rows. `seat_index` numbers them within the
    -- guest, and the pair is the identity of a token on the chart.
    --
    -- `event_id` is redundant with the guest's own, and deliberately so: the
    -- chart is read and rewritten a whole event at a time, and doing that
    -- through a join on every save would be the only reason the join exists.
    CREATE TABLE seats (
        guest_id   BIGINT           NOT NULL REFERENCES guests (id) ON DELETE CASCADE,
        seat_index BIGINT           NOT NULL,
        event_id   BIGINT           NOT NULL REFERENCES events (id) ON DELETE CASCADE,
        x          DOUBLE PRECISION NOT NULL,
        y          DOUBLE PRECISION NOT NULL,
        PRIMARY KEY (guest_id, seat_index)
    );
    CREATE INDEX seats_event_idx ON seats (event_id);
    ",
];

// ---------------------------------------------------------------------------
// Connecting
// ---------------------------------------------------------------------------

/// Reads the connection string. `HAINVITER_DATABASE_URL` wins; `DATABASE_URL`
/// is accepted because almost every Postgres tool sets it.
pub fn database_url() -> Result<String, String> {
    for key in ["HAINVITER_DATABASE_URL", "DATABASE_URL"] {
        if let Ok(url) = std::env::var(key)
            && !url.trim().is_empty()
        {
            return Ok(url.trim().to_owned());
        }
    }
    Err(
        "HAINVITER_DATABASE_URL is not set: HaInviter needs a PostgreSQL \
         connection string, e.g. postgres://user:password@host:5432/hainviter"
            .to_owned(),
    )
}

/// Builds the pool, verifies it can connect, and applies migrations.
pub async fn init(url: &str) -> Result<(), String> {
    let mut cfg = Config::new();
    cfg.url = Some(url.to_owned());
    let pool = cfg
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .map_err(|e| format!("cannot configure the database pool: {e}"))?;

    // Fail here rather than on the first request: a replica that cannot reach
    // its database should never report itself as ready.
    let mut client = pool
        .get()
        .await
        .map_err(|e| format!("cannot connect to the database: {e}"))?;
    migrate(&mut client)
        .await
        .map_err(|e| format!("migration failed: {e}"))?;
    drop(client);

    POOL.set(pool)
        .map_err(|_| "database already initialised".to_owned())
}

/// Checks out a connection from the pool.
pub async fn client() -> Result<deadpool_postgres::Object, String> {
    POOL.get()
        .ok_or_else(|| "database not initialised".to_owned())?
        .get()
        .await
        .map_err(|e| format!("no database connection available: {e}"))
}

async fn migrate(client: &mut deadpool_postgres::Object) -> Result<(), tokio_postgres::Error> {
    let tx = client.transaction().await?;
    // Serialises concurrent replicas; released when this transaction ends.
    tx.query("SELECT pg_advisory_xact_lock($1)", &[&MIGRATION_LOCK])
        .await?;
    tx.batch_execute("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")
        .await?;
    let applied: i32 = tx
        .query_one("SELECT COALESCE(MAX(version), 0) FROM schema_version", &[])
        .await?
        .get(0);

    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied.max(0) as usize) {
        let version = i as i32 + 1;
        tx.batch_execute(sql).await?;
        tx.execute(
            "INSERT INTO schema_version (version) VALUES ($1)",
            &[&version],
        )
        .await?;
        println!("db: applied migration {version}");
    }
    tx.commit().await
}

/// Current time as an RFC 3339 UTC string, the one timestamp format stored.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Reads a setting, storing and returning `default` if it is not there yet.
///
/// The insert is `ON CONFLICT DO NOTHING` followed by a read, so when several
/// replicas start together they all end up with the value the first one wrote.
pub async fn setting_or_insert<C: GenericClient>(
    client: &C,
    key: &str,
    default: &str,
) -> Result<String, String> {
    client
        .execute(
            "INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING",
            &[&key, &default],
        )
        .await
        .map_err(|e| e.to_string())?;
    let row = client
        .query_one("SELECT value FROM settings WHERE key = $1", &[&key])
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.get(0))
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

const EVENT_FIELDS: &str = "title, hosts, description, cover_image, location, \
                            location_url, starts_at, rsvp_deadline, allow_plus_ones, \
                            venue_map";

fn event_from_row(row: &tokio_postgres::Row, offset: usize) -> EventInput {
    EventInput {
        title: row.get(offset),
        hosts: row.get(offset + 1),
        description: row.get(offset + 2),
        cover_image: row.get(offset + 3),
        location: row.get(offset + 4),
        location_url: row.get(offset + 5),
        starts_at: row.get(offset + 6),
        rsvp_deadline: row.get(offset + 7),
        allow_plus_ones: row.get(offset + 8),
        venue_map: row.get(offset + 9),
    }
}

/// All events, soonest first, with events that have no date last.
pub async fn list_events<C: GenericClient>(client: &C) -> Result<Vec<EventSummary>, String> {
    let rows = client
        .query(
            "SELECT e.id, e.title, e.starts_at, e.location,
                    COUNT(g.id),
                    COUNT(g.id) FILTER (WHERE g.status = 'attending'),
                    COUNT(g.id) FILTER (WHERE g.status = 'declined'),
                    COUNT(g.id) FILTER (WHERE g.status = 'pending'),
                    COALESCE(SUM(g.party_size) FILTER (WHERE g.status = 'attending'), 0)::BIGINT
             FROM events e LEFT JOIN guests g ON g.event_id = e.id
             GROUP BY e.id
             ORDER BY (e.starts_at = '') ASC, e.starts_at ASC, e.id DESC",
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|row| EventSummary {
            id: row.get(0),
            title: row.get(1),
            starts_at: row.get(2),
            location: row.get(3),
            guest_count: row.get(4),
            attending: row.get(5),
            declined: row.get(6),
            pending: row.get(7),
            head_count: row.get(8),
        })
        .collect())
}

pub async fn create_event<C: GenericClient>(client: &C, input: &EventInput) -> Result<i64, String> {
    let row = client
        .query_one(
            "INSERT INTO events (title, hosts, description, cover_image, location, location_url,
                                 starts_at, rsvp_deadline, allow_plus_ones, venue_map, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
            &[
                &input.title,
                &input.hosts,
                &input.description,
                &input.cover_image,
                &input.location,
                &input.location_url,
                &input.starts_at,
                &input.rsvp_deadline,
                &input.allow_plus_ones,
                &input.venue_map,
                &now(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.get(0))
}

pub async fn update_event<C: GenericClient>(
    client: &C,
    id: i64,
    input: &EventInput,
) -> Result<u64, String> {
    client
        .execute(
            "UPDATE events SET title = $1, hosts = $2, description = $3, cover_image = $4,
                               location = $5, location_url = $6, starts_at = $7,
                               rsvp_deadline = $8, allow_plus_ones = $9, venue_map = $10
             WHERE id = $11",
            &[
                &input.title,
                &input.hosts,
                &input.description,
                &input.cover_image,
                &input.location,
                &input.location_url,
                &input.starts_at,
                &input.rsvp_deadline,
                &input.allow_plus_ones,
                &input.venue_map,
                &id,
            ],
        )
        .await
        .map_err(|e| e.to_string())
}

pub async fn delete_event<C: GenericClient>(client: &C, id: i64) -> Result<u64, String> {
    client
        .execute("DELETE FROM events WHERE id = $1", &[&id])
        .await
        .map_err(|e| e.to_string())
}

pub async fn event_title<C: GenericClient>(client: &C, id: i64) -> Result<Option<String>, String> {
    let row = client
        .query_opt("SELECT title FROM events WHERE id = $1", &[&id])
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.map(|r| r.get(0)))
}

/// An event with its full guest list, for the event admin screen.
pub async fn event_admin_view<C: GenericClient>(
    client: &C,
    id: i64,
) -> Result<Option<EventAdminView>, String> {
    let row = client
        .query_opt(
            &format!("SELECT {EVENT_FIELDS} FROM events WHERE id = $1"),
            &[&id],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(EventAdminView {
        id,
        event: event_from_row(&row, 0),
        guests: list_guests(client, id).await?,
    }))
}

// ---------------------------------------------------------------------------
// Guests
// ---------------------------------------------------------------------------

const GUEST_FIELDS: &str = "id, name, token, max_party_size, status, party_size, note, \
     responded_at, phone, invite_sent_at";

fn guest_from_row(row: &tokio_postgres::Row) -> GuestDto {
    let status: String = row.get(4);
    GuestDto {
        id: row.get(0),
        name: row.get(1),
        token: row.get(2),
        max_party_size: row.get(3),
        status: Rsvp::from_db(&status),
        party_size: row.get(5),
        note: row.get(6),
        responded_at: row.get(7),
        phone: row.get(8),
        invite_sent_at: row.get(9),
    }
}

pub async fn list_guests<C: GenericClient>(
    client: &C,
    event_id: i64,
) -> Result<Vec<GuestDto>, String> {
    let rows = client
        .query(
            &format!(
                "SELECT {GUEST_FIELDS} FROM guests WHERE event_id = $1
                 ORDER BY lower(name) ASC, id ASC"
            ),
            &[&event_id],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.iter().map(guest_from_row).collect())
}

/// A guest about to be added. Tokens are generated by the caller so this module
/// stays free of randomness concerns, and the phone arrives already normalised.
pub struct NewGuest {
    pub name: String,
    pub max_party_size: i64,
    pub token: String,
    /// E.164, or empty when the contact card carried no usable number.
    pub phone: String,
}

/// Inserts guests in one transaction, skipping names already on this event's
/// list so re-pasting a list does not create duplicates.
///
/// A name already present keeps its existing token and reply; only a missing
/// phone is filled in. That makes importing contacts over a list that was
/// pasted by name earlier additive rather than destructive, which is the order
/// hosts actually work in.
pub async fn add_guests(
    client: &mut deadpool_postgres::Object,
    event_id: i64,
    guests: &[NewGuest],
) -> Result<usize, String> {
    let tx = client.transaction().await.map_err(|e| e.to_string())?;
    let created = now();
    let mut added = 0usize;
    for guest in guests {
        // The unique index on (event_id, lower(name)) is what actually
        // guarantees this; DO NOTHING turns a re-paste into a no-op instead of
        // an error, including when two admins paste at the same moment.
        let inserted = tx
            .execute(
                "INSERT INTO guests (event_id, name, token, max_party_size, phone, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
                &[
                    &event_id,
                    &guest.name,
                    &guest.token,
                    &guest.max_party_size,
                    &guest.phone,
                    &created,
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        if inserted == 1 {
            added += 1;
            continue;
        }
        // Already invited under this name. Their link and their reply are left
        // exactly as they are — only a number we did not have is filled in, so
        // importing contacts over a list pasted by name earlier adds to it
        // rather than overwriting it. This is counted separately from `added`,
        // because the host is told how many guests they gained.
        if !guest.phone.is_empty() {
            tx.execute(
                "UPDATE guests SET phone = $1
                  WHERE event_id = $2 AND lower(name) = lower($3) AND phone = ''",
                &[&guest.phone, &event_id, &guest.name],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(added)
}

pub async fn update_guest<C: GenericClient>(
    client: &C,
    guest_id: i64,
    name: &str,
    max_party_size: i64,
    phone: &str,
) -> Result<u64, String> {
    client
        .execute(
            "UPDATE guests SET name = $1, max_party_size = $2, phone = $3 WHERE id = $4",
            &[&name, &max_party_size, &phone, &guest_id],
        )
        .await
        .map_err(|e| e.to_string())
}

/// Records that the host has sent this guest their invitation.
///
/// Sending happens in WhatsApp, outside this application entirely, so this is
/// the host's own bookkeeping rather than a delivery receipt — `at` empty marks
/// it unsent again, which is what lets the button toggle.
pub async fn set_invite_sent<C: GenericClient>(
    client: &C,
    guest_id: i64,
    at: &str,
) -> Result<u64, String> {
    client
        .execute(
            "UPDATE guests SET invite_sent_at = $1 WHERE id = $2",
            &[&at, &guest_id],
        )
        .await
        .map_err(|e| e.to_string())
}

pub async fn delete_guest<C: GenericClient>(client: &C, guest_id: i64) -> Result<u64, String> {
    client
        .execute("DELETE FROM guests WHERE id = $1", &[&guest_id])
        .await
        .map_err(|e| e.to_string())
}

/// Clears a guest's answer so they can be asked again. The original reply stays
/// in `rsvp_log`.
pub async fn reset_guest<C: GenericClient>(client: &C, guest_id: i64) -> Result<u64, String> {
    client
        .execute(
            "UPDATE guests SET status = 'pending', party_size = 0, note = '', responded_at = ''
             WHERE id = $1",
            &[&guest_id],
        )
        .await
        .map_err(|e| e.to_string())
}

/// Rotates a guest's invitation link, invalidating the old one.
pub async fn reissue_guest_token<C: GenericClient>(
    client: &C,
    guest_id: i64,
    token: &str,
) -> Result<u64, String> {
    client
        .execute(
            "UPDATE guests SET token = $1 WHERE id = $2",
            &[&token, &guest_id],
        )
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Uploaded images
// ---------------------------------------------------------------------------

pub async fn store_image<C: GenericClient>(
    client: &C,
    id: &str,
    content_type: &str,
    bytes: &[u8],
) -> Result<(), String> {
    client
        .execute(
            "INSERT INTO images (id, content_type, bytes, created_at)
             VALUES ($1, $2, $3, $4)",
            &[&id, &content_type, &bytes, &now()],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Returns `(content_type, bytes)` for a stored image.
pub async fn load_image<C: GenericClient>(
    client: &C,
    id: &str,
) -> Result<Option<(String, Vec<u8>)>, String> {
    let row = client
        .query_opt(
            "SELECT content_type, bytes FROM images WHERE id = $1",
            &[&id],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.map(|r| (r.get(0), r.get(1))))
}

// ---------------------------------------------------------------------------
// Seating
// ---------------------------------------------------------------------------

/// Every placed person on one event's seating chart.
pub async fn list_seats<C: GenericClient>(
    client: &C,
    event_id: i64,
) -> Result<Vec<SeatPlacement>, String> {
    let rows = client
        .query(
            "SELECT guest_id, seat_index, x, y FROM seats WHERE event_id = $1
             ORDER BY guest_id ASC, seat_index ASC",
            &[&event_id],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|row| SeatPlacement {
            guest_id: row.get(0),
            seat_index: row.get(1),
            x: row.get(2),
            y: row.get(3),
        })
        .collect())
}

/// Replaces an event's whole chart in one transaction.
///
/// The chart is saved as a whole rather than one token at a time because that is
/// how it is edited: one drag can move a dozen people at once, and a
/// half-applied rearrangement is not a state anyone asked for. Two admins
/// arranging the same event therefore overwrite each other wholesale — last drag
/// wins — which is the same rule the room itself follows.
///
/// A placement whose guest no longer belongs to this event is dropped rather
/// than rejected: the browser may well be working from a guest list that changed
/// underneath it, and losing one stale token is better than losing the drag.
pub async fn replace_seats(
    client: &mut deadpool_postgres::Object,
    event_id: i64,
    seats: &[SeatPlacement],
) -> Result<(), String> {
    let tx = client.transaction().await.map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM seats WHERE event_id = $1", &[&event_id])
        .await
        .map_err(|e| e.to_string())?;
    for seat in seats {
        tx.execute(
            "INSERT INTO seats (guest_id, seat_index, event_id, x, y)
             SELECT $1, $2, $3, $4, $5
             WHERE EXISTS (SELECT 1 FROM guests WHERE id = $1 AND event_id = $3)",
            &[
                &seat.guest_id,
                &seat.seat_index,
                &event_id,
                &seat.x,
                &seat.y,
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Invitations
// ---------------------------------------------------------------------------

/// Resolves a guest token into the invitation page's render model.
///
/// Returns `(guest_id, event_id, view)`.
pub async fn invite_by_token<C: GenericClient>(
    client: &C,
    token: &str,
) -> Result<Option<(i64, i64, InviteView)>, String> {
    let columns = EVENT_FIELDS
        .split(',')
        .map(|f| format!("e.{}", f.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let row = client
        .query_opt(
            &format!(
                "SELECT g.id, g.event_id, g.name, g.max_party_size, g.status, g.party_size,
                        g.note, {columns}
                 FROM guests g JOIN events e ON e.id = g.event_id
                 WHERE g.token = $1"
            ),
            &[&token],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = row else { return Ok(None) };
    let status: String = row.get(4);
    Ok(Some((
        row.get(0),
        row.get(1),
        InviteView {
            guest_name: row.get(2),
            max_party_size: row.get(3),
            status: Rsvp::from_db(&status),
            party_size: row.get(5),
            note: row.get(6),
            event: event_from_row(&row, 7),
            closed: false,
        },
    )))
}

/// One guest's reply, as it is about to be written.
///
/// The event title and guest name are copied in rather than joined at read time:
/// `rsvp_log` has to stay readable after the event or the guest is deleted.
pub struct RsvpRecord {
    pub guest_id: i64,
    pub event_id: i64,
    pub event_title: String,
    pub guest_name: String,
    pub status: Rsvp,
    pub party_size: i64,
    pub note: String,
}

/// Records a guest's answer and appends it to the reply history, atomically:
/// either both land or neither does.
pub async fn record_rsvp(
    client: &mut deadpool_postgres::Object,
    reply: &RsvpRecord,
) -> Result<(), String> {
    let at = now();
    let status = reply.status.as_str();
    let tx = client.transaction().await.map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE guests SET status = $1, party_size = $2, note = $3, responded_at = $4
         WHERE id = $5",
        &[
            &status,
            &reply.party_size,
            &reply.note,
            &at,
            &reply.guest_id,
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO rsvp_log (at, event_id, event_title, guest_id, guest_name,
                               status, party_size, note)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        &[
            &at,
            &reply.event_id,
            &reply.event_title,
            &reply.guest_id,
            &reply.guest_name,
            &status,
            &reply.party_size,
            &reply.note,
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}
