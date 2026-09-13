//! Token-based access control.
//!
//! There are no user accounts. Two kinds of unguessable token exist:
//!
//! * one **admin** token, printed to the log at startup — whoever holds the
//!   printed URL administers the system;
//! * one **guest** token per invitee, which is their personal invitation link.
//!
//! Both are 128 random bits from the operating system, hex encoded.

use std::{fmt::Write as _, sync::OnceLock};

static ADMIN_TOKEN: OnceLock<String> = OnceLock::new();

/// Key under which the generated admin token is stored in the database.
const SETTING_KEY: &str = "admin_token";

/// Generates a fresh 128-bit token as 32 lowercase hex characters.
///
/// Panics only if the OS cannot produce randomness, in which case issuing
/// guessable invitation links would be far worse than refusing to start.
pub fn new_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("operating system randomness is unavailable");
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Settles the admin token for this process, once, at startup.
///
/// `HAINVITER_ADMIN_TOKEN` wins if set. Otherwise a token is generated and
/// stored in the database rather than kept in memory: with more than one replica
/// every pod has to accept the same admin link, and the pod that printed it is
/// not necessarily the pod that serves the next request.
pub async fn resolve<C: tokio_postgres::GenericClient>(client: &C) -> Result<&'static str, String> {
    if let Some(existing) = ADMIN_TOKEN.get() {
        return Ok(existing);
    }
    let token = match std::env::var("HAINVITER_ADMIN_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t.trim().to_owned(),
        _ => crate::db::setting_or_insert(client, SETTING_KEY, &new_token()).await?,
    };
    let _ = ADMIN_TOKEN.set(token);
    Ok(ADMIN_TOKEN.get().expect("just set"))
}

/// The admin token. Empty until [`resolve`] has run, which it does before the
/// listener is bound.
pub fn admin_token() -> &'static str {
    ADMIN_TOKEN.get().map(String::as_str).unwrap_or("")
}

/// Whether `candidate` is the admin token.
///
/// Compared in constant time: the check runs on every admin request, and a
/// timing side channel would leak the token one byte at a time. An empty stored
/// token never matches, so requests that arrive before startup finishes are
/// refused rather than waved through.
pub fn is_admin(candidate: &str) -> bool {
    let expected = admin_token();
    !expected.is_empty() && constant_time_eq(candidate.as_bytes(), expected.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}
