//! The guest-facing invitation page.
//!
//! One route, one token, one decision to make. Everything is sized for a phone
//! held in one hand, because that is where invitations are opened.

use crate::{
    api,
    i18n::{active, t},
    types::{InviteView, Rsvp, RsvpSubmission},
    ui::material::{Button, ButtonKind, Chip, Icon, Loading, SelectField, TextArea, Tone},
};
use dioxus::prelude::*;

/// Resolves the token, then renders the invitation.
#[component]
pub fn Invite(token: String) -> Element {
    let s = t();
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
                main { class: "page page--centred",
                    Loading { label: s.invite_loading.to_owned() }
                }
            },
        }
    }
}

/// Shown when a token matches nothing. Deliberately says nothing about whether
/// the link ever existed.
#[component]
fn InviteUnknown() -> Element {
    let s = t();
    rsx! {
        document::Title { "{s.invite_invalid_title} — {s.app_name}" }
        main { class: "page page--centred",
            div { class: "md-card md-card--elevated landing",
                Icon { name: "error", class: "landing__mark".to_owned() }
                h1 { class: "md-headline-medium", "{s.invite_invalid_title}" }
                p { class: "md-body-large md-on-surface-variant", "{s.invite_invalid_body}" }
                p { class: "md-body-small md-on-surface-variant landing__note",
                    "{s.invite_invalid_note}"
                }
            }
        }
    }
}

/// The invitation itself, plus the RSVP form.
#[component]
fn InviteCard(token: String, initial: InviteView) -> Element {
    let s = t();
    let locale = active();

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
    let when = locale.format_datetime(&event.starts_at);
    let deadline = locale.format_date(&event.rsvp_deadline);
    let can_bring_others = event.allow_plus_ones && current.max_party_size > 1;
    let status_label = locale.status(current.status);

    // Hoisted out of `rsx!`: a conditional in attribute position is treated as a
    // string expression there, which would not type-check for typed props.
    let (status_tone, status_icon) = if current.status == Rsvp::Attending {
        (Tone::Positive, "check_circle")
    } else {
        (Tone::Negative, "cancel_circle")
    };

    let greeting = s.invite_greeting.replace("{}", &current.guest_name);
    let reply_by = s.invite_reply_by.replace("{}", &deadline);
    let closed_note = if answered {
        s.invite_closed_answered.replace("{}", status_label)
    } else {
        s.invite_closed.to_owned()
    };

    let hero_style = if event.cover_image.is_empty() {
        String::new()
    } else {
        // Quotes guard against a URL containing a closing paren.
        format!("background-image:url(\"{}\")", event.cover_image)
    };

    let submit = move |_| {
        let Some(coming) = attending() else {
            error.set(Some(s.invite_choose_first.to_owned()));
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
        document::Title { "{event.title} — {s.invite_page_title_suffix} {current.guest_name}" }
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
                    p { class: "md-title-medium invite__greeting", "{greeting}" }
                    if event.description.is_empty() {
                        p { class: "md-body-large", "{s.invite_default_body}" }
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
                                    span { class: "md-label-large", "{s.invite_when}" }
                                    span { class: "md-body-large", "{when}" }
                                }
                            }
                        }
                        if !event.location.is_empty() {
                            div { class: "invite__detail",
                                Icon { name: "place" }
                                div {
                                    span { class: "md-label-large", "{s.invite_where}" }
                                    span { class: "md-body-large", "{event.location}" }
                                    if !event.location_url.is_empty() {
                                        a {
                                            class: "invite__map-link",
                                            href: "{event.location_url}",
                                            target: "_blank",
                                            rel: "noopener noreferrer",
                                            "{s.invite_open_maps}"
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
                        h2 { class: "md-title-large", "{s.invite_question}" }
                        if answered {
                            Chip {
                                label: status_label.to_owned(),
                                tone: status_tone,
                                icon: status_icon,
                            }
                        }
                    }

                    if !deadline.is_empty() && !current.closed {
                        p { class: "md-body-small md-on-surface-variant", "{reply_by}" }
                    }

                    if current.closed {
                        div { class: "invite__closed",
                            Icon { name: "schedule" }
                            div {
                                p { class: "md-body-large", "{closed_note}" }
                                p { class: "md-body-small md-on-surface-variant",
                                    "{s.invite_closed_contact}"
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
                                span { "{s.invite_yes}" }
                            }
                            button {
                                class: if attending() == Some(false) { "invite__choice invite__choice--no is-selected" } else { "invite__choice invite__choice--no" },
                                r#type: "button",
                                role: "radio",
                                "aria-checked": if attending() == Some(false) { "true" } else { "false" },
                                onclick: move |_| attending.set(Some(false)),
                                Icon { name: "cancel_circle" }
                                span { "{s.invite_no}" }
                            }
                        }

                        if attending() == Some(true) && can_bring_others {
                            SelectField {
                                label: s.invite_party_question.to_owned(),
                                value: "{party}",
                                options: (1..=current.max_party_size)
                                    .map(|n| (n.to_string(), locale.n_of_us(n)))
                                    .collect::<Vec<_>>(),
                                onchange: move |e: FormEvent| {
                                    if let Ok(n) = e.value().parse::<i64>() {
                                        party.set(n);
                                    }
                                },
                            }
                        }

                        TextArea {
                            label: s.invite_note_label.to_owned(),
                            value: "{note}",
                            rows: 3,
                            supporting: s.invite_note_help.to_owned(),
                            oninput: move |e: FormEvent| note.set(e.value()),
                        }

                        if let Some(text) = error() {
                            p { class: "invite__error", Icon { name: "error" } "{text}" }
                        }

                        if just_saved() {
                            p { class: "invite__saved", Icon { name: "check_circle" }
                                if current.status == Rsvp::Attending {
                                    "{s.invite_thanks_yes}"
                                } else {
                                    "{s.invite_thanks_no}"
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
                                "{s.invite_sending}"
                            } else if answered {
                                "{s.invite_update}"
                            } else {
                                "{s.invite_send}"
                            }
                        }
                    }
                }

                footer { class: "invite__footer md-body-small", "{s.invite_footer}" }
            }
        }
    }
}

/// Extracts the human-readable part of a server-function error.
fn message_of(error: &ServerFnError) -> String {
    let text = error.to_string();
    text.rsplit_once(": ")
        .map(|(_, tail)| tail.to_owned())
        .unwrap_or(text)
}
