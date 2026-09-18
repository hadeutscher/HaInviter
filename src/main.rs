//! HaInviter — guest invitations and RSVP tracking.
//!
//! One binary serves three things: the admin panel, the per-guest invitation
//! pages, and the WASM bundle that drives both. The same crate is compiled twice
//! — once for `wasm32-unknown-unknown` (`--features web`) and once natively
//! (`--features server`) — so the page components and the DTOs are shared and can
//! never drift apart.
//!
//! Nothing of consequence lives in the process or on its local disk: state is in
//! PostgreSQL, which is what lets the deployment run more than one replica.

mod api;
mod contacts;
mod i18n;
mod types;
mod ui;

#[cfg(feature = "server")]
mod audit;
#[cfg(feature = "server")]
mod auth;
#[cfg(feature = "server")]
mod db;
#[cfg(feature = "server")]
mod export;

#[cfg(all(test, feature = "server"))]
mod tests;

use dioxus::prelude::*;
use i18n::Locale;
use ui::{AdminEvent, AdminEvents, Home, Invite, NotFound};

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// Every page in the application.
///
/// Both admin routes carry the admin token in the path: there is no session, so
/// the URL *is* the credential and must travel with every navigation.
#[derive(Clone, Debug, PartialEq, Routable)]
pub enum Route {
    #[route("/")]
    Home {},

    /// A guest's personal invitation.
    #[route("/i/:token")]
    Invite { token: String },

    #[route("/admin/:token")]
    AdminEvents { token: String },

    #[route("/admin/:token/e/:event_id")]
    AdminEvent { token: String, event_id: i64 },

    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

/// Root component: the locale, the global assets, and the router.
#[component]
fn App() -> Element {
    // Resolved through a server function rather than read on each side
    // separately, and awaited before the first render: the server renders with
    // this locale, and a client that hydrated with a different one would
    // mismatch every string on the page.
    let resolved = use_server_future(api::locale)?;
    let locale = match &*resolved.read_unchecked() {
        Some(Ok(locale)) => *locale,
        _ => Locale::default(),
    };
    provide_context(locale);

    rsx! {
        document::Link { rel: "icon", href: asset!("/assets/favicon.ico") }
        document::Stylesheet { href: asset!("/assets/material.css") }
        document::Stylesheet { href: asset!("/assets/app.css") }
        // `dir` is what the stylesheet's right-to-left rules key off, and what
        // the browser uses to lay out mixed Hebrew and Latin text correctly.
        div { class: "app-root", dir: locale.dir(), lang: locale.lang(),
            Router::<Route> {}
        }
    }
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

/// Reports a fatal startup problem and exits. A replica that cannot serve must
/// not linger in a half-working state.
#[cfg(feature = "server")]
fn fatal(message: &str) -> ! {
    eprintln!("hainviter: {message}");
    std::process::exit(1);
}

#[cfg(feature = "server")]
#[tokio::main]
async fn main() {
    use axum::routing::get;
    use dioxus::server::{DioxusRouterExt, ServeConfig};

    let url = db::database_url().unwrap_or_else(|e| fatal(&e));
    // The URL carries a password, so it is never logged.
    db::init(&url).await.unwrap_or_else(|e| fatal(&e));
    println!("hainviter: connected to PostgreSQL");

    let locale = i18n::from_env();
    println!(
        "hainviter: locale {} ({})",
        locale.tag(),
        locale.dir().to_uppercase()
    );
    match audit::log_path() {
        Some(path) => println!("hainviter: audit log at {}", path.display()),
        None => println!(
            "hainviter: audit log goes to stdout only (HAINVITER_DATA_DIR is unset); \
             replies are also kept in the database"
        ),
    }

    // Settled before the listener opens, so no request can arrive while the
    // admin token is still empty.
    {
        let client = db::client().await.unwrap_or_else(|e| fatal(&e));
        auth::resolve(&**client).await.unwrap_or_else(|e| fatal(&e));
    }

    let base = std::env::var("HAINVITER_BASE_URL").unwrap_or_default();
    let origin = if base.trim().is_empty() {
        "http://<host>:8080".to_owned()
    } else {
        base.trim_end_matches('/').to_owned()
    };
    println!();
    println!("  ┌────────────────────────────────────────────────────────────");
    println!("  │ HaInviter admin panel — anyone with this link is an admin:");
    println!("  │");
    println!("  │   {origin}/admin/{}", auth::admin_token());
    println!("  │");
    println!("  └────────────────────────────────────────────────────────────");
    println!();

    audit::record(
        "server_started",
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "locale": locale.tag(),
            "base_url": base,
        }),
    );

    let addr = dioxus::cli_config::fullstack_address_or_localhost();
    let router = axum::Router::new()
        // Uploaded cover images, served out of the database.
        .route("/uploads/{name}", get(serve_upload))
        // CSV download. A plain GET (rather than a server function) so the
        // browser saves a properly named file.
        .route(
            "/admin/{token}/events/{event_id}/responses.csv",
            get(export_csv),
        )
        .serve_dioxus_application(ServeConfig::new(), App)
        .into_make_service();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| fatal(&format!("cannot bind {addr}: {e}")));
    println!("hainviter: listening on http://{addr}");
    axum::serve(listener, router).await.unwrap();
}

/// Serves one uploaded cover image out of the database.
#[cfg(feature = "server")]
async fn serve_upload(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::{
        http::{StatusCode, header},
        response::IntoResponse,
    };

    // Stored names are generated by us and are always `<32 hex>.<ext>`; anything
    // else is either a mistake or someone probing.
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
    if !valid {
        return (StatusCode::BAD_REQUEST, "bad file name").into_response();
    }

    let client = match db::client().await {
        Ok(client) => client,
        Err(e) => {
            eprintln!("uploads: {e}");
            return (StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
        }
    };
    match db::load_cover(&**client, &name).await {
        Ok(Some((content_type, bytes))) => (
            [
                (header::CONTENT_TYPE, content_type),
                // Ids are random, so a stored image never changes.
                (
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable".to_owned(),
                ),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => {
            eprintln!("uploads: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed").into_response()
        }
    }
}

/// Streams an event's responses as a CSV attachment.
#[cfg(feature = "server")]
async fn export_csv(
    axum::extract::Path((token, event_id)): axum::extract::Path<(String, i64)>,
) -> axum::response::Response {
    use axum::{
        http::{StatusCode, header},
        response::IntoResponse,
    };

    if !auth::is_admin(&token) {
        // 404 rather than 403: an invalid admin link should not confirm that the
        // route exists.
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let client = match db::client().await {
        Ok(client) => client,
        Err(e) => {
            eprintln!("export: {e}");
            return (StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
        }
    };
    let view = match db::event_admin_view(&**client, event_id).await {
        Ok(Some(view)) => view,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such event").into_response(),
        Err(e) => {
            eprintln!("export: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "export failed").into_response();
        }
    };

    let base = std::env::var("HAINVITER_BASE_URL").unwrap_or_default();
    let body = export::responses_csv(
        &view.guests,
        &base,
        i18n::from_env().strings().invite_message,
    );
    let filename = format!("{}-responses.csv", export::slug(&view.event.title));
    audit::record(
        "responses_exported",
        serde_json::json!({ "event_id": event_id, "guests": view.guests.len() }),
    );

    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Client entry point
// ---------------------------------------------------------------------------

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}
