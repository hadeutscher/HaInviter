//! SQLite persistence.
//!
//! One process, one small database file, so a single connection behind a mutex
//! is both sufficient and the leanest option available — there is no pool, no
//! background worker and no network database to keep alive on a Raspberry Pi.
//! Blocking work is pushed onto Tokio's blocking pool by [`with_db`].

use crate::types::{EventAdminView, EventInput, EventSummary, GuestDto, InviteView, Rsvp};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

/// Shared handle to the one and only connection.
pub type Db = Arc<Mutex<Connection>>;

static DB: OnceLock<Db> = OnceLock::new();

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Directory holding every piece of state we must not lose: the database, the
/// audit log and uploaded cover images. Mounted as a host volume in production.
pub fn data_dir() -> PathBuf {
    std::env::var_os("HAINVITER_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data"))
}

/// Where uploaded cover images are stored and served from.
pub fn uploads_dir() -> PathBuf {
    data_dir().join("uploads")
}

/// Path of the SQLite database file.
pub fn db_path() -> PathBuf {
    data_dir().join("hainviter.db")
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Schema migrations, applied in order. The index of the last applied entry is
/// stored in SQLite's own `user_version`, so adding a migration is append-only.
const MIGRATIONS: &[&str] = &[r"
    CREATE TABLE events (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        title           TEXT    NOT NULL,
        hosts           TEXT    NOT NULL DEFAULT '',
        description     TEXT    NOT NULL DEFAULT '',
        cover_image     TEXT    NOT NULL DEFAULT '',
        location        TEXT    NOT NULL DEFAULT '',
        location_url    TEXT    NOT NULL DEFAULT '',
        starts_at       TEXT    NOT NULL DEFAULT '',
        rsvp_deadline   TEXT    NOT NULL DEFAULT '',
        allow_plus_ones INTEGER NOT NULL DEFAULT 1,
        created_at      TEXT    NOT NULL
    );

    CREATE TABLE guests (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        event_id        INTEGER NOT NULL REFERENCES events(id) ON DELETE CASCADE,
        name            TEXT    NOT NULL,
        token           TEXT    NOT NULL UNIQUE,
        max_party_size  INTEGER NOT NULL DEFAULT 1,
        status          TEXT    NOT NULL DEFAULT 'pending',
        party_size      INTEGER NOT NULL DEFAULT 0,
        note            TEXT    NOT NULL DEFAULT '',
        responded_at    TEXT    NOT NULL DEFAULT '',
        created_at      TEXT    NOT NULL
    );
    CREATE INDEX guests_event_idx ON guests (event_id);

    -- Append-only history of every reply ever received. Deliberately carries no
    -- foreign key: deleting a guest or an event must never erase the record of
    -- what they answered.
    CREATE TABLE rsvp_log (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        at          TEXT    NOT NULL,
        event_id    INTEGER NOT NULL,
        event_title TEXT    NOT NULL,
        guest_id    INTEGER NOT NULL,
        guest_name  TEXT    NOT NULL,
        status      TEXT    NOT NULL,
        party_size  INTEGER NOT NULL,
        note        TEXT    NOT NULL
    );
    CREATE INDEX rsvp_log_event_idx ON rsvp_log (event_id);
    "];

/// Opens (creating if needed) the database at `path`, applies migrations and
/// installs it as the process-wide handle.
pub fn init(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut conn = Connection::open(path).map_err(|e| e.to_string())?;

    // WAL survives an unclean shutdown and keeps readers off the writer's back;
    // synchronous=FULL costs a little throughput per RSVP and buys durability
    // against power loss, which is exactly the right trade here.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )
    .map_err(|e| e.to_string())?;

    migrate(&mut conn).map_err(|e| format!("migration failed: {e}"))?;

    DB.set(Arc::new(Mutex::new(conn)))
        .map_err(|_| "database already initialised".to_owned())
}

fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let applied: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let applied = applied.max(0) as usize;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        // PRAGMA does not accept bound parameters.
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
        tx.commit()?;
        println!("db: applied migration {}", i + 1);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

/// Runs `f` against the database on Tokio's blocking pool.
///
/// Errors are stringified because they cross a `spawn_blocking` boundary on
/// their way into a `ServerFnError`, and no caller can act on the distinction.
pub async fn with_db<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let db = DB
        .get()
        .ok_or_else(|| "database not initialised".to_owned())?
        .clone();
    tokio::task::spawn_blocking(move || {
        let mut guard = db.lock().map_err(|_| "database lock poisoned".to_owned())?;
        f(&mut guard).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("database task failed: {e}"))?
}

/// Current time as an RFC 3339 UTC string, the one timestamp format stored.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

const EVENT_FIELDS: &str = "title, hosts, description, cover_image, location, \
                            location_url, starts_at, rsvp_deadline, allow_plus_ones";

fn event_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<EventInput> {
    Ok(EventInput {
        title: row.get(offset)?,
        hosts: row.get(offset + 1)?,
        description: row.get(offset + 2)?,
        cover_image: row.get(offset + 3)?,
        location: row.get(offset + 4)?,
        location_url: row.get(offset + 5)?,
        starts_at: row.get(offset + 6)?,
        rsvp_deadline: row.get(offset + 7)?,
        allow_plus_ones: row.get::<_, i64>(offset + 8)? != 0,
    })
}

/// All events, soonest first, with events that have no date last.
pub fn list_events(conn: &Connection) -> rusqlite::Result<Vec<EventSummary>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, e.title, e.starts_at, e.location,
                (SELECT COUNT(*) FROM guests g WHERE g.event_id = e.id),
                (SELECT COUNT(*) FROM guests g WHERE g.event_id = e.id AND g.status = 'attending'),
                (SELECT COUNT(*) FROM guests g WHERE g.event_id = e.id AND g.status = 'declined'),
                (SELECT COUNT(*) FROM guests g WHERE g.event_id = e.id AND g.status = 'pending'),
                (SELECT COALESCE(SUM(g.party_size), 0) FROM guests g
                  WHERE g.event_id = e.id AND g.status = 'attending')
         FROM events e
         ORDER BY (e.starts_at = '') ASC, e.starts_at ASC, e.id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(EventSummary {
            id: row.get(0)?,
            title: row.get(1)?,
            starts_at: row.get(2)?,
            location: row.get(3)?,
            guest_count: row.get(4)?,
            attending: row.get(5)?,
            declined: row.get(6)?,
            pending: row.get(7)?,
            head_count: row.get(8)?,
        })
    })?;
    rows.collect()
}

pub fn create_event(conn: &Connection, input: &EventInput) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO events (title, hosts, description, cover_image, location, location_url,
                             starts_at, rsvp_deadline, allow_plus_ones, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            input.title,
            input.hosts,
            input.description,
            input.cover_image,
            input.location,
            input.location_url,
            input.starts_at,
            input.rsvp_deadline,
            i64::from(input.allow_plus_ones),
            now(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_event(conn: &Connection, id: i64, input: &EventInput) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE events SET title = ?1, hosts = ?2, description = ?3, cover_image = ?4,
                           location = ?5, location_url = ?6, starts_at = ?7,
                           rsvp_deadline = ?8, allow_plus_ones = ?9
         WHERE id = ?10",
        params![
            input.title,
            input.hosts,
            input.description,
            input.cover_image,
            input.location,
            input.location_url,
            input.starts_at,
            input.rsvp_deadline,
            i64::from(input.allow_plus_ones),
            id,
        ],
    )
}

pub fn delete_event(conn: &Connection, id: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM events WHERE id = ?1", params![id])
}

pub fn event_title(conn: &Connection, id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT title FROM events WHERE id = ?1", params![id], |r| {
        r.get(0)
    })
    .optional()
}

/// An event with its full guest list, for the event admin screen.
pub fn event_admin_view(conn: &Connection, id: i64) -> rusqlite::Result<Option<EventAdminView>> {
    let event = conn
        .query_row(
            &format!("SELECT {EVENT_FIELDS} FROM events WHERE id = ?1"),
            params![id],
            |row| event_from_row(row, 0),
        )
        .optional()?;
    let Some(event) = event else {
        return Ok(None);
    };
    Ok(Some(EventAdminView {
        id,
        event,
        guests: list_guests(conn, id)?,
    }))
}

// ---------------------------------------------------------------------------
// Guests
// ---------------------------------------------------------------------------

fn guest_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GuestDto> {
    let status: String = row.get(4)?;
    Ok(GuestDto {
        id: row.get(0)?,
        name: row.get(1)?,
        token: row.get(2)?,
        max_party_size: row.get(3)?,
        status: Rsvp::from_db(&status),
        party_size: row.get(5)?,
        note: row.get(6)?,
        responded_at: row.get(7)?,
    })
}

const GUEST_FIELDS: &str =
    "id, name, token, max_party_size, status, party_size, note, responded_at";

pub fn list_guests(conn: &Connection, event_id: i64) -> rusqlite::Result<Vec<GuestDto>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {GUEST_FIELDS} FROM guests WHERE event_id = ?1
         ORDER BY name COLLATE NOCASE ASC, id ASC"
    ))?;
    let rows = stmt.query_map(params![event_id], guest_from_row)?;
    rows.collect()
}

/// Inserts guests in one transaction, skipping names already on this event's
/// list so re-pasting a list does not create duplicates.
///
/// `guests` carries `(name, max_party_size, token)`; tokens are generated by the
/// caller so this module stays free of randomness concerns.
pub fn add_guests(
    conn: &mut Connection,
    event_id: i64,
    guests: &[(String, i64, String)],
) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    let mut added = 0usize;
    {
        let mut exists = tx.prepare(
            "SELECT 1 FROM guests WHERE event_id = ?1 AND name = ?2 COLLATE NOCASE LIMIT 1",
        )?;
        let mut insert = tx.prepare(
            "INSERT INTO guests (event_id, name, token, max_party_size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        let created = now();
        for (name, max_party_size, token) in guests {
            let already: Option<i64> = exists
                .query_row(params![event_id, name], |r| r.get(0))
                .optional()?;
            if already.is_some() {
                continue;
            }
            insert.execute(params![event_id, name, token, max_party_size, created])?;
            added += 1;
        }
    }
    tx.commit()?;
    Ok(added)
}

pub fn update_guest(
    conn: &Connection,
    guest_id: i64,
    name: &str,
    max_party_size: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE guests SET name = ?1, max_party_size = ?2 WHERE id = ?3",
        params![name, max_party_size, guest_id],
    )
}

pub fn delete_guest(conn: &Connection, guest_id: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM guests WHERE id = ?1", params![guest_id])
}

/// Clears a guest's answer so they can be asked again. The original reply stays
/// in `rsvp_log`.
pub fn reset_guest(conn: &Connection, guest_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE guests SET status = 'pending', party_size = 0, note = '', responded_at = ''
         WHERE id = ?1",
        params![guest_id],
    )
}

/// Rotates a guest's invitation link, invalidating the old one.
pub fn reissue_guest_token(
    conn: &Connection,
    guest_id: i64,
    token: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE guests SET token = ?1 WHERE id = ?2",
        params![token, guest_id],
    )
}

// ---------------------------------------------------------------------------
// Invitations
// ---------------------------------------------------------------------------

/// Resolves a guest token into the invitation page's render model.
///
/// Returns `(guest_id, event_id, view)`.
pub fn invite_by_token(
    conn: &Connection,
    token: &str,
) -> rusqlite::Result<Option<(i64, i64, InviteView)>> {
    conn.query_row(
        &format!(
            "SELECT g.id, g.event_id, g.name, g.max_party_size, g.status, g.party_size, g.note,
                    {}
             FROM guests g JOIN events e ON e.id = g.event_id
             WHERE g.token = ?1",
            EVENT_FIELDS
                .split(", ")
                .map(|f| format!("e.{}", f.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        params![token],
        |row| {
            let status: String = row.get(4)?;
            let event = event_from_row(row, 7)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                InviteView {
                    guest_name: row.get(2)?,
                    max_party_size: row.get(3)?,
                    status: Rsvp::from_db(&status),
                    party_size: row.get(5)?,
                    note: row.get(6)?,
                    event,
                    closed: false,
                },
            ))
        },
    )
    .optional()
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
pub fn record_rsvp(conn: &mut Connection, reply: &RsvpRecord) -> rusqlite::Result<()> {
    let at = now();
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE guests SET status = ?1, party_size = ?2, note = ?3, responded_at = ?4
         WHERE id = ?5",
        params![
            reply.status.as_str(),
            reply.party_size,
            reply.note,
            at,
            reply.guest_id
        ],
    )?;
    tx.execute(
        "INSERT INTO rsvp_log (at, event_id, event_title, guest_id, guest_name,
                               status, party_size, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            at,
            reply.event_id,
            reply.event_title,
            reply.guest_id,
            reply.guest_name,
            reply.status.as_str(),
            reply.party_size,
            reply.note
        ],
    )?;
    tx.commit()
}
