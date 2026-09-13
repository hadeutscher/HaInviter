//! Page components.

pub mod admin;
pub mod invite;
pub mod material;

pub use admin::{AdminEvent, AdminEvents};
pub use invite::Invite;

use dioxus::prelude::*;
use material::Icon;

/// The public front door. There is deliberately nothing to do here: guests
/// arrive through their personal link and admins through theirs.
#[component]
pub fn Home() -> Element {
    rsx! {
        document::Title { "HaInviter" }
        main { class: "page page--centred",
            div { class: "md-card md-card--elevated landing",
                Icon { name: "mail", class: "landing__mark".to_owned() }
                h1 { class: "md-headline-medium", "HaInviter" }
                p { class: "md-body-large md-on-surface-variant",
                    "Invitations here are personal. Open the link the hosts sent you to see
                     your invitation and let them know whether you can come."
                }
                p { class: "md-body-small md-on-surface-variant landing__note",
                    "Lost your link? Ask whoever invited you to send it again."
                }
            }
        }
    }
}

/// Anything that is not a known route.
#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let path = segments.join("/");
    rsx! {
        document::Title { "Not found — HaInviter" }
        main { class: "page page--centred",
            div { class: "md-card md-card--elevated landing",
                Icon { name: "search", class: "landing__mark".to_owned() }
                h1 { class: "md-headline-medium", "Nothing here" }
                p { class: "md-body-large md-on-surface-variant",
                    "We could not find " code { "/{path}" } "."
                }
                p { class: "md-body-small md-on-surface-variant landing__note",
                    "Invitation links look like /i/… — check the link you were sent, or ask the
                     hosts to resend it."
                }
            }
        }
    }
}
