//! Token-based access control.
//!
//! There are no user accounts. Two kinds of unguessable token exist:
//!
//! * one **admin** token, generated at startup and printed to the log — whoever
//!   holds the printed URL administers the system;
//! * one **guest** token per invitee, which is their personal invitation link.
//!
//! Both are 128 random bits from the operating system, hex encoded.

use std::{fmt::Write as _, sync::OnceLock};

static ADMIN_TOKEN: OnceLock<String> = OnceLock::new();

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

/// Returns the admin token, generating it on first call.
///
/// `HAINVITER_ADMIN_TOKEN` overrides generation, which is what you want when the
/// admin link should survive a pod restart; leave it unset to get a fresh token
/// (printed to the log) on every startup.
pub fn admin_token() -> &'static str {
    ADMIN_TOKEN.get_or_init(|| match std::env::var("HAINVITER_ADMIN_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t.trim().to_owned(),
        _ => new_token(),
    })
}

/// Whether `candidate` is the admin token.
///
/// Compared in constant time: the check runs on every admin request, and a
/// timing side channel would leak the token one byte at a time.
pub fn is_admin(candidate: &str) -> bool {
    constant_time_eq(candidate.as_bytes(), admin_token().as_bytes())
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
