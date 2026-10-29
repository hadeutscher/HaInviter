//! Invitation image preview and sharing. Files are prepared before a share
//! gesture; downloading or copying never claims that an invitation was sent.
use crate::{
    api,
    contacts::invite_link,
    i18n::{active, t},
    types::{EventInput, GuestDto},
    ui::{
        admin::js_string,
        material::{Button, ButtonKind, Dialog, Loading, TextArea},
        message_of,
    },
};
use dioxus::prelude::*;

#[component]
pub fn InvitationCardDialog(
    token: String,
    event: EventInput,
    guest: GuestDto,
    origin: String,
    onclose: EventHandler<()>,
    on_sent: EventHandler<()>,
    on_message: EventHandler<String>,
    on_error: EventHandler<String>,
) -> Element {
    let s = t();
    let locale = active();
    let link = invite_link(&origin, &guest.token);
    let first = event.card_caption(
        if event.card_message.trim().is_empty() {
            s.card_first_default
        } else {
            &event.card_message
        },
        &guest.name,
        &link,
        locale,
    );
    let reminder = event.card_caption(
        if event.reminder_message.trim().is_empty() {
            s.card_reminder_default
        } else {
            &event.reminder_message
        },
        &guest.name,
        &link,
        locale,
    );
    let mut caption = use_signal(|| first.clone());
    let mut fallback = use_signal(|| false);
    let key = use_signal(|| guest.token.clone());
    let token = use_signal(|| token);
    let mut marking = use_signal(|| false);
    let guest_id = guest.id;
    let mark_sent = move |_| {
        marking.set(true);
        spawn(async move {
            let result = api::mark_invite_sent(token(), guest_id, true).await;
            marking.set(false);
            match result {
                Ok(_) => {
                    on_sent.call(());
                    onclose.call(());
                }
                Err(error) => on_error.call(message_of(&error)),
            }
        });
    };
    let config = format!(
        "const config = {{ key: {}, image: {}, title: {}, hosts: {}, date: {}, location: {}, rtl: {} }};\n{}",
        js_string(&key()),
        js_string(&event.invitation_image),
        js_string(&event.title),
        js_string(&event.hosts),
        js_string(&locale.format_datetime(&event.starts_at)),
        js_string(&event.location),
        locale == crate::i18n::Locale::HeIl,
        include_str!("invitation_card.js"),
    );
    let config = use_signal(|| config);
    let mut image = use_resource(move || async move {
        document::eval(&config())
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|url| !url.is_empty())
    });
    use_drop(move || {
        let _ = document::eval(&format!(
            "const key = {}; const cards = window.hainviterCards; const entry = cards?.get(key); \
             if (entry?.url) URL.revokeObjectURL(entry.url); cards?.delete(key);",
            js_string(&key()),
        ));
    });

    let share = move |_| {
        let text = caption();
        spawn(async move {
            let script = format!(
                "const key = {}; const text = {};\n{}",
                js_string(&key()),
                js_string(&text),
                include_str!("invitation_card_share.js"),
            );
            let result = document::eval(&script).await;
            match result.as_ref().ok().and_then(|v| v.as_str()) {
                Some("shared" | "cancelled") => {}
                Some("unsupported") => fallback.set(true),
                _ => on_error.call(s.err_share_failed.to_owned()),
            }
        });
    };
    let copy = move |_| {
        let text = caption();
        spawn(async move {
            let script = format!(
                "try {{ await navigator.clipboard.writeText({}); return true; }} catch (_) {{ return false; }}",
                js_string(&text),
            );
            match document::eval(&script).await {
                Ok(value) if value.as_bool() == Some(true) => {
                    on_message.call(s.share_copied.to_owned())
                }
                _ => on_error.call(s.err_share_failed.to_owned()),
            }
        });
    };
    let url = image.read().as_ref().and_then(Clone::clone);
    rsx! {
        Dialog {
            open: true,
            title: format!("{} · {}", s.card_send, guest.name),
            onclose,
            actions: rsx! {
                Button { kind: ButtonKind::Text, onclick: move |_| onclose.call(()), "{s.cancel}" }
                Button { kind: ButtonKind::Text, disabled: marking(), onclick: mark_sent, "{s.card_mark_sent}" }
                Button { kind: ButtonKind::Outlined, onclick: copy, "{s.card_copy}" }
                Button { disabled: url.is_none() || caption().trim().is_empty(), onclick: share, "{s.card_share}" }
            },
            div { class: "stack",
                div { class: "inline-actions",
                    Button { kind: ButtonKind::Outlined, onclick: move |_| caption.set(first.clone()), "{s.card_first}" }
                    Button { kind: ButtonKind::Outlined, onclick: move |_| caption.set(reminder.clone()), "{s.card_reminder}" }
                }
                if let Some(url) = url {
                    img { class: "invitation-card-preview", src: "{url}", alt: "{s.card_preview}" }
                    a { class: "md-button md-button--outlined", href: "{url}", download: "invitation.png", "{s.card_download}" }
                } else if image.read().is_none() {
                    Loading { label: s.card_loading.to_owned() }
                } else {
                    p { role: "alert", "{s.card_failed}" }
                    Button { kind: ButtonKind::Text, onclick: move |_| image.restart(), "{s.card_retry}" }
                }
                TextArea {
                    label: s.card_caption.to_owned(),
                    value: caption(),
                    rows: 4,
                    oninput: move |e: FormEvent| caption.set(e.value()),
                }
                p { class: "md-field__supporting", "{s.card_hint}" }
                if fallback() {
                    p { role: "status", "{s.card_fallback}" }
                }
            }
        }
    }
}
