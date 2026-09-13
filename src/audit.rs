//! Audit logging.
//!
//! Losing guest replies is the worst thing this system could do, so every reply
//! is written in three independent places: the `guests` row, the append-only
//! `rsvp_log` table, and — here — a JSON-lines file on the data volume plus a
//! line on stdout for whatever collects container logs.

use crate::db;
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

/// Serialises writers so two concurrent replies cannot interleave mid-line.
fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Path of the append-only audit log on the data volume.
pub fn log_path() -> PathBuf {
    db::data_dir().join("audit.log")
}

/// Appends one event to the audit log and echoes it to stdout.
///
/// A failure to write the file is reported loudly but never propagated: the
/// caller has already committed the change to the database, and refusing the
/// request at that point would be a lie.
pub fn record(kind: &str, detail: serde_json::Value) {
    let line = serde_json::json!({
        "at": db::now(),
        "kind": kind,
        "detail": detail,
    });
    let rendered = line.to_string();

    println!("audit {rendered}");

    let path = log_path();
    let _guard = write_lock().lock();
    let result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| {
            f.write_all(rendered.as_bytes())?;
            f.write_all(b"\n")?;
            // Guest replies must be on disk before we move on; the volume of
            // writes here is a handful per guest, so the sync costs nothing.
            f.flush()?;
            f.sync_data()
        });
    if let Err(e) = result {
        eprintln!(
            "audit: FAILED to append to {}: {e} (entry was: {rendered})",
            path.display()
        );
    }
}
