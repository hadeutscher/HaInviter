//! The admin panel.
//!
//! Authorisation is the URL: every screen carries the admin token in its path
//! and passes it to each server call. Nothing is cached in the browser, so
//! closing the tab ends the session.

use crate::{
    Route, api,
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
    rsx! {
        header { class: "admin__bar",
            if let Some(route) = back {
                Link { to: route, class: "md-icon-button", "aria-label": "Back to events",
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
    rsx! {
        main { class: "page page--centred",
            div { class: "md-card md-card--elevated landing",
                Icon { name: "error", class: "landing__mark".to_owned() }
                h1 { class: "md-headline-medium", "This admin link is not valid" }
                p { class: "md-body-large md-on-surface-variant",
                    "A fresh admin link is printed to the server log every time HaInviter starts."
                }
                p { class: "md-body-small md-on-surface-variant landing__note",
                    "Run "
                    code { "kubectl logs deploy/hainviter" }
                    " to read it, or set HAINVITER_ADMIN_TOKEN to keep the link stable."
                }
            }
        }
    }
}

/// The chips summarising one event's replies.
#[component]
fn ResponseChips(summary: EventSummary) -> Element {
    rsx! {
        div { class: "event-card__chips",
            Chip {
                label: format!("{} coming", summary.attending),
                tone: Tone::Positive,
                icon: "check_circle",
            }
            Chip {
                label: format!("{} declined", summary.declined),
                tone: Tone::Negative,
                icon: "cancel_circle",
            }
            Chip {
                label: format!("{} waiting", summary.pending),
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
            toast.set(Some(("An event needs a title.".to_owned(), true)));
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
        document::Title { "Events — HaInviter admin" }
        main { class: "page admin",
            AdminBar {
                title: "Events".to_owned(),
                subtitle: "HaInviter admin".to_owned(),
                actions: rsx! {
                    IconButton {
                        icon: "refresh",
                        label: "Reload".to_owned(),
                        onclick: move |_| events.restart(),
                    }
                },
            }

            div { class: "admin__content",
                div { class: "token-callout",
                    Icon { name: "link" }
                    p {
                        "Anyone with this page's address can administer HaInviter. Keep it out of
                         chats and screenshots — restarting the server issues a new one."
                    }
                }

                match &*events.read_unchecked() {
                    Some(Ok(list)) if list.is_empty() => rsx! {
                        div { class: "md-card md-card--outlined",
                            EmptyState {
                                icon: "event",
                                title: "No events yet".to_owned(),
                                body: "Create an event, then add the people you want to invite. Each
                                       guest gets their own private link."
                                    .to_owned(),
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
                                                "{crate::types::format_event_datetime(&summary.starts_at)}"
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
                                            "{summary.guest_count} invited · {summary.head_count} expected"
                                        }
                                    }
                                    ResponseChips { summary: summary.clone() }
                                }
                            }
                        }
                    },
                    Some(Err(_)) => rsx! { NotAuthorised {} },
                    None => rsx! { Loading { label: "Loading events…".to_owned() } },
                }
            }

            Fab {
                icon: "add",
                label: "New event".to_owned(),
                onclick: move |_| {
                    form.set(EventInput { allow_plus_ones: true, ..EventInput::default() });
                    creating.set(true);
                },
            }

            Dialog {
                open: creating(),
                wide: true,
                title: "New event".to_owned(),
                onclose: move |_| creating.set(false),
                actions: rsx! {
                    Button {
                        kind: ButtonKind::Text,
                        onclick: move |_| creating.set(false),
                        "Cancel"
                    }
                    Button { disabled: saving(), onclick: create,
                        if saving() { "Creating…" } else { "Create event" }
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
            upload_error.set(Some("That image is larger than 8 MB.".to_owned()));
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
                Err(e) => upload_error.set(Some(format!("Could not read that file: {e}"))),
            }
            uploading.set(false);
        });
    };

    rsx! {
        div { class: "form-grid",
            div { class: "form-span-2",
                TextField {
                    label: "Event title".to_owned(),
                    value: current.title.clone(),
                    supporting: "Shown as the invitation's headline.".to_owned(),
                    oninput: move |e: FormEvent| form.with_mut(|f| f.title = e.value()),
                }
            }
            TextField {
                label: "Hosted by".to_owned(),
                value: current.hosts.clone(),
                supporting: "e.g. Dana & Yuval".to_owned(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.hosts = e.value()),
            }
            TextField {
                label: "Starts".to_owned(),
                value: current.starts_at.clone(),
                input_type: "datetime-local".to_owned(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.starts_at = e.value()),
            }
            TextField {
                label: "Location".to_owned(),
                value: current.location.clone(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.location = e.value()),
            }
            TextField {
                label: "Map link".to_owned(),
                value: current.location_url.clone(),
                supporting: "Optional — opens in the guest's maps app.".to_owned(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.location_url = e.value()),
            }
            TextField {
                label: "Reply by".to_owned(),
                value: current.rsvp_deadline.clone(),
                input_type: "date".to_owned(),
                supporting: "After this date the form becomes read-only.".to_owned(),
                oninput: move |e: FormEvent| form.with_mut(|f| f.rsvp_deadline = e.value()),
            }
            div { class: "form-span-2",
                TextArea {
                    label: "Invitation text".to_owned(),
                    value: current.description.clone(),
                    rows: 5,
                    supporting: "Leave a blank line between paragraphs.".to_owned(),
                    oninput: move |e: FormEvent| form.with_mut(|f| f.description = e.value()),
                }
            }
            div { class: "form-span-2 stack",
                span { class: "md-label-large md-on-surface-variant", "Cover image" }
                div { class: "cover-preview",
                    if current.cover_image.is_empty() {
                        img { src: "", alt: "" }
                    } else {
                        img { src: "{current.cover_image}", alt: "Cover preview" }
                    }
                    div { class: "inline-actions",
                        // A styled <label> opens the file picker without JavaScript.
                        label { r#for: "cover-file", class: "md-button md-button--outlined",
                            Icon { name: "upload" }
                            span { class: "md-button__label",
                                if uploading() { "Uploading…" } else { "Upload image" }
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
                                "Remove"
                            }
                        }
                    }
                }
                TextField {
                    label: "…or paste an image URL".to_owned(),
                    value: current.cover_image.clone(),
                    oninput: move |e: FormEvent| form.with_mut(|f| f.cover_image = e.value()),
                }
                if let Some(text) = upload_error() {
                    span { class: "invite__error", Icon { name: "error" } "{text}" }
                }
            }
            div { class: "form-span-2",
                Switch {
                    label: "Guests may bring the people on their invitation".to_owned(),
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
                        title: "Loading…".to_owned(),
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
                subtitle: format!(
                    "{} invited · {} expected",
                    view.guests.len(),
                    expected_head_count(&view.guests),
                ),
                back: Route::AdminEvents { token: tok() },
                actions: rsx! {
                    IconButton {
                        icon: "refresh",
                        label: "Reload".to_owned(),
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
                        "Details"
                    }
                    button {
                        class: if tab() == Tab::Guests { "admin-tab is-active" } else { "admin-tab" },
                        r#type: "button",
                        onclick: move |_| tab.set(Tab::Guests),
                        "Guests ({view.guests.len()})"
                    }
                    button {
                        class: if tab() == Tab::Responses { "admin-tab is-active" } else { "admin-tab" },
                        r#type: "button",
                        onclick: move |_| tab.set(Tab::Responses),
                        "Responses"
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
                    toast.set(Some(("Event saved.".to_owned(), false)));
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
                    "Delete event"
                }
                Button { icon: "check", disabled: saving(), onclick: save,
                    if saving() { "Saving…" } else { "Save changes" }
                }
            }
        }

        Dialog {
            open: confirming_delete(),
            title: "Delete this event?".to_owned(),
            onclose: move |_| confirming_delete.set(false),
            actions: rsx! {
                Button {
                    kind: ButtonKind::Text,
                    onclick: move |_| confirming_delete.set(false),
                    "Keep it"
                }
                Button { kind: ButtonKind::Danger, onclick: delete, "Delete event" }
            },
            p { class: "md-body-medium",
                "The event and its guest list will be removed, and every invitation link for it
                 will stop working. Replies already received stay in the audit log and in the
                 database's reply history, but they will no longer be listed here or exported."
            }
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
    let tok = use_signal(|| token);
    let origin = use_signal(|| base_url);
    let mut toast = toast;
    let mut reload = reload;

    let mut bulk = use_signal(String::new);
    let mut default_party = use_signal(|| "1".to_owned());
    let mut adding = use_signal(|| false);
    let mut editing: Signal<Option<GuestDto>> = use_signal(|| None);
    let mut edit_name = use_signal(String::new);
    let mut edit_party = use_signal(|| "1".to_owned());
    let mut removing: Signal<Option<GuestDto>> = use_signal(|| None);

    let add = move |_| {
        let raw = bulk();
        if raw.trim().is_empty() {
            toast.set(Some(("Type at least one name.".to_owned(), true)));
            return;
        }
        let size = default_party().parse::<i64>().unwrap_or(1);
        adding.set(true);
        spawn(async move {
            match api::add_guests(tok(), event_id, raw, size).await {
                Ok(0) => toast.set(Some((
                    "Everyone on that list was already invited.".to_owned(),
                    false,
                ))),
                Ok(n) => {
                    bulk.set(String::new());
                    toast.set(Some((
                        format!("Added {n} guest{}.", if n == 1 { "" } else { "s" }),
                        false,
                    )));
                    reload.restart();
                }
                Err(e) => toast.set(Some((message_of(&e), true))),
            }
            adding.set(false);
        });
    };

    let save_edit = move |_| {
        let Some(guest) = editing() else { return };
        let name = edit_name();
        let size = edit_party().parse::<i64>().unwrap_or(1);
        spawn(async move {
            match api::update_guest(tok(), guest.id, name, size).await {
                Ok(()) => {
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
                    toast.set(Some((format!("Removed {}.", guest.name), false)));
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
            h2 { class: "md-title-large", "Add guests" }
            TextArea {
                label: "One name per line".to_owned(),
                value: bulk(),
                rows: 6,
                supporting: "Add \", 4\" after a name to let that guest bring up to four people. \
                             Names already invited are skipped."
                    .to_owned(),
                oninput: move |e: FormEvent| bulk.set(e.value()),
            }
            div { class: "inline-actions",
                SelectField {
                    label: "Default party size".to_owned(),
                    value: default_party(),
                    options: (1..=10).map(|n| (n.to_string(), n.to_string())).collect::<Vec<_>>(),
                    onchange: move |e: FormEvent| default_party.set(e.value()),
                }
                div { class: "spacer" }
                Button { icon: "add", disabled: adding(), onclick: add,
                    if adding() { "Adding…" } else { "Add to guest list" }
                }
            }
        }

        section { class: "md-card md-card--elevated stack",
            div { class: "admin__section-title",
                h2 { class: "md-title-large", "Guest list" }
                span { class: "md-body-small md-on-surface-variant",
                    "{guests.len()} invited"
                }
            }

            if guests.is_empty() {
                EmptyState {
                    icon: "group",
                    title: "Nobody invited yet".to_owned(),
                    body: "Paste your guest list above. Each name gets a private invitation link."
                        .to_owned(),
                }
            } else {
                div { class: "guest-table-wrap",
                    table { class: "guest-table",
                        thead {
                            tr {
                                th { "Guest" }
                                th { "Reply" }
                                th { class: "numeric", "Party" }
                                th { "Invitation link" }
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
                                        edit_party.set(g.max_party_size.to_string());
                                        editing.set(Some(g));
                                    },
                                    on_remove: move |g| removing.set(Some(g)),
                                    on_reissued: move |name: String| {
                                        toast.set(Some((format!("New link issued for {name}."), false)));
                                        reload.restart();
                                    },
                                    on_reset: move |name: String| {
                                        toast.set(Some((format!("Cleared {name}'s reply."), false)));
                                        reload.restart();
                                    },
                                    on_error: move |text| toast.set(Some((text, true))),
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
            title: "Edit guest".to_owned(),
            onclose: move |_| editing.set(None),
            actions: rsx! {
                Button { kind: ButtonKind::Text, onclick: move |_| editing.set(None), "Cancel" }
                Button { onclick: save_edit, "Save" }
            },
            TextField {
                label: "Name".to_owned(),
                value: edit_name(),
                oninput: move |e: FormEvent| edit_name.set(e.value()),
            }
            SelectField {
                label: "May bring up to".to_owned(),
                value: edit_party(),
                options: (1..=20)
                    .map(|n| (
                        n.to_string(),
                        if n == 1 { "just themselves".to_owned() } else { format!("{n} people") },
                    ))
                    .collect::<Vec<_>>(),
                onchange: move |e: FormEvent| edit_party.set(e.value()),
            }
        }

        Dialog {
            open: removing().is_some(),
            title: "Remove this guest?".to_owned(),
            onclose: move |_| removing.set(None),
            actions: rsx! {
                Button { kind: ButtonKind::Text, onclick: move |_| removing.set(None), "Cancel" }
                Button { kind: ButtonKind::Danger, onclick: confirm_remove, "Remove guest" }
            },
            p { class: "md-body-medium",
                if let Some(guest) = removing() {
                    "{guest.name} will be removed and their invitation link will stop working.
                     Any reply they already sent stays in the audit log."
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
) -> Element {
    let tok = use_signal(|| token);
    let id = guest.id;
    let name = guest.name.clone();
    let (tone, icon) = match guest.status {
        Rsvp::Attending => (Tone::Positive, "check_circle"),
        Rsvp::Declined => (Tone::Negative, "cancel_circle"),
        Rsvp::Pending => (Tone::Waiting, "schedule"),
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
                        "replied {crate::types::format_timestamp(&guest.responded_at)}"
                    }
                }
            }
            td {
                Chip { label: guest.status.label().to_owned(), tone, icon }
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
                        label: "Copy invitation link".to_owned(),
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
                    IconButton {
                        icon: "edit",
                        label: "Edit guest".to_owned(),
                        onclick: {
                            let guest = guest.clone();
                            move |_| on_edit.call(guest.clone())
                        },
                    }
                    if guest.status != Rsvp::Pending {
                        IconButton {
                            icon: "refresh",
                            label: "Clear this reply".to_owned(),
                            onclick: reset,
                        }
                    }
                    IconButton {
                        icon: "link",
                        label: "Issue a new link".to_owned(),
                        onclick: reissue,
                    }
                    IconButton {
                        icon: "delete",
                        label: "Remove guest".to_owned(),
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
                    caption: "Invited".to_owned(),
                }
                Stat {
                    value: attending.to_string(),
                    caption: "Coming".to_owned(),
                    tone: Tone::Positive,
                }
                Stat {
                    value: declined.to_string(),
                    caption: "Declined".to_owned(),
                    tone: Tone::Negative,
                }
                Stat {
                    value: pending.to_string(),
                    caption: "Waiting".to_owned(),
                    tone: Tone::Waiting,
                }
                Stat {
                    value: head_count.to_string(),
                    caption: "People expected".to_owned(),
                    tone: Tone::Primary,
                }
            }
            div { class: "form-actions",
                // A real link, so the browser saves the file the server names.
                a {
                    class: "md-button md-button--tonal",
                    href: "/admin/{token}/events/{event_id}/responses.csv",
                    Icon { name: "download" }
                    span { class: "md-button__label", "Download CSV" }
                }
            }
        }

        section { class: "md-card md-card--elevated stack",
            h2 { class: "md-title-large", "Replies" }
            if answered.is_empty() {
                EmptyState {
                    icon: "mail",
                    title: "No replies yet".to_owned(),
                    body: "Replies appear here as guests open their links. Every one is also
                           written to the audit log."
                        .to_owned(),
                }
            } else {
                div { class: "guest-table-wrap",
                    table { class: "guest-table",
                        thead {
                            tr {
                                th { "Guest" }
                                th { "Reply" }
                                th { class: "numeric", "Party" }
                                th { "Note" }
                                th { "When" }
                            }
                        }
                        tbody {
                            for guest in answered {
                                tr { key: "{guest.id}",
                                    td { class: "guest-table__name", "{guest.name}" }
                                    td {
                                        if guest.status == Rsvp::Attending {
                                            Chip {
                                                label: "Coming".to_owned(),
                                                tone: Tone::Positive,
                                                icon: "check_circle",
                                            }
                                        } else {
                                            Chip {
                                                label: "Declined".to_owned(),
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
