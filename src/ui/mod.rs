//! Page components.

pub mod admin;
pub mod invite;
pub mod material;

pub use admin::{AdminEvent, AdminEvents};
pub use invite::Invite;

use crate::i18n::t;
use dioxus::prelude::*;
use material::Icon;

/// The public front door. There is deliberately nothing to do here: guests
/// arrive through their personal link and admins through theirs.
#[component]
pub fn Home() -> Element {
    let s = t();
    rsx! {
        document::Title { "{s.app_name}" }
        main { class: "page page--centred",
            div { class: "md-card md-card--elevated landing",
                Icon { name: "mail", class: "landing__mark".to_owned() }
                h1 { class: "md-headline-medium", "{s.app_name}" }
                p { class: "md-body-large md-on-surface-variant", "{s.home_body}" }
                p { class: "md-body-small md-on-surface-variant landing__note", "{s.home_note}" }
            }
        }
    }
}

/// Anything that is not a known route.
#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let s = t();
    let path = format!("/{}", segments.join("/"));
    let body = s.not_found_body.replace("{}", &path);
    rsx! {
        document::Title { "{s.not_found_title} — {s.app_name}" }
        main { class: "page page--centred",
            div { class: "md-card md-card--elevated landing",
                Icon { name: "search", class: "landing__mark".to_owned() }
                h1 { class: "md-headline-medium", "{s.not_found_title}" }
                p { class: "md-body-large md-on-surface-variant", "{body}" }
                p { class: "md-body-small md-on-surface-variant landing__note", "{s.not_found_note}" }
            }
        }
    }
}
