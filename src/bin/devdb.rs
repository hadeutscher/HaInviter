//! Development helper: runs an embedded PostgreSQL and hands its URL to a child
//! command.
//!
//! `dx serve` needs a database, and asking every contributor to install and run
//! PostgreSQL for that is a poor trade. This downloads a real PostgreSQL once,
//! keeps it in a directory of its own, and starts it on a free port:
//!
//! ```text
//! cargo dev dx serve          # the alias in .cargo/config.toml
//! ```
//!
//! It is a real PostgreSQL rather than SQLite or a mock on purpose. A second
//! backend would mean every query written twice in two dialects, and the failure
//! mode of that is local development passing while production breaks.
//!
//! This binary requires the `dev-db` feature, so it is never built for the
//! container image.

use postgresql_embedded::{PostgreSQL, Settings};
use std::{path::PathBuf, process::Command};

/// The database created inside the embedded server.
const DATABASE: &str = "hainviter";

/// Selects Zonky's PostgreSQL builds rather than the crate's default ones.
///
/// The defaults link against `libxml2.so.2`, and current distributions ship
/// `.so.16`, so the server refuses to start with a dynamic-linker error. Zonky's
/// builds carry what they need. This is a sentinel the crate recognises; it
/// resolves the actual per-platform artifact itself.
const ZONKY_RELEASES: &str = "https://github.com/zonkyio/embedded-postgres-binaries";

/// Superuser password for the local database.
///
/// Fixed rather than generated: the settings default to a fresh random password
/// every run, which authenticates against a freshly initialised data directory
/// and then fails against the one from last time. This database listens on
/// localhost on a random port and holds nothing but test data.
const PASSWORD: &str = "hainviter-dev";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("devdb: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<std::process::ExitCode, Box<dyn std::error::Error>> {
    // Persistent by default: losing the test guest list on every restart would
    // make the helper more annoying than installing PostgreSQL.
    let base = std::env::var_os("HAINVITER_DEV_DB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("hainviter-devdb"));

    let mut settings = Settings::new();
    settings.releases_url = ZONKY_RELEASES.to_owned();
    settings.temporary = false;
    settings.installation_dir = base.join("postgresql");
    settings.data_dir = base.join("data");
    settings.password = PASSWORD.to_owned();
    // A free port is chosen for us; the URL is passed to the child, so nothing
    // depends on which one.
    settings.port = 0;

    let mut postgresql = PostgreSQL::new(settings);

    // Unconditional: which directory the binaries land in depends on the version
    // the requirement resolves to, so there is no reliable way to tell in advance
    // whether this run will download or reuse.
    println!(
        "devdb: preparing PostgreSQL in {} (the first run downloads about 100 MB)",
        base.display()
    );
    postgresql.setup().await?;
    postgresql.start().await?;

    if !postgresql.database_exists(DATABASE).await? {
        postgresql.create_database(DATABASE).await?;
        println!("devdb: created database {DATABASE}");
    }

    let url = postgresql.settings().url(DATABASE);
    println!("devdb: PostgreSQL ready at {url}");

    let mut args = std::env::args_os().skip(1);
    let code = match args.next() {
        Some(program) => {
            // Hand the URL to the child rather than making the caller copy it.
            let status = Command::new(&program)
                .args(args)
                .env("HAINVITER_DATABASE_URL", &url)
                .status();
            match status {
                Ok(status) => {
                    if status.success() {
                        std::process::ExitCode::SUCCESS
                    } else {
                        std::process::ExitCode::FAILURE
                    }
                }
                Err(e) => {
                    eprintln!("devdb: cannot run {}: {e}", program.to_string_lossy());
                    std::process::ExitCode::FAILURE
                }
            }
        }
        None => {
            println!();
            println!("devdb: export this and start the server in another terminal:");
            println!();
            println!("    HAINVITER_DATABASE_URL={url} dx serve");
            println!();
            println!("devdb: Ctrl-C to stop the database.");
            // Exiting here would stop the server and make the URL useless.
            let _ = tokio::signal::ctrl_c().await;
            println!();
            std::process::ExitCode::SUCCESS
        }
    };

    postgresql.stop().await?;
    println!(
        "devdb: database stopped (its data is kept in {})",
        base.display()
    );
    Ok(code)
}
