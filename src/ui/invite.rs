//! The guest-facing invitation page.
//!
//! One route, one token, one decision to make. Everything is sized for a phone
//! held in one hand, because that is where invitations are opened.

use crate::{
    api,
    types::{InviteView, Rsvp, RsvpSubmission},
    ui::material::{Button, ButtonKind, Chip, Icon, Loading, SelectField, TextArea, Tone},
};
use dioxus::prelude::*;

/// Resolves the token, then renders the invitation.
#[component]
pub fn Invite(token: String) -> Element {
    let lookup = token.clone();
    let invite = use_server_future(move || {
        let token = lookup.clone();
        async move { api::get_invite(token).await }
    })?;

    rsx! {
        match &*invite.read_unchecked() {
            Some(Ok(view)) => rsx! {
                InviteCard { token: token.clone(), initial: view.clone() }
            },
            Some(Err(_)) => rsx! { InviteUnknown {} },
            None => rsx! {
                main { class: "page page--centred", Loading { label: "Opening your invitation…".to_owned() } }
            },
        }
    }
}

/// Shown when a token matches nothing. Deliberately says nothing about whether
/// the link ever existed.
#[component]
fn InviteUnknown() -> Element {
    rsx! {
        document::Title { "Invitation not found — HaInviter" }
        main { class: "page page--centred",
            div { class: "md-card md-card--elevated landing",
                Icon { name: "error", class: "landing__mark".to_owned() }
                h1 { class: "md-headline-medium", "This invitation link is not valid" }
                p { class: "md-body-large md-on-surface-variant",
                    "The link may have been copied incompletely, or replaced with a new one."
                }
                p { class: "md-body-small md-on-surface-variant landing__note",
                    "Please ask the hosts to send you a fresh link."
                }
            }
        }
    }
}

/// The invitation itself, plus the RSVP form.
#[component]
fn InviteCard(token: String, initial: InviteView) -> Element {
    // The server's answer is the source of truth: after a successful reply we
    // replace this wholesale with what was actually stored.
    let mut view = use_signal(|| initial.clone());

    let mut attending: Signal<Option<bool>> = use_signal(|| match initial.status {
        Rsvp::Pending => None,
        Rsvp::Attending => Some(true),
        Rsvp::Declined => Some(false),
    });
    let mut party = use_signal(|| initial.party_size.max(1));
    let mut note = use_signal(|| initial.note.clone());
    let mut saving = use_signal(|| false);
    let mut just_saved = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let current = view();
    let event = current.event.clone();
    let answered = current.status != Rsvp::Pending;
    let when = crate::types::format_event_datetime(&event.starts_at);
    let deadline = crate::types::format_date(&event.rsvp_deadline);
    let can_bring_others = event.allow_plus_ones && current.max_party_size > 1;
    // Hoisted out of `rsx!`: a conditional in attribute position is treated as a
    // string expression there, which would not type-check for typed props.
    let (status_tone, status_icon) = if current.status == Rsvp::Attending {
        (Tone::Positive, "check_circle")
    } else {
        (Tone::Negative, "cancel_circle")
    };

    let hero_style = if event.cover_image.is_empty() {
        String::new()
    } else {
        // Quotes guard against a URL containing a closing paren.
        format!("background-image:url(\"{}\")", event.cover_image)
    };

    let submit = move |_| {
        let Some(coming) = attending() else {
            error.set(Some("Please choose whether you can come.".to_owned()));
            return;
        };
        let submission = RsvpSubmission {
            attending: coming,
            party_size: party(),
            note: note(),
        };
        let token = token.clone();
        saving.set(true);
        just_saved.set(false);
        error.set(None);
        spawn(async move {
            match api::submit_rsvp(token, submission).await {
                Ok(updated) => {
                    party.set(updated.party_size.max(1));
                    note.set(updated.note.clone());
                    view.set(updated);
                    just_saved.set(true);
                    saving.set(false);
                }
                Err(e) => {
                    error.set(Some(message_of(&e)));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        document::Title { "{event.title} — invitation for {current.guest_name}" }
        document::Meta { name: "description", content: "{event.title}. {when}" }

        main { class: "page invite",
            // ── Hero ──────────────────────────────────────────────────────
            header {
                class: if event.cover_image.is_empty() { "invite__hero invite__hero--plain" } else { "invite__hero" },
                style: "{hero_style}",
                div { class: "invite__hero-veil" }
                div { class: "invite__hero-text",
                    if !event.hosts.is_empty() {
                        p { class: "invite__hosts", "{event.hosts}" }
                    }
                    h1 { class: "invite__title", "{event.title}" }
                    if !when.is_empty() {
                        p { class: "invite__when", "{when}" }
                    }
                }
            }

            div { class: "invite__body",
                // ── Greeting & description ────────────────────────────────
                section { class: "md-card md-card--elevated invite__card",
                    p { class: "md-title-medium invite__greeting", "Dear {current.guest_name}," }
                    if event.description.is_empty() {
                        p { class: "md-body-large", "You are invited — we would love to see you there." }
                    } else {
                        // Preserve the paragraph breaks the host typed.
                        for (i , para) in event.description.split("\n\n").enumerate() {
                            p { key: "{i}", class: "md-body-large invite__paragraph", "{para}" }
                        }
                    }
                }

                // ── Where & when ─────────────────────────────────────────
                if !when.is_empty() || !event.location.is_empty() {
                    section { class: "md-card md-card--filled invite__card invite__details",
                        if !when.is_empty() {
                            div { class: "invite__detail",
                                Icon { name: "event" }
                                div {
                                    span { class: "md-label-large", "When" }
                                    span { class: "md-body-large", "{when}" }
                                }
                            }
                        }
                        if !event.location.is_empty() {
                            div { class: "invite__detail",
                                Icon { name: "place" }
                                div {
                                    span { class: "md-label-large", "Where" }
                                    span { class: "md-body-large", "{event.location}" }
                                    if !event.location_url.is_empty() {
                                        a {
                                            class: "invite__map-link",
                                            href: "{event.location_url}",
                                            target: "_blank",
                                            rel: "noopener noreferrer",
                                            "Open in maps"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── RSVP ─────────────────────────────────────────────────
                section { class: "md-card md-card--elevated invite__card invite__rsvp",
                    div { class: "invite__rsvp-head",
                        h2 { class: "md-title-large", "Will you join us?" }
                        if answered {
                            Chip {
                                label: current.status.label().to_owned(),
                                tone: status_tone,
                                icon: status_icon,
                            }
                        }
                    }

                    if !deadline.is_empty() && !current.closed {
                        p { class: "md-body-small md-on-surface-variant",
                            "Please reply by {deadline}."
                        }
                    }

                    if current.closed {
                        div { class: "invite__closed",
                            Icon { name: "schedule" }
                            div {
                                p { class: "md-body-large",
                                    if answered {
                                        "Replies have closed. Your answer is recorded as \"{current.status.label()}\"."
                                    } else {
                                        "Replies have closed."
                                    }
                                }
                                p { class: "md-body-small md-on-surface-variant",
                                    "If something has changed, please contact the hosts directly."
                                }
                            }
                        }
                    } else {
                        // Two mutually exclusive choices, Material's "segmented
                        // button" pattern scaled up for thumbs.
                        div { class: "invite__choices", role: "radiogroup",
                            button {
                                class: if attending() == Some(true) { "invite__choice invite__choice--yes is-selected" } else { "invite__choice invite__choice--yes" },
                                r#type: "button",
                                role: "radio",
                                "aria-checked": if attending() == Some(true) { "true" } else { "false" },
                                onclick: move |_| attending.set(Some(true)),
                                Icon { name: "check_circle" }
                                span { "Yes, I'll be there" }
                            }
                            button {
                                class: if attending() == Some(false) { "invite__choice invite__choice--no is-selected" } else { "invite__choice invite__choice--no" },
                                r#type: "button",
                                role: "radio",
                                "aria-checked": if attending() == Some(false) { "true" } else { "false" },
                                onclick: move |_| attending.set(Some(false)),
                                Icon { name: "cancel_circle" }
                                span { "Sorry, I can't" }
                            }
                        }

                        if attending() == Some(true) && can_bring_others {
                            SelectField {
                                label: "How many of you are coming?".to_owned(),
                                value: "{party}",
                                options: (1..=current.max_party_size).map(party_option).collect::<Vec<_>>(),
                                onchange: move |e: FormEvent| {
                                    if let Ok(n) = e.value().parse::<i64>() {
                                        party.set(n);
                                    }
                                },
                            }
                        }

                        TextArea {
                            label: "A note for the hosts (optional)".to_owned(),
                            value: "{note}",
                            rows: 3,
                            supporting: "Dietary needs, a song request, or just hello.".to_owned(),
                            oninput: move |e: FormEvent| note.set(e.value()),
                        }

                        if let Some(text) = error() {
                            p { class: "invite__error", Icon { name: "error" } "{text}" }
                        }

                        if just_saved() {
                            p { class: "invite__saved", Icon { name: "check_circle" }
                                if current.status == Rsvp::Attending {
                                    "Thank you — we have you down. See you there!"
                                } else {
                                    "Thank you for letting us know — you will be missed."
                                }
                            }
                        }

                        Button {
                            kind: ButtonKind::Filled,
                            full_width: true,
                            disabled: saving(),
                            icon: "mail",
                            onclick: submit,
                            if saving() {
                                "Sending…"
                            } else if answered {
                                "Update my reply"
                            } else {
                                "Send my reply"
                            }
                        }
                    }
                }

                footer { class: "invite__footer md-body-small",
                    "This invitation is personal to you — please do not forward the link."
                }
            }
        }
    }
}

/// Label for one party-size option.
fn party_option(n: i64) -> (String, String) {
    let label = match n {
        1 => "Just me".to_owned(),
        n => format!("{n} of us"),
    };
    (n.to_string(), label)
}

/// Extracts the human-readable part of a server-function error.
fn message_of(error: &ServerFnError) -> String {
    let text = error.to_string();
    text.rsplit_once(": ")
        .map(|(_, tail)| tail.to_owned())
        .unwrap_or(text)
}
