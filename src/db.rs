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

/// A table the base schema creates, used to recognise a database that already
/// has that schema. No migration may rename or drop this one.
const BASE_SCHEMA_SENTINEL: &str = "events";

/// Identifies the migration advisory lock. Several replicas start at once, and
/// exactly one of them may apply a migration.
const MIGRATION_LOCK: i64 = 0x4841_494E_5654_5201;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Migrations, each identified by a name and applied at most once.
///
/// The name is the identity, not the position. Position is not safe: two
/// branches that each append a migration both take the same index, so a
/// database that ran one of them records that index and silently skips the
/// other. The build then queries columns that were never added, and the failure
/// surfaces as a 500 from whichever request touches them — a long way from the
/// cause. Names are independent of what else has landed, so migrations can be
/// written and merged in any order.
///
/// Every migration must converge when it is offered a second time. A database
/// migrated under the older numbering may have missed one and is offered it
/// again here, so "runs cleanly against a database that already has it" is the
/// requirement — which is more than writing `IF NOT EXISTS` and stopping there.
/// A migration that renames or drops something an earlier one created has to
/// check both ends, that the old object is still present *and* that the new one
/// is not, because either may already be true.
///
/// The base schema is exempt: it is never offered twice. See [`migrate`].
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001-initial-schema",
        r"
    CREATE TABLE IF NOT EXISTS events (
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

    CREATE TABLE IF NOT EXISTS guests (
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
    CREATE INDEX IF NOT EXISTS guests_event_idx ON guests (event_id);
    CREATE UNIQUE INDEX IF NOT EXISTS guests_event_name_idx ON guests (event_id, lower(name));

    -- Append-only history of every reply ever received. Deliberately carries no
    -- foreign key: deleting a guest or an event must never erase the record of
    -- what they answered.
    CREATE TABLE IF NOT EXISTS rsvp_log (
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
    CREATE INDEX IF NOT EXISTS rsvp_log_event_idx ON rsvp_log (event_id);

    -- Cover images live in the database so that replicas need no shared
    -- filesystem. They are a handful of photographs, not a media library.
    CREATE TABLE IF NOT EXISTS cover_images (
        id           TEXT NOT NULL PRIMARY KEY,
        content_type TEXT NOT NULL,
        bytes        BYTEA NOT NULL,
        created_at   TEXT NOT NULL
    );

    -- Process-wide settings that must be identical across replicas; currently
    -- just the generated admin token.
    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT NOT NULL PRIMARY KEY,
        value TEXT NOT NULL
    );
    ",
    ),
    (
        "0002-guest-contact-details",
        r"
    -- Contact details imported from vCards, and the host's own record of who
    -- they have already sent to. Both default to empty, so an existing guest
    -- list needs no backfill.
    ALTER TABLE guests ADD COLUMN IF NOT EXISTS phone          TEXT NOT NULL DEFAULT '';
    ALTER TABLE guests ADD COLUMN IF NOT EXISTS invite_sent_at TEXT NOT NULL DEFAULT '';
    ",
    ),
    (
        "0002-seating-chart",
        r"
    -- Uploaded images are no longer only cover photographs: an event also
    -- carries a plan of its venue for the seating chart to be drawn on.
    --
    -- The rename is the one statement here that cannot simply be asked for
    -- twice, so it is guarded: rename only when the old table is there and the
    -- new one is not. A database that already has `images` skips it; one that
    -- missed this migration under the old numbering gets it on the next start.
    --
    -- Both tables existing at once is not a contradiction. The base schema
    -- creates `cover_images IF NOT EXISTS`, so replaying it against a database
    -- that has already been through here puts an empty `cover_images` back
    -- beside the real `images`. Renaming onto the live table would be wrong and
    -- erroring would strand the run, so the guard simply declines; the empty
    -- table is inert and `load_image` never looks at it.
    DO $$ BEGIN
        IF EXISTS (SELECT 1 FROM information_schema.tables
                    WHERE table_schema = current_schema() AND table_name = 'cover_images')
           AND NOT EXISTS (SELECT 1 FROM information_schema.tables
                            WHERE table_schema = current_schema() AND table_name = 'images') THEN
            ALTER TABLE cover_images RENAME TO images;
        END IF;
    END $$;
    ALTER TABLE events ADD COLUMN IF NOT EXISTS venue_map TEXT NOT NULL DEFAULT '';

    -- One row per arriving person, not per guest: a party of three occupies
    -- three chairs and so has three rows. `seat_index` numbers them within the
    -- guest, and the pair is the identity of a token on the chart.
    --
    -- `event_id` is redundant with the guest's own, and deliberately so: the
    -- chart is read and rewritten a whole event at a time, and doing that
    -- through a join on every save would be the only reason the join exists.
    CREATE TABLE IF NOT EXISTS seats (
        guest_id   BIGINT           NOT NULL REFERENCES guests (id) ON DELETE CASCADE,
        seat_index BIGINT           NOT NULL,
        event_id   BIGINT           NOT NULL REFERENCES events (id) ON DELETE CASCADE,
        x          DOUBLE PRECISION NOT NULL,
        y          DOUBLE PRECISION NOT NULL,
        PRIMARY KEY (guest_id, seat_index)
    );
    CREATE INDEX IF NOT EXISTS seats_event_idx ON seats (event_id);
    ",
    ),
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

async fn migrate(client: &mut tokio_postgres::Client) -> Result<(), tokio_postgres::Error> {
    let tx = client.transaction().await?;
    // Serialises concurrent replicas; released when this transaction ends.
    tx.query("SELECT pg_advisory_xact_lock($1)", &[&MIGRATION_LOCK])
        .await?;
    tx.batch_execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             name       TEXT NOT NULL PRIMARY KEY,
             applied_at TEXT NOT NULL
         )",
    )
    .await?;

    // The base schema counts as applied whenever it is already there, whatever
    // the ledger says. That covers a database migrated under the older
    // numbering — which recorded only a count, in `schema_version` — and one
    // whose ledger has been lost or truncated.
    //
    // Deciding it from the schema rather than from a counter is what keeps the
    // base schema from ever being replayed, and replaying it would be worse
    // than useless: later migrations rename and drop the objects it creates, so
    // running it again against a live database puts an empty table back beside
    // the one that replaced it. Ruling that out here means no later migration
    // has to defend against it.
    //
    // Everything after the base is offered again, which costs nothing because
    // each migration converges — and is precisely what repairs a database that
    // silently skipped one.
    if let Some((base, _)) = MIGRATIONS.first() {
        let already_there: bool = tx
            .query_one(
                "SELECT EXISTS (
                     SELECT 1 FROM information_schema.tables
                      WHERE table_schema = current_schema() AND table_name = $1
                 )",
                &[&BASE_SCHEMA_SENTINEL],
            )
            .await?
            .get(0);
        if already_there {
            tx.execute(
                "INSERT INTO schema_migrations (name, applied_at) VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
                &[&base, &now()],
            )
            .await?;
        }
    }

    for &(name, sql) in MIGRATIONS {
        let done: bool = tx
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM schema_migrations WHERE name = $1)",
                &[&name],
            )
            .await?
            .get(0);
        if done {
            continue;
        }
        tx.batch_execute(sql).await?;
        tx.execute(
            "INSERT INTO schema_migrations (name, applied_at) VALUES ($1, $2)",
            &[&name, &now()],
        )
        .await?;
        println!("db: applied migration {name}");
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "server"))]
mod tests {
    //! Exercises the migration runner against the scratch database named by
    //! `HAINVITER_TEST_DATABASE_URL`, in a schema of its own so it cannot
    //! collide with the end-to-end test — which drops `public` wholesale.

    use super::*;

    /// Connects to the scratch database and hands back a client pointed at an
    /// empty schema of this test's own. `None` means no database is configured.
    async fn scratch(schema: &str) -> Option<tokio_postgres::Client> {
        let url = std::env::var("HAINVITER_TEST_DATABASE_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())?;
        let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .expect("the test database should be reachable");
        tokio::spawn(connection);
        client
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE;
                 CREATE SCHEMA {schema};
                 SET search_path TO {schema};"
            ))
            .await
            .expect("a schema of our own");
        Some(client)
    }

    async fn has_table(client: &tokio_postgres::Client, table: &str) -> bool {
        client
            .query_one(
                "SELECT EXISTS (
                     SELECT 1 FROM information_schema.tables
                      WHERE table_schema = current_schema() AND table_name = $1
                 )",
                &[&table],
            )
            .await
            .expect("table lookup")
            .get(0)
    }

    async fn has_column(client: &tokio_postgres::Client, table: &str, column: &str) -> bool {
        client
            .query_one(
                "SELECT EXISTS (
                     SELECT 1 FROM information_schema.columns
                      WHERE table_schema = current_schema()
                        AND table_name = $1 AND column_name = $2
                 )",
                &[&table, &column],
            )
            .await
            .expect("column lookup")
            .get(0)
    }

    /// The failure this runner exists to prevent.
    ///
    /// Two branches each appended a migration, so both claimed number 2. A
    /// database that ran the other branch's recorded "2" and skipped this one,
    /// and every later query for a guest's phone failed — a 500 from
    /// `get_event`, a long way from the cause. Naming the migrations fixes it,
    /// and, because they are idempotent, repairs such a database in place.
    #[tokio::test]
    async fn a_migration_another_branch_numbered_over_is_still_applied() {
        let Some(mut client) = scratch("migration_collision").await else {
            if std::env::var("HAINVITER_REQUIRE_DB").is_ok() {
                panic!("HAINVITER_TEST_DATABASE_URL must be set when HAINVITER_REQUIRE_DB is");
            }
            eprintln!("\n  SKIPPED: set HAINVITER_TEST_DATABASE_URL to run the migration tests.\n");
            return;
        };

        migrate(&mut client).await.expect("a clean migration");
        assert!(has_column(&client, "guests", "phone").await);

        // Rewind to a database that predates named migrations and whose
        // number 2 belonged to some other branch: the contact columns were
        // never added, but the counter claims two migrations are done.
        client
            .batch_execute(
                "DROP TABLE schema_migrations;
                 CREATE TABLE schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version (version) VALUES (1), (2);
                 ALTER TABLE guests DROP COLUMN phone, DROP COLUMN invite_sent_at;",
            )
            .await
            .expect("rewind");
        assert!(!has_column(&client, "guests", "phone").await);

        migrate(&mut client).await.expect("a repairing migration");
        assert!(
            has_column(&client, "guests", "phone").await,
            "the skipped migration must be applied rather than assumed done"
        );
        assert!(has_column(&client, "guests", "invite_sent_at").await);

        client
            .batch_execute("DROP SCHEMA migration_collision CASCADE")
            .await
            .expect("cleanup");
    }

    /// Losing the ledger must not put back what a later migration renamed away.
    ///
    /// Reported by the seating chart session. Its migration renames
    /// `cover_images` to `images`. Were the base schema replayed, its
    /// `CREATE TABLE IF NOT EXISTS cover_images` would see no such table and
    /// make an empty one beside the live `images` — and the rename, offered
    /// again in the same pass, would collide with the table it had just
    /// produced. The base schema is recognised from the schema itself precisely
    /// so that this cannot arise, whatever a later migration does to its
    /// tables.
    #[tokio::test]
    async fn a_lost_ledger_does_not_resurrect_what_a_later_migration_renamed() {
        let Some(mut client) = scratch("migration_lost_ledger").await else {
            return;
        };
        migrate(&mut client).await.expect("a clean migration");
        // The seating migration has already renamed `cover_images` to `images`,
        // so this is no longer a hypothetical.
        assert!(!has_table(&client, "cover_images").await);

        // Lose every record of what has been applied, as a restored dump or a
        // hand-repaired database might.
        client
            .batch_execute("DELETE FROM schema_migrations")
            .await
            .expect("lose the ledger");

        migrate(&mut client)
            .await
            .expect("a migration with no ledger");
        assert!(
            !has_table(&client, "cover_images").await,
            "the base schema must not be replayed over a database that has moved on"
        );
        assert!(
            has_table(&client, "images").await,
            "the live table survives"
        );

        client
            .batch_execute("DROP SCHEMA migration_lost_ledger CASCADE")
            .await
            .expect("cleanup");
    }

    /// Migrating twice must not fail, and must not apply anything twice.
    /// The seating migration renames a table, which is the one statement in it
    /// that cannot simply be asked for twice.
    ///
    /// A database repaired by the rules above is offered every migration again,
    /// so an unguarded `ALTER TABLE cover_images RENAME TO images` would abort
    /// the whole run the second time round — and the run is one transaction, so
    /// it would take every other migration down with it.
    #[tokio::test]
    async fn the_seating_migration_survives_being_offered_twice() {
        let Some(mut client) = scratch("migration_seating").await else {
            if std::env::var("HAINVITER_REQUIRE_DB").is_ok() {
                panic!("HAINVITER_TEST_DATABASE_URL must be set when HAINVITER_REQUIRE_DB is");
            }
            eprintln!("\n  SKIPPED: set HAINVITER_TEST_DATABASE_URL to run the migration tests.\n");
            return;
        };

        migrate(&mut client).await.expect("a clean migration");
        assert!(has_table(&client, "images").await);
        assert!(!has_table(&client, "cover_images").await);

        // Forget what has been applied, as a database recovering from the old
        // numbering effectively has: every migration is offered again.
        client
            .batch_execute("DELETE FROM schema_migrations")
            .await
            .expect("rewind");

        migrate(&mut client)
            .await
            .expect("offering the seating migration again must not fail on the rename");
        assert!(has_table(&client, "images").await);
        assert!(has_column(&client, "events", "venue_map").await);
        assert!(has_table(&client, "seats").await);

        client
            .batch_execute("DROP SCHEMA migration_seating CASCADE")
            .await
            .expect("cleanup");
    }

    #[tokio::test]
    async fn migrating_an_up_to_date_database_does_nothing() {
        let Some(mut client) = scratch("migration_idempotent").await else {
            return;
        };

        migrate(&mut client).await.expect("first");
        migrate(&mut client).await.expect("second");
        let applied: i64 = client
            .query_one("SELECT COUNT(*) FROM schema_migrations", &[])
            .await
            .expect("count")
            .get(0);
        assert_eq!(applied as usize, MIGRATIONS.len());

        client
            .batch_execute("DROP SCHEMA migration_idempotent CASCADE")
            .await
            .expect("cleanup");
    }
}
