//! The admin panel.
//!
//! Authorisation is the URL: every screen carries the admin token in its path
//! and passes it to each server call. Nothing is cached in the browser, so
//! closing the tab ends the session.

use crate::{
    Route, api,
    i18n::{active, t},
    types::{EventAdminView, EventInput, EventSummary, GuestDto, Rsvp},
    ui::material::{
        Button, ButtonKind, Chip, Dialog, EmptyState, Fab, Icon, IconButton, Loading, SelectField,
        Snackbar, Stat, Switch, TextArea, TextField, Tone,
    },
};
use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

/// A transient message with a colour.
type Toast = Option<(String, bool)>;

/// Extracts the human-readable part of a server-function error.
fn message_of(error: &ServerFnError) -> String {
    let text = error.to_string();
    text.rsplit_once(": ")
        .map(|(_, tail)| tail.to_owned())
        .unwrap_or(text)
}

/// Escapes a string for interpolation into a JavaScript source snippet.
fn js_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Copies a guest's invitation link to the clipboard.
///
/// The origin is resolved in the browser unless `HAINVITER_BASE_URL` provides
/// one, so links copied from a LAN address and from the public hostname are both
/// correct without configuration.
fn copy_invite_link(base: String, guest_token: &str) {
    let script = format!(
        "const base = {}; const path = {}; \
         await navigator.clipboard.writeText((base || location.origin) + path);",
        js_string(&base),
        js_string(&format!("/i/{guest_token}")),
    );
    spawn(async move {
        let _ = document::eval(&script).await;
    });
}

/// The Material top app bar used by both admin screens.
#[component]
fn AdminBar(title: String, subtitle: String, back: Option<Route>, actions: Element) -> Element {
    let s = t();
    rsx! {
        header { class: "admin__bar",
            if let Some(route) = back {
                Link { to: route, class: "md-icon-button", "aria-label": "{s.admin_back}",
                    Icon { name: "arrow_back" }
                }
            }
            div { class: "admin__bar-titles",
                span { class: "admin__bar-title", "{title}" }
                if !subtitle.is_empty() {
                    span { class: "admin__bar-subtitle", "{subtitle}" }
                }
            }
            div { class: "admin__bar-actions", {actions} }
        }
    }
}

/// Shown whenever the server rejects the token in the URL.
#[component]
fn NotAuthorised() -> Element {
    let s = t();
    rsx! {
        main { class: "page page--centred",
            div { class: "md-card md-card--elevated landing",
                Icon { name: "error", class: "landing__mark".to_owned() }
                h1 { class: "md-headline-medium", "{s.admin_invalid_title}" }
                p { class: "md-body-large md-on-surface-variant", "{s.admin_invalid_body}" }
                p { class: "md-body-small md-on-surface-variant landing__note",
                    "{s.admin_invalid_note_before}"
                    code { "kubectl logs deploy/hainviter" }
                    "{s.admin_invalid_note_after}"
                }
            }
        }
    }
}

/// "12 invited · 30 expected", in the active language.
fn invited_expected(s: &'static crate::i18n::Strings, invited: i64, expected: i64) -> String {
    s.admin_invited_expected
        .replace("{invited}", &invited.to_string())
        .replace("{expected}", &expected.to_string())
}

/// The chips summarising one event's replies.
#[component]
fn ResponseChips(summary: EventSummary) -> Element {
    let s = t();
    let count = |template: &str, n: i64| template.replace("{n}", &n.to_string());
    rsx! {
        div { class: "event-card__chips",
            Chip {
                label: count(s.admin_n_coming, summary.attending),
                tone: Tone::Positive,
                icon: "check_circle",
            }
            Chip {
                label: count(s.admin_n_declined, summary.declined),
                tone: Tone::Negative,
                icon: "cancel_circle",
            }
            Chip {
                label: count(s.admin_n_waiting, summary.pending),
                tone: Tone::Waiting,
                icon: "schedule",
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event list
// ---------------------------------------------------------------------------

/// The admin landing screen: every event, and a button to create another.
#[component]
pub fn AdminEvents(token: String) -> Element {
    let s = t();
    let locale = active();
    let tok = use_signal(|| token);
    let mut events = use_resource(move || async move { api::list_events(tok()).await });

    let mut creating = use_signal(|| false);
    let mut form = use_signal(EventInput::default);
    let mut saving = use_signal(|| false);
    let mut toast: Signal<Toast> = use_signal(|| None);

    let navigator = navigator();

    let create = move |_| {
        let input = form();
        if input.title.trim().is_empty() {
            toast.set(Some((s.admin_needs_title.to_owned(), true)));
            return;
        }
        saving.set(true);
        spawn(async move {
            match api::create_event(tok(), input).await {
                Ok(id) => {
                    saving.set(false);
                    creating.set(false);
                    form.set(EventInput::default());
                    // Straight into the new event: the next thing anyone wants is
                    // to add the guest list.
                    navigator.push(Route::AdminEvent {
                        token: tok(),
                        event_id: id,
                    });
                }
                Err(e) => {
                    toast.set(Some((message_of(&e), true)));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        document::Title { "{s.admin_events} — {s.admin_subtitle}" }
        main { class: "page admin",
            AdminBar {
                title: s.admin_events.to_owned(),
                subtitle: s.admin_subtitle.to_owned(),
                actions: rsx! {
                    IconButton {
                        icon: "refresh",
                        label: s.reload.to_owned(),
                        onclick: move |_| events.restart(),
                    }
                },
            }

            div { class: "admin__content",
                div { class: "token-callout",
                    Icon { name: "link" }
                    p { "{s.admin_link_warning}" }
                }

                match &*events.read_unchecked() {
                    Some(Ok(list)) if list.is_empty() => rsx! {
                        div { class: "md-card md-card--outlined",
                            EmptyState {
                                icon: "event",
                                title: s.admin_no_events_title.to_owned(),
                                body: s.admin_no_events_body.to_owned(),
                            }
                        }
                    },
                    Some(Ok(list)) => rsx! {
                        div { class: "event-list",
                            for summary in list.clone() {
                                Link {
                                    key: "{summary.id}",
                                    to: Route::AdminEvent { token: tok(), event_id: summary.id },
                                    class: "md-card md-card--elevated event-card",
                                    h2 { class: "event-card__title", "{summary.title}" }
                                    div { class: "event-card__meta",
                                        if !summary.starts_at.is_empty() {
                                            span {
                                                Icon { name: "event" }
                                                "{locale.format_datetime(&summary.starts_at)}"
                                            }
                                        }
                                        if !summary.location.is_empty() {
                                            span {
                                                Icon { name: "place" }
                                                "{summary.location}"
                                            }
                                        }
                                        span {
                                            Icon { name: "group" }
                                            {invited_expected(s, summary.guest_count, summary.head_count)}
                                        }
                                    }
                                    ResponseChips { summary: summary.clone() }
                                }
                            }
                        }
                    },
                    Some(Err(_)) => rsx! { NotAuthorised {} },
                    None => rsx! { Loading { label: s.admin_loading_events.to_owned() } },
                }
            }

            Fab {
                icon: "add",
                label: s.admin_new_event.to_owned(),
                onclick: move |_| {
                    form.set(EventInput { allow_plus_ones: true, ..EventInput::default() });
                    creating.set(true);
                },
            }

            Dialog {
                open: creating(),
                wide: true,
                title: s.admin_new_event.to_owned(),
                onclose: move |_| creating.set(false),
                actions: rsx! {
                    Button {
                        kind: ButtonKind::Text,
                        onclick: move |_| creating.set(false),
                        "{s.cancel}"
                    }
                    Button { disabled: saving(), onclick: create,
                        if saving() { "{s.admin_creating}" } else { "{s.admin_create_event}" }
                    }
                },
                EventFields { token: tok(), form }
            }

            if let Some((message, is_error)) = toast() {
                Snackbar {
                    message,
                    error: is_error,
                    onclick: move |_| toast.set(None),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event editor fields
// ---------------------------------------------------------------------------

/// The editable fields of an event, shared by the create dialog and the event
/// screen's "Details" tab.
#[component]
fn EventFields(token: String, form: Signal<EventInput>) -> Element {
    let s = t();
    let tok = use_signal(|| token);
    let mut form = form;
    let mut uploading = use_signal(|| false);
    let mut upload_error: Signal<Option<String>> = use_signal(|| None);
    let current = form();

    let pick_cover = move |event: FormEvent| {
        let Some(file) = event.files().first().cloned() else {
            return;
        };
        // Checked here as well as on the server so an oversized photo fails
        // before it is read into memory and shipped over the wire.
        if file.size() > 8 * 1024 * 1024 {
            upload_error.set(Some(
                s.field_cover_too_large
                    .replace("{}", &(api::MAX_COVER_BYTES / (1024 * 1024)).to_string()),
            ));
            return;
        }
        let name = file.name();
        uploading.set(true);
        upload_error.set(None);
        spawn(async move {
            match file.read_bytes().await {
                Ok(bytes) => match api::upload_cover(tok(), name, bytes.to_vec()).await {
                    Ok(url) => form.with_mut(|f| f.cover_image = url),
                    Err(e) => upload_error.set(Some(message_of(&e))),
                },
                Err(e) => upload_error.set(Some(format!("{} ({e})", s.field_cover_unreadable))),
            }
            uploading.set(false);
        });
    };

    rsx! {
        div { class: "form-grid",
            div { class: "form-span-2",
                TextField {
                    label: s.field_title.to_owned(),
                    value: current.title.clone(),
                    supporting: s.field_title_help.to_owned(),
                    oninput: move |e: FormEvent| form.with_mut(|f| f.title = e.value()),
                }
            }
            TextField {
                label: s.field_hosts.to_owned(),
                value: current.hosts.clone(),
                supporting: s.field_hosts_help.to_owned(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.hosts = e.value()),
            }
            TextField {
                label: s.field_starts.to_owned(),
                value: current.starts_at.clone(),
                input_type: "datetime-local".to_owned(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.starts_at = e.value()),
            }
            TextField {
                label: s.field_location.to_owned(),
                value: current.location.clone(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.location = e.value()),
            }
            TextField {
                label: s.field_map.to_owned(),
                value: current.location_url.clone(),
                supporting: s.field_map_help.to_owned(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.location_url = e.value()),
            }
            TextField {
                label: s.field_deadline.to_owned(),
                value: current.rsvp_deadline.clone(),
                input_type: "date".to_owned(),
                supporting: s.field_deadline_help.to_owned(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.rsvp_deadline = e.value()),
            }
            div { class: "form-span-2",
                TextArea {
                    label: s.field_body.to_owned(),
                    value: current.description.clone(),
                    rows: 5,
                    supporting: s.field_body_help.to_owned(),
                    oninput: move |e: FormEvent| form.with_mut(|f| f.description = e.value()),
                }
            }
            div { class: "form-span-2 stack",
                span { class: "md-label-large md-on-surface-variant", "{s.field_cover}" }
                div { class: "cover-preview",
                    if current.cover_image.is_empty() {
                        img { src: "", alt: "" }
                    } else {
                        img { src: "{current.cover_image}", alt: "{s.field_cover_preview_alt}" }
                    }
                    div { class: "inline-actions",
                        // A styled <label> opens the file picker without JavaScript.
                        label { r#for: "cover-file", class: "md-button md-button--outlined",
                            Icon { name: "upload" }
                            span { class: "md-button__label",
                                if uploading() { "{s.field_cover_uploading}" } else { "{s.field_cover_upload}" }
                            }
                        }
                        input {
                            id: "cover-file",
                            class: "file-input",
                            r#type: "file",
                            accept: "image/png,image/jpeg,image/gif,image/webp",
                            onchange: pick_cover,
                        }
                        if !current.cover_image.is_empty() {
                            Button {
                                kind: ButtonKind::Text,
                                onclick: move |_| form.with_mut(|f| f.cover_image = String::new()),
                                "{s.field_cover_remove}"
                            }
                        }
                    }
                }
                TextField {
                    label: s.field_cover_url.to_owned(),
                    value: current.cover_image.clone(),
                    oninput: move |e: FormEvent| form.with_mut(|f| f.cover_image = e.value()),
                }
                if let Some(text) = upload_error() {
                    span { class: "invite__error", Icon { name: "error" } "{text}" }
                }
            }
            div { class: "form-span-2",
                Switch {
                    label: s.field_plus_ones.to_owned(),
                    checked: current.allow_plus_ones,
                    onchange: move |on| form.with_mut(|f| f.allow_plus_ones = on),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Single event
// ---------------------------------------------------------------------------

/// Which pane of the event screen is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Details,
    Guests,
    Responses,
}

/// One event: its details, its guest list and its replies.
#[component]
pub fn AdminEvent(token: String, event_id: i64) -> Element {
    let s = t();
    let tok = use_signal(|| token);
    let mut detail = use_resource(move || async move { api::get_event(tok(), event_id).await });
    let base = use_resource(move || async move { api::base_url().await });

    let mut tab = use_signal(|| Tab::Details);
    let mut toast: Signal<Toast> = use_signal(|| None);
    let mut form = use_signal(EventInput::default);
    let mut form_loaded: Signal<Option<i64>> = use_signal(|| None);

    // Seed the editor once per event, so a reload after saving does not discard
    // whatever is being typed.
    use_effect(move || {
        if let Some(Ok(view)) = &*detail.read()
            && form_loaded() != Some(view.id)
        {
            form.set(view.event.clone());
            form_loaded.set(Some(view.id));
        }
    });

    let base_url = match &*base.read_unchecked() {
        Some(Ok(url)) => url.clone(),
        _ => String::new(),
    };

    let view = match &*detail.read_unchecked() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => return rsx! { NotAuthorised {} },
        None => {
            return rsx! {
                main { class: "page admin",
                    AdminBar {
                        title: s.loading.to_owned(),
                        subtitle: String::new(),
                        back: Route::AdminEvents { token: tok() },
                        actions: rsx! {},
                    }
                    div { class: "admin__content", Loading {} }
                }
            };
        }
    };

    rsx! {
        document::Title { "{view.event.title} — HaInviter admin" }
        main { class: "page admin",
            AdminBar {
                title: view.event.title.clone(),
                subtitle: invited_expected(
                    s,
                    view.guests.len() as i64,
                    expected_head_count(&view.guests),
                ),
                back: Route::AdminEvents { token: tok() },
                actions: rsx! {
                    IconButton {
                        icon: "refresh",
                        label: s.reload.to_owned(),
                        onclick: move |_| detail.restart(),
                    }
                },
            }

            div { class: "admin__content",
                nav { class: "admin-tabs",
                    button {
                        class: if tab() == Tab::Details { "admin-tab is-active" } else { "admin-tab" },
                        r#type: "button",
                        onclick: move |_| tab.set(Tab::Details),
                        "{s.tab_details}"
                    }
                    button {
                        class: if tab() == Tab::Guests { "admin-tab is-active" } else { "admin-tab" },
                        r#type: "button",
                        onclick: move |_| tab.set(Tab::Guests),
                        {s.tab_guests.replace("{n}", &view.guests.len().to_string())}
                    }
                    button {
                        class: if tab() == Tab::Responses { "admin-tab is-active" } else { "admin-tab" },
                        r#type: "button",
                        onclick: move |_| tab.set(Tab::Responses),
                        "{s.tab_responses}"
                    }
                }

                match tab() {
                    Tab::Details => rsx! {
                        DetailsTab { token: tok(), event_id, form, toast, reload: detail }
                    },
                    Tab::Guests => rsx! {
                        GuestsTab {
                            token: tok(),
                            event_id,
                            base_url: base_url.clone(),
                            guests: view.guests.clone(),
                            toast,
                            reload: detail,
                        }
                    },
                    Tab::Responses => rsx! {
                        ResponsesTab {
                            token: tok(),
                            event_id,
                            guests: view.guests.clone(),
                        }
                    },
                }
            }

            if let Some((message, is_error)) = toast() {
                Snackbar {
                    message,
                    error: is_error,
                    onclick: move |_| toast.set(None),
                }
            }
        }
    }
}

/// Total people expected: the confirmed party size of every attending guest.
fn expected_head_count(guests: &[GuestDto]) -> i64 {
    guests
        .iter()
        .filter(|g| g.status == Rsvp::Attending)
        .map(|g| g.party_size.max(1))
        .sum()
}

// ---------------------------------------------------------------------------
// Details tab
// ---------------------------------------------------------------------------

#[component]
fn DetailsTab(
    token: String,
    event_id: i64,
    form: Signal<EventInput>,
    toast: Signal<Toast>,
    reload: Resource<Result<EventAdminView, ServerFnError>>,
) -> Element {
    let s = t();
    let tok = use_signal(|| token);
    let mut toast = toast;
    let mut reload = reload;
    let mut saving = use_signal(|| false);
    let mut confirming_delete = use_signal(|| false);
    let navigator = navigator();

    let save = move |_| {
        let input = form();
        saving.set(true);
        spawn(async move {
            match api::update_event(tok(), event_id, input).await {
                Ok(()) => {
                    toast.set(Some((s.admin_saved.to_owned(), false)));
                    reload.restart();
                }
                Err(e) => toast.set(Some((message_of(&e), true))),
            }
            saving.set(false);
        });
    };

    let delete = move |_| {
        spawn(async move {
            match api::delete_event(tok(), event_id).await {
                Ok(()) => {
                    navigator.push(Route::AdminEvents { token: tok() });
                }
                Err(e) => {
                    confirming_delete.set(false);
                    toast.set(Some((message_of(&e), true)));
                }
            }
        });
    };

    rsx! {
        section { class: "md-card md-card--elevated stack",
            EventFields { token: tok(), form }
            div { class: "form-actions",
                Button {
                    kind: ButtonKind::Danger,
                    icon: "delete",
                    onclick: move |_| confirming_delete.set(true),
                    "{s.admin_delete_event}"
                }
                Button { icon: "check", disabled: saving(), onclick: save,
                    if saving() { "{s.saving}" } else { "{s.admin_save_changes}" }
                }
            }
        }

        Dialog {
            open: confirming_delete(),
            title: s.admin_delete_event_title.to_owned(),
            onclose: move |_| confirming_delete.set(false),
            actions: rsx! {
                Button {
                    kind: ButtonKind::Text,
                    onclick: move |_| confirming_delete.set(false),
                    "{s.admin_keep_it}"
                }
                Button {
                    kind: ButtonKind::Danger,
                    onclick: delete,
                    "{s.admin_delete_event}"
                }
            },
            p { class: "md-body-medium", "{s.admin_delete_event_body}" }
        }
    }
}

// ---------------------------------------------------------------------------
// Guests tab
// ---------------------------------------------------------------------------

#[component]
fn GuestsTab(
    token: String,
    event_id: i64,
    base_url: String,
    guests: Vec<GuestDto>,
    toast: Signal<Toast>,
    reload: Resource<Result<EventAdminView, ServerFnError>>,
) -> Element {
    let s = t();
    let locale = active();
    let tok = use_signal(|| token);
    let mut origin = use_signal(|| base_url);
    let mut toast = toast;
    let mut reload = reload;

    // `HAINVITER_BASE_URL` when it is set, and the browser's own origin
    // otherwise. Resolved once, here, because the WhatsApp control needs a
    // complete URL in its `href` at render time rather than in a click handler.
    let _origin_resolved = use_future(move || async move {
        if origin.peek().trim().is_empty()
            && let Ok(value) = document::eval("return location.origin;").await
            && let Some(text) = value.as_str()
        {
            origin.set(text.to_owned());
        }
    });

    let mut bulk = use_signal(String::new);
    let mut default_party = use_signal(|| "1".to_owned());
    let mut adding = use_signal(|| false);
    let mut importing = use_signal(|| false);
    let mut editing: Signal<Option<GuestDto>> = use_signal(|| None);
    let mut edit_name = use_signal(String::new);
    let mut edit_phone = use_signal(String::new);
    let mut edit_party = use_signal(|| "1".to_owned());
    let mut removing: Signal<Option<GuestDto>> = use_signal(|| None);

    let add = move |_| {
        let raw = bulk();
        if raw.trim().is_empty() {
            toast.set(Some((s.guests_type_a_name.to_owned(), true)));
            return;
        }
        let size = default_party().parse::<i64>().unwrap_or(1);
        adding.set(true);
        spawn(async move {
            match api::add_guests(tok(), event_id, raw, size).await {
                Ok(0) => toast.set(Some((s.guests_all_present.to_owned(), false))),
                Ok(n) => {
                    bulk.set(String::new());
                    toast.set(Some((
                        s.guests_added_n.replace("{n}", &n.to_string()),
                        false,
                    )));
                    reload.restart();
                }
                Err(e) => toast.set(Some((message_of(&e), true))),
            }
            adding.set(false);
        });
    };

    let import = move |event: FormEvent| {
        let Some(file) = event.files().first().cloned() else {
            return;
        };
        // Checked here as well as on the server so an accidental photo fails
        // before it is read into memory and shipped over the wire.
        if file.size() > api::MAX_VCF_BYTES as u64 {
            toast.set(Some((
                s.field_cover_too_large
                    .replace("{}", &(api::MAX_VCF_BYTES / (1024 * 1024)).to_string()),
                true,
            )));
            return;
        }
        let size = default_party().parse::<i64>().unwrap_or(1);
        importing.set(true);
        spawn(async move {
            match file.read_bytes().await {
                Ok(bytes) => {
                    match api::import_contacts(tok(), event_id, bytes.to_vec(), size).await {
                        Ok(summary) => {
                            let mut text = s
                                .guests_imported
                                .replace("{found}", &summary.found.to_string())
                                .replace("{added}", &summary.added.to_string());
                            // Worth saying out loud: those guests exist but
                            // cannot be reached from their row.
                            if summary.without_phone > 0 {
                                text.push(' ');
                                text.push_str(
                                    &s.guests_import_no_phone
                                        .replace("{n}", &summary.without_phone.to_string()),
                                );
                            }
                            toast.set(Some((text, false)));
                            reload.restart();
                        }
                        Err(e) => toast.set(Some((message_of(&e), true))),
                    }
                }
                Err(e) => {
                    toast.set(Some((format!("{} ({e})", s.field_cover_unreadable), true)));
                }
            }
            importing.set(false);
        });
    };

    let save_edit = move |_| {
        let Some(guest) = editing() else { return };
        let name = edit_name();
        let phone = edit_phone();
        let size = edit_party().parse::<i64>().unwrap_or(1);
        spawn(async move {
            match api::update_guest(tok(), guest.id, name, size, phone).await {
                Ok(_) => {
                    editing.set(None);
                    reload.restart();
                }
                Err(e) => toast.set(Some((message_of(&e), true))),
            }
        });
    };

    let confirm_remove = move |_| {
        let Some(guest) = removing() else { return };
        spawn(async move {
            match api::delete_guest(tok(), guest.id).await {
                Ok(()) => {
                    removing.set(None);
                    toast.set(Some((s.guests_removed.replace("{}", &guest.name), false)));
                    reload.restart();
                }
                Err(e) => {
                    removing.set(None);
                    toast.set(Some((message_of(&e), true)));
                }
            }
        });
    };

    rsx! {
        section { class: "md-card md-card--elevated stack",
            h2 { class: "md-title-large", "{s.guests_add_title}" }
            TextArea {
                label: s.guests_one_per_line.to_owned(),
                value: bulk(),
                rows: 6,
                supporting: s.guests_paste_help.to_owned(),
                oninput: move |e: FormEvent| bulk.set(e.value()),
            }
            div { class: "inline-actions",
                SelectField {
                    label: s.guests_default_party.to_owned(),
                    value: default_party(),
                    options: (1..=10).map(|n| (n.to_string(), n.to_string())).collect::<Vec<_>>(),
                    onchange: move |e: FormEvent| default_party.set(e.value()),
                }
                div { class: "spacer" }
                // A styled <label> opens the file picker without JavaScript, the
                // same way the cover image does.
                label { r#for: "vcf-file", class: "md-button md-button--outlined",
                    Icon { name: "upload" }
                    span { class: "md-button__label",
                        if importing() {
                            "{s.guests_importing}"
                        } else {
                            "{s.guests_import_button}"
                        }
                    }
                }
                input {
                    id: "vcf-file",
                    class: "file-input",
                    r#type: "file",
                    accept: ".vcf,text/vcard,text/x-vcard,text/directory",
                    onchange: import,
                }
                Button { icon: "add", disabled: adding(), onclick: add,
                    if adding() { "{s.guests_adding}" } else { "{s.guests_add_button}" }
                }
            }
            p { class: "md-body-small md-on-surface-variant", "{s.guests_import_help}" }
        }

        section { class: "md-card md-card--elevated stack",
            div { class: "admin__section-title",
                h2 { class: "md-title-large", "{s.guests_list_title}" }
                span { class: "md-body-small md-on-surface-variant",
                    {s.guests_n_invited.replace("{n}", &guests.len().to_string())}
                }
            }

            if guests.is_empty() {
                EmptyState {
                    icon: "group",
                    title: s.guests_none_title.to_owned(),
                    body: s.guests_none_body.to_owned(),
                }
            } else {
                div { class: "guest-table-wrap",
                    table { class: "guest-table",
                        thead {
                            tr {
                                th { "{s.col_guest}" }
                                th { "{s.col_reply}" }
                                th { class: "numeric", "{s.col_party}" }
                                th { "{s.col_link}" }
                                th { }
                            }
                        }
                        tbody {
                            for guest in guests.clone() {
                                GuestRow {
                                    key: "{guest.id}",
                                    guest: guest.clone(),
                                    origin: origin(),
                                    on_edit: move |g: GuestDto| {
                                        edit_name.set(g.name.clone());
                                        edit_phone.set(g.phone.clone());
                                        edit_party.set(g.max_party_size.to_string());
                                        editing.set(Some(g));
                                    },
                                    on_remove: move |g| removing.set(Some(g)),
                                    on_reissued: move |name: String| {
                                        toast.set(Some((
                                            s.guests_link_reissued.replace("{}", &name),
                                            false,
                                        )));
                                        reload.restart();
                                    },
                                    on_reset: move |name: String| {
                                        toast.set(Some((
                                            s.guests_reply_cleared.replace("{}", &name),
                                            false,
                                        )));
                                        reload.restart();
                                    },
                                    on_error: move |text| toast.set(Some((text, true))),
                                    on_sent: move |_| reload.restart(),
                                    on_message: move |text| toast.set(Some((text, false))),
                                    token: tok(),
                                }
                            }
                        }
                    }
                }
            }
        }

        Dialog {
            open: editing().is_some(),
            title: s.guests_edit_title.to_owned(),
            onclose: move |_| editing.set(None),
            actions: rsx! {
                Button {
                    kind: ButtonKind::Text,
                    onclick: move |_| editing.set(None),
                    "{s.cancel}"
                }
                Button { onclick: save_edit, "{s.save}" }
            },
            TextField {
                label: s.guests_name.to_owned(),
                value: edit_name(),
                oninput: move |e: FormEvent| edit_name.set(e.value()),
            }
            TextField {
                label: s.guests_phone.to_owned(),
                value: edit_phone(),
                input_type: "tel".to_owned(),
                supporting: s.guests_phone_help.to_owned(),
                oninput: move |e: FormEvent| edit_phone.set(e.value()),
            }
            SelectField {
                label: s.guests_may_bring.to_owned(),
                value: edit_party(),
                options: (1..=20)
                    .map(|n| (n.to_string(), locale.may_bring(n)))
                    .collect::<Vec<_>>(),
                onchange: move |e: FormEvent| edit_party.set(e.value()),
            }
        }

        Dialog {
            open: removing().is_some(),
            title: s.guests_remove_title.to_owned(),
            onclose: move |_| removing.set(None),
            actions: rsx! {
                Button {
                    kind: ButtonKind::Text,
                    onclick: move |_| removing.set(None),
                    "{s.cancel}"
                }
                Button {
                    kind: ButtonKind::Danger,
                    onclick: confirm_remove,
                    "{s.guests_remove_button}"
                }
            },
            p { class: "md-body-medium",
                if let Some(guest) = removing() {
                    {s.guests_remove_body.replace("{}", &guest.name)}
                }
            }
        }
    }
}

/// One row of the guest table.
#[component]
fn GuestRow(
    token: String,
    guest: GuestDto,
    origin: String,
    on_edit: EventHandler<GuestDto>,
    on_remove: EventHandler<GuestDto>,
    on_reissued: EventHandler<String>,
    on_reset: EventHandler<String>,
    on_error: EventHandler<String>,
    /// Called once this guest's sent marker has changed, so the list reloads.
    on_sent: EventHandler<()>,
    /// An informational message for the host — not an error.
    on_message: EventHandler<String>,
) -> Element {
    let s = t();
    let locale = active();
    let tok = use_signal(|| token);
    let id = guest.id;
    let name = guest.name.clone();
    let (tone, icon) = match guest.status {
        Rsvp::Attending => (Tone::Positive, "check_circle"),
        Rsvp::Declined => (Tone::Negative, "cancel_circle"),
        Rsvp::Pending => (Tone::Waiting, "schedule"),
    };

    // Built here rather than in a click handler so the WhatsApp control can be a
    // real link: an `href` is immune to popup blocking, and on a phone it hands
    // straight to the installed app.
    let link = crate::contacts::invite_link(&origin, &guest.token);
    let message = crate::contacts::invite_message(s.invite_message, &guest.name, &link);
    let whatsapp_url = crate::contacts::wa_me(&guest.phone, &message);

    let mark_sent = move |_| {
        spawn(async move {
            match api::mark_invite_sent(tok(), id, true).await {
                Ok(_) => on_sent.call(()),
                Err(e) => on_error.call(message_of(&e)),
            }
        });
    };

    let clear_sent = move |_| {
        spawn(async move {
            match api::mark_invite_sent(tok(), id, false).await {
                Ok(_) => on_sent.call(()),
                Err(e) => on_error.call(message_of(&e)),
            }
        });
    };

    let share = {
        let message = message.clone();
        move |_| {
            let message = message.clone();
            spawn(async move {
                // Web Share is the right mechanism — it reaches every app on the
                // device rather than the ones we happened to think of — but it
                // exists only on a secure origin, and desktop Firefox has no
                // share sheet at all. The clipboard is the honest fallback, and
                // the host is told which of the two actually happened.
                let script = format!(
                    "const text = {}; \
                     if (navigator.share) {{ \
                         try {{ await navigator.share({{ text }}); return \"shared\"; }} \
                         catch (e) {{ \
                             if (e && e.name === \"AbortError\") return \"cancelled\"; \
                         }} \
                     }} \
                     try {{ await navigator.clipboard.writeText(text); return \"copied\"; }} \
                     catch (e) {{ return \"failed\"; }}",
                    js_string(&message),
                );
                let evaluated = document::eval(&script).await;
                let outcome = evaluated
                    .as_ref()
                    .ok()
                    .and_then(|value| value.as_str())
                    .unwrap_or("failed");
                match outcome {
                    // The host dismissed the sheet; nothing was sent.
                    "cancelled" => {}
                    "failed" => on_error.call(s.err_share_failed.to_owned()),
                    handed_off => {
                        if handed_off == "copied" {
                            on_message.call(s.share_copied.to_owned());
                        }
                        match api::mark_invite_sent(tok(), id, true).await {
                            Ok(_) => on_sent.call(()),
                            Err(e) => on_error.call(message_of(&e)),
                        }
                    }
                }
            });
        }
    };

    let reissue = {
        let name = name.clone();
        move |_| {
            let name = name.clone();
            spawn(async move {
                match api::reissue_invite(tok(), id).await {
                    Ok(_) => on_reissued.call(name),
                    Err(e) => on_error.call(message_of(&e)),
                }
            });
        }
    };

    let reset = {
        let name = name.clone();
        move |_| {
            let name = name.clone();
            spawn(async move {
                match api::reset_guest(tok(), id).await {
                    Ok(()) => on_reset.call(name),
                    Err(e) => on_error.call(message_of(&e)),
                }
            });
        }
    };

    rsx! {
        tr {
            td {
                span { class: "guest-table__name", "{guest.name}" }
                if !guest.note.is_empty() {
                    span { class: "guest-table__note", "“{guest.note}”" }
                }
                if !guest.responded_at.is_empty() {
                    span { class: "guest-table__note",
                        {s.guests_replied_at
                            .replace("{}", &crate::types::format_timestamp(&guest.responded_at))}
                    }
                }
                if !guest.phone.is_empty() {
                    span { class: "guest-table__note", dir: "ltr", "{guest.phone}" }
                }
                if !guest.invite_sent_at.is_empty() {
                    // The marker doubles as the control that clears it, so
                    // tracking who has been messaged costs the row no further
                    // button.
                    button {
                        class: "guest-table__note guest-table__sent",
                        r#type: "button",
                        title: "{s.action_mark_unsent}",
                        onclick: clear_sent,
                        {s.guests_sent_at.replace(
                            "{}",
                            &crate::types::format_timestamp(&guest.invite_sent_at),
                        )}
                    }
                }
            }
            td {
                Chip { label: locale.status(guest.status).to_owned(), tone, icon }
            }
            td { class: "numeric",
                if guest.status == Rsvp::Attending {
                    "{guest.party_size} / {guest.max_party_size}"
                } else {
                    "— / {guest.max_party_size}"
                }
            }
            td {
                div { class: "guest-table__link",
                    code { "/i/{guest.token}" }
                    IconButton {
                        icon: "copy",
                        label: s.action_copy_link.to_owned(),
                        onclick: {
                            let origin = origin.clone();
                            let guest_token = guest.token.clone();
                            move |_| copy_invite_link(origin.clone(), &guest_token)
                        },
                    }
                }
            }
            td {
                div { class: "guest-table__actions",
                    a {
                        class: "md-icon-button",
                        href: "{whatsapp_url}",
                        target: "_blank",
                        rel: "noopener",
                        title: "{s.action_send_whatsapp}",
                        "aria-label": "{s.action_send_whatsapp}",
                        onclick: mark_sent,
                        Icon { name: "whatsapp" }
                    }
                    IconButton {
                        icon: "share",
                        label: s.action_share.to_owned(),
                        onclick: share,
                    }
                    IconButton {
                        icon: "edit",
                        label: s.action_edit_guest.to_owned(),
                        onclick: {
                            let guest = guest.clone();
                            move |_| on_edit.call(guest.clone())
                        },
                    }
                    if guest.status != Rsvp::Pending {
                        IconButton {
                            icon: "refresh",
                            label: s.action_clear_reply.to_owned(),
                            onclick: reset,
                        }
                    }
                    IconButton {
                        icon: "link",
                        label: s.action_new_link.to_owned(),
                        onclick: reissue,
                    }
                    IconButton {
                        icon: "delete",
                        label: s.action_remove_guest.to_owned(),
                        danger: true,
                        onclick: {
                            let guest = guest.clone();
                            move |_| on_remove.call(guest.clone())
                        },
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Responses tab
// ---------------------------------------------------------------------------

#[component]
fn ResponsesTab(token: String, event_id: i64, guests: Vec<GuestDto>) -> Element {
    let s = t();
    let locale = active();
    let attending = guests
        .iter()
        .filter(|g| g.status == Rsvp::Attending)
        .count();
    let declined = guests.iter().filter(|g| g.status == Rsvp::Declined).count();
    let pending = guests.iter().filter(|g| g.status == Rsvp::Pending).count();
    let head_count = expected_head_count(&guests);
    let answered: Vec<GuestDto> = guests
        .iter()
        .filter(|g| g.status != Rsvp::Pending)
        .cloned()
        .collect();

    rsx! {
        section { class: "md-card md-card--elevated stack",
            div { class: "stat-row",
                Stat {
                    value: guests.len().to_string(),
                    caption: s.stat_invited.to_owned(),
                }
                Stat {
                    value: attending.to_string(),
                    caption: s.stat_coming.to_owned(),
                    tone: Tone::Positive,
                }
                Stat {
                    value: declined.to_string(),
                    caption: s.stat_declined.to_owned(),
                    tone: Tone::Negative,
                }
                Stat {
                    value: pending.to_string(),
                    caption: s.stat_waiting.to_owned(),
                    tone: Tone::Waiting,
                }
                Stat {
                    value: head_count.to_string(),
                    caption: s.stat_expected.to_owned(),
                    tone: Tone::Primary,
                }
            }
            div { class: "form-actions",
                // A real link, so the browser saves the file the server names.
                a {
                    class: "md-button md-button--tonal",
                    href: "/admin/{token}/events/{event_id}/responses.csv",
                    Icon { name: "download" }
                    span { class: "md-button__label", "{s.responses_download}" }
                }
            }
        }

        section { class: "md-card md-card--elevated stack",
            h2 { class: "md-title-large", "{s.responses_title}" }
            if answered.is_empty() {
                EmptyState {
                    icon: "mail",
                    title: s.responses_none_title.to_owned(),
                    body: s.responses_none_body.to_owned(),
                }
            } else {
                div { class: "guest-table-wrap",
                    table { class: "guest-table",
                        thead {
                            tr {
                                th { "{s.col_guest}" }
                                th { "{s.col_reply}" }
                                th { class: "numeric", "{s.col_party}" }
                                th { "{s.col_note}" }
                                th { "{s.col_when}" }
                            }
                        }
                        tbody {
                            for guest in answered {
                                tr { key: "{guest.id}",
                                    td { class: "guest-table__name", "{guest.name}" }
                                    td {
                                        if guest.status == Rsvp::Attending {
                                            Chip {
                                                label: locale.status(guest.status).to_owned(),
                                                tone: Tone::Positive,
                                                icon: "check_circle",
                                            }
                                        } else {
                                            Chip {
                                                label: locale.status(guest.status).to_owned(),
                                                tone: Tone::Negative,
                                                icon: "cancel_circle",
                                            }
                                        }
                                    }
                                    td { class: "numeric",
                                        if guest.status == Rsvp::Attending {
                                            "{guest.party_size}"
                                        } else {
                                            "—"
                                        }
                                    }
                                    td { "{guest.note}" }
                                    td { "{crate::types::format_timestamp(&guest.responded_at)}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
