//! A small Material 3 component set.
//!
//! Rather than pull in a JavaScript component library, the Material design
//! language is implemented directly: these components emit the markup that
//! `assets/material.css` styles, using Material's own tokens for colour, shape,
//! elevation, typography and state layers. Nothing here fetches anything at
//! runtime, which keeps the invitation pages fast on a phone and the server
//! dependency-free.

use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

/// Path data for the icon set, keyed by name.
///
/// Hand-drawn on Material's 24×24 grid so the bundle carries no icon font.
fn icon_path(name: &str) -> &'static str {
    match name {
        "add" => "M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6z",
        "close" => {
            "M6.4 5 5 6.4 10.6 12 5 17.6 6.4 19 12 13.4 17.6 19 19 17.6 13.4 12 19 6.4 17.6 5 12 10.6z"
        }
        "check" => "M9.6 17.6 4 12l1.4-1.4 4.2 4.2 8.9-8.9L20 7.3z",
        "check_circle" => {
            "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20m-1.2 14.6L6.6 12.4 8 11l2.8 2.8L16.6 8 18 9.4z"
        }
        "cancel_circle" => {
            "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20m3.6 13.2-1.4 1.4L12 13.4l-2.2 2.2-1.4-1.4L10.6 12 8.4 9.8l1.4-1.4L12 10.6l2.2-2.2 1.4 1.4L13.4 12z"
        }
        "delete" => {
            "M9 3h6l1 2h4v2H4V5h4zM6 8h12l-.9 12.1A2 2 0 0 1 15.1 22H8.9a2 2 0 0 1-2-1.9zm4 3v8h1.5v-8zm3 0v8h1.5v-8z"
        }
        "copy" => {
            "M15 1H5a2 2 0 0 0-2 2v13h2V3h10zm4 4H9a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2m0 16H9V7h10z"
        }
        "download" => "M12 3v10.2l3.6-3.6L17 11l-5 5-5-5 1.4-1.4L11 13.2V3zM5 19h14v2H5z",
        "upload" => "M12 16V6.8L8.4 10.4 7 9l5-5 5 5-1.4 1.4L13 6.8V16zM5 18h14v2H5z",
        "place" => {
            "M12 2a7 7 0 0 0-7 7c0 5.2 7 13 7 13s7-7.8 7-13a7 7 0 0 0-7-7m0 9.5A2.5 2.5 0 1 1 12 6.5a2.5 2.5 0 0 1 0 5z"
        }
        "event" => {
            "M5 4h2V2h2v2h6V2h2v2h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2m14 6H5v10h14z"
        }
        "schedule" => {
            "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20m0 18a8 8 0 1 1 0-16 8 8 0 0 1 0 16m1-13h-2v6l5 3 1-1.7-4-2.3z"
        }
        "person" => {
            "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8m0 2c-4.4 0-8 2.2-8 5v1h16v-1c0-2.8-3.6-5-8-5"
        }
        "group" => {
            "M8 12a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7m0 1.5c-3.9 0-7 1.9-7 4.3V20h14v-2.2c0-2.4-3.1-4.3-7-4.3M17 12a3 3 0 1 0 0-6 3 3 0 0 0 0 6m0 1.5c-.7 0-1.4.1-2 .3 1.8.9 3 2.3 3 4V20h5v-2c0-2.2-2.7-4.5-6-4.5"
        }
        "edit" => "M3 17.2 16.9 3.3l3.8 3.8L6.8 21H3zM18.7 1.5l2.8 2.8-1.7 1.7-2.8-2.8z",
        "arrow_back" => "M20 11H7.8l5.6-5.6L12 4l-8 8 8 8 1.4-1.4L7.8 13H20z",
        "link" => {
            "M3.9 12A3.1 3.1 0 0 1 7 8.9h4V7H7a5 5 0 0 0 0 10h4v-1.9H7A3.1 3.1 0 0 1 3.9 12M8 13h8v-2H8zm9-6h-4v1.9h4a3.1 3.1 0 0 1 0 6.2h-4V17h4a5 5 0 0 0 0-10"
        }
        "refresh" => "M12 5V1L7 6l5 5V7a5 5 0 1 1-4.9 6h-2A7 7 0 1 0 12 5",
        "mail" => {
            "M4 5h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1m15 3.3-7 4.4-7-4.4V17h14zM18.3 7H5.7l6.3 4z"
        }
        "expand_more" => "M7.4 8.6 12 13.2l4.6-4.6L18 10l-6 6-6-6z",
        "error" => "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20m1 15h-2v-2h2zm0-4h-2V7h2z",
        // The WhatsApp mark: the bubble is drawn as a ring and the handset as
        // its own subpath, so both read correctly against any surface colour.
        "whatsapp" => {
            "M12 2a10 10 0 0 0-8.5 15.2L2 22l4.9-1.5A10 10 0 1 0 12 2m0 2a8 8 0 1 1-4.2 14.8l-.4-.2-2.3.7.7-2.2-.3-.4A8 8 0 0 1 12 4m-2.6 4.1c-.2 0-.5 0-.7.3-.3.3-.9.9-.9 2.1s.9 2.4 1 2.6c.2.2 1.8 2.7 4.4 3.7 2 .8 2.5.7 2.9.6.7-.1 1.7-.7 2-1.4.2-.7.2-1.2.2-1.4l-.6-.3-1.7-.8c-.2-.1-.4-.1-.6.1l-.8 1c-.1.2-.3.2-.5.1a6.5 6.5 0 0 1-1.9-1.2 7.3 7.3 0 0 1-1.3-1.7c-.1-.2 0-.4.1-.5l.4-.5.3-.5v-.5l-.8-1.8c-.2-.5-.4-.4-.6-.4z"
        }
        "share" => {
            "M18 16.1c-.8 0-1.5.3-2 .8l-7.1-4.2q.1-.35.1-.7t-.1-.7L16 7.1c.5.5 1.2.8 2 .8a3 3 0 1 0-3-3q0 .35.1.7L8 9.9a3 3 0 1 0 0 4.2l7.1 4.2q-.1.3-.1.6a2.9 2.9 0 1 0 2.9-2.9z"
        }
        "search" => {
            "M15.5 14h-.8l-.3-.3a6.5 6.5 0 1 0-.7.7l.3.3v.8l5 5 1.5-1.5zM10 14a4 4 0 1 1 0-8 4 4 0 0 1 0 8"
        }
        // An unknown name renders nothing rather than a broken glyph.
        _ => "",
    }
}

/// A 24×24 inline SVG icon. `name` must be one of the names in [`icon_path`].
#[component]
pub fn Icon(name: &'static str, #[props(default)] class: Option<String>) -> Element {
    let extra = class.unwrap_or_default();
    rsx! {
        svg {
            class: "md-icon {extra}",
            view_box: "0 0 24 24",
            width: "24",
            height: "24",
            "aria-hidden": "true",
            path { d: icon_path(name), fill: "currentColor" }
        }
    }
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// The Material button emphases this application uses.
///
/// Not every variant is reachable through [`Button`]: the same CSS modifiers are
/// applied directly to the `<a>` that downloads a CSV and the `<label>` that
/// opens the file picker, because those must stay real HTML elements.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonKind {
    /// Highest emphasis: the single primary action on a screen.
    #[default]
    Filled,
    /// Medium emphasis, filled with a container colour.
    Tonal,
    /// Medium emphasis, outlined.
    Outlined,
    /// Lowest emphasis, no container.
    Text,
    /// Destructive action, rendered in the error colour.
    Danger,
}

impl ButtonKind {
    const fn modifier(self) -> &'static str {
        match self {
            Self::Filled => "filled",
            Self::Tonal => "tonal",
            Self::Outlined => "outlined",
            Self::Text => "text",
            Self::Danger => "danger",
        }
    }
}

/// A Material button. Wrap the label in the children.
#[component]
pub fn Button(
    #[props(default)] kind: ButtonKind,
    #[props(default)] disabled: bool,
    #[props(default)] full_width: bool,
    icon: Option<&'static str>,
    class: Option<String>,
    #[props(default)] onclick: EventHandler<MouseEvent>,
    children: Element,
) -> Element {
    let wide = if full_width { "md-button--wide" } else { "" };
    let extra = class.unwrap_or_default();
    rsx! {
        button {
            class: "md-button md-button--{kind.modifier()} {wide} {extra}",
            r#type: "button",
            disabled,
            onclick: move |e| onclick.call(e),
            if let Some(name) = icon {
                Icon { name }
            }
            span { class: "md-button__label", {children} }
        }
    }
}

/// A borderless, circular icon-only button.
#[component]
pub fn IconButton(
    icon: &'static str,
    label: String,
    #[props(default)] disabled: bool,
    #[props(default)] danger: bool,
    #[props(default)] onclick: EventHandler<MouseEvent>,
) -> Element {
    let tone = if danger { "md-icon-button--danger" } else { "" };
    rsx! {
        button {
            class: "md-icon-button {tone}",
            r#type: "button",
            title: "{label}",
            "aria-label": "{label}",
            disabled,
            onclick: move |e| onclick.call(e),
            Icon { name: icon }
        }
    }
}

/// An extended floating action button, pinned bottom-right by the stylesheet.
#[component]
pub fn Fab(
    icon: &'static str,
    label: String,
    #[props(default)] onclick: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        button {
            class: "md-fab",
            r#type: "button",
            onclick: move |e| onclick.call(e),
            Icon { name: icon }
            span { class: "md-fab__label", "{label}" }
        }
    }
}

// ---------------------------------------------------------------------------
// Text fields
// ---------------------------------------------------------------------------

/// A filled Material text field. The `<label>` wraps the control, so clicking
/// the label focuses it without needing generated ids.
#[component]
pub fn TextField(
    label: String,
    value: String,
    #[props(default)] oninput: EventHandler<FormEvent>,
    input_type: Option<String>,
    supporting: Option<String>,
    #[props(default)] disabled: bool,
    #[props(default)] error: bool,
) -> Element {
    let kind = input_type.unwrap_or_else(|| "text".to_owned());
    // Date and time controls always show content, so their label must stay up.
    let always_raised = matches!(kind.as_str(), "date" | "time" | "datetime-local");
    let raised = if always_raised {
        "md-field--raised"
    } else {
        ""
    };
    let invalid = if error { "md-field--error" } else { "" };
    rsx! {
        div { class: "md-field-wrap",
            label { class: "md-field {raised} {invalid}",
                input {
                    class: "md-field__input",
                    r#type: "{kind}",
                    value: "{value}",
                    placeholder: " ",
                    disabled,
                    oninput: move |e| oninput.call(e),
                }
                span { class: "md-field__label", "{label}" }
            }
            if let Some(text) = supporting {
                span { class: "md-field__supporting", "{text}" }
            }
        }
    }
}

/// A multi-line Material text field.
#[component]
pub fn TextArea(
    label: String,
    value: String,
    #[props(default)] oninput: EventHandler<FormEvent>,
    #[props(default = 4)] rows: i64,
    supporting: Option<String>,
    #[props(default)] disabled: bool,
) -> Element {
    rsx! {
        div { class: "md-field-wrap",
            label { class: "md-field md-field--multiline",
                textarea {
                    class: "md-field__input",
                    rows: "{rows}",
                    placeholder: " ",
                    disabled,
                    // The value belongs in the attribute, never as a text
                    // child: a dynamic child is wrapped in hydration marker
                    // comments, and inside a <textarea> those markers are the
                    // field's literal content — they showed up as the default
                    // text of the note box.
                    value: "{value}",
                    oninput: move |e| oninput.call(e),
                }
                span { class: "md-field__label", "{label}" }
            }
            if let Some(text) = supporting {
                span { class: "md-field__supporting", "{text}" }
            }
        }
    }
}

/// A filled Material select. Options are supplied as `(value, label)` pairs.
#[component]
pub fn SelectField(
    label: String,
    value: String,
    options: Vec<(String, String)>,
    #[props(default)] onchange: EventHandler<FormEvent>,
    #[props(default)] disabled: bool,
) -> Element {
    rsx! {
        div { class: "md-field-wrap",
            label { class: "md-field md-field--raised md-field--select",
                select {
                    class: "md-field__input",
                    disabled,
                    onchange: move |e| onchange.call(e),
                    for (v , text) in options {
                        option { key: "{v}", value: "{v}", selected: v == value, "{text}" }
                    }
                }
                span { class: "md-field__label", "{label}" }
                Icon { name: "expand_more", class: "md-field__trailing".to_owned() }
            }
        }
    }
}

/// A Material switch with a trailing label.
#[component]
pub fn Switch(
    label: String,
    checked: bool,
    #[props(default)] onchange: EventHandler<bool>,
) -> Element {
    rsx! {
        label { class: "md-switch",
            input {
                class: "md-switch__input",
                r#type: "checkbox",
                checked,
                oninput: move |e: FormEvent| onchange.call(e.value() == "true"),
            }
            span { class: "md-switch__track", span { class: "md-switch__handle" } }
            span { class: "md-switch__label", "{label}" }
        }
    }
}

// ---------------------------------------------------------------------------
// Communication
// ---------------------------------------------------------------------------

/// Semantic colouring for chips and stat tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Primary,
    Positive,
    Negative,
    Waiting,
}

impl Tone {
    const fn modifier(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Primary => "primary",
            Self::Positive => "positive",
            Self::Negative => "negative",
            Self::Waiting => "waiting",
        }
    }
}

/// A compact, non-interactive Material chip.
#[component]
pub fn Chip(label: String, #[props(default)] tone: Tone, icon: Option<&'static str>) -> Element {
    rsx! {
        span { class: "md-chip md-chip--{tone.modifier()}",
            if let Some(name) = icon {
                Icon { name, class: "md-chip__icon".to_owned() }
            }
            "{label}"
        }
    }
}

/// A single figure with a caption, used for the response tallies.
#[component]
pub fn Stat(value: String, caption: String, #[props(default)] tone: Tone) -> Element {
    rsx! {
        div { class: "md-stat md-stat--{tone.modifier()}",
            span { class: "md-stat__value", "{value}" }
            span { class: "md-stat__caption", "{caption}" }
        }
    }
}

/// A modal dialog. Renders nothing when `open` is false.
#[component]
pub fn Dialog(
    open: bool,
    title: String,
    #[props(default)] onclose: EventHandler<()>,
    #[props(default)] wide: bool,
    actions: Element,
    children: Element,
) -> Element {
    if !open {
        return rsx! {};
    }
    let size = if wide { "md-dialog--wide" } else { "" };
    rsx! {
        div { class: "md-scrim", onclick: move |_| onclose.call(()),
            div {
                class: "md-dialog {size}",
                // Clicks inside the dialog must not reach the scrim behind it.
                onclick: move |e| e.stop_propagation(),
                h2 { class: "md-dialog__title", "{title}" }
                div { class: "md-dialog__content", {children} }
                div { class: "md-dialog__actions", {actions} }
            }
        }
    }
}

/// A message pinned to the bottom of the screen. Clicking it dismisses it.
#[component]
pub fn Snackbar(
    message: String,
    #[props(default)] error: bool,
    #[props(default)] onclick: EventHandler<MouseEvent>,
) -> Element {
    let tone = if error { "md-snackbar--error" } else { "" };
    rsx! {
        div {
            class: "md-snackbar {tone}",
            role: "status",
            onclick: move |e| onclick.call(e),
            if error {
                Icon { name: "error" }
            }
            span { "{message}" }
        }
    }
}

/// An indeterminate progress indicator.
#[component]
pub fn Loading(#[props(default)] label: Option<String>) -> Element {
    rsx! {
        div { class: "md-loading",
            div { class: "md-linear-progress", div { class: "md-linear-progress__bar" } }
            if let Some(text) = label {
                span { class: "md-loading__label", "{text}" }
            }
        }
    }
}

/// The Material "empty state": an icon, a headline and an explanation.
#[component]
pub fn EmptyState(icon: &'static str, title: String, body: String) -> Element {
    rsx! {
        div { class: "md-empty",
            Icon { name: icon, class: "md-empty__icon".to_owned() }
            p { class: "md-empty__title", "{title}" }
            p { class: "md-empty__body", "{body}" }
        }
    }
}
