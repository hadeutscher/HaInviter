//! The seating chart window.
//!
//! A full-screen surface over the admin panel on which every person who accepted
//! is a token to be dragged onto a plan of the venue. The drawing itself belongs
//! to [`crate::seating::board`]; what is here is the chrome around it, the data
//! it is fed, and the saving of whatever gets arranged.
//!
//! The window is mounted by the root component and never unmounted, only
//! hidden. That is not a stylistic choice: the browser gives a page one GPU
//! event loop, it is bound to the canvas below for good, and a canvas that came
//! and went with a route would leave the engine drawing into nothing.

use crate::{
    api,
    i18n::t,
    seating,
    types::SeatPlacement,
    ui::{
        material::{EmptyState, Icon, IconButton, Loading},
        message_of,
    },
};
use dioxus::prelude::*;

/// Which event's chart the window is showing, if any. Provided as context by the
/// root component; set it to open the window and clear it to close.
#[derive(Clone, Debug, PartialEq)]
pub struct SeatingTarget {
    pub token: String,
    pub event_id: i64,
    pub title: String,
}

/// The id of the canvas the engine draws on. It is looked up by name exactly
/// once per page, so it has to be stable and unique.
const CANVAS_ID: &str = "seating-surface";

/// What the window was able to load: the venue plan's URL, everyone who
/// accepted, and where they were last left.
type Chart = (String, Vec<seating::Arrival>, Vec<SeatPlacement>);

#[component]
pub fn SeatingWindow() -> Element {
    let s = t();
    let mut target = use_context::<Signal<Option<SeatingTarget>>>();

    // The open event's guest list and saved arrangement, reloaded whenever a
    // different event is opened.
    let chart = use_resource(move || async move {
        let open = target()?;
        let view = api::get_event(open.token.clone(), open.event_id).await;
        let seats = api::seating_plan(open.token.clone(), open.event_id).await;
        Some(match (view, seats) {
            (Ok(view), Ok(seats)) => {
                Ok((view.event.venue_map, seating::arrivals(&view.guests), seats))
            }
            (Err(e), _) | (_, Err(e)) => Err(message_of(&e)),
        })
    });

    // Where the board reports an arrangement worth saving. It is written from
    // inside the GPU event loop rather than from a component, which is exactly
    // what a signal is for.
    let mut arranged: Signal<Option<Vec<SeatPlacement>>> = use_signal(|| None);
    let mut saving = use_signal(|| false);
    let mut saved = use_signal(|| false);
    let mut failed: Signal<Option<String>> = use_signal(|| None);

    // Hand each freshly loaded chart to the board, and take the board off screen
    // when the window closes.
    use_effect(move || {
        let loaded = chart.read().clone().flatten();
        let open = target().is_some();
        if !open {
            hide();
            return;
        }
        let Some(Ok(chart)) = loaded else { return };
        saved.set(false);
        failed.set(None);
        spawn(async move { draw(chart, arranged).await });
    });

    // Save whatever the board reports, one arrangement at a time. A save in
    // flight picks up anything that lands while it runs, so holding down an
    // arrow key moves people without queueing a request per keystroke.
    use_effect(move || {
        if arranged.read().is_none() || saving() {
            return;
        }
        let Some(open) = target() else { return };
        saving.set(true);
        spawn(async move {
            loop {
                let Some(seats) = arranged.write().take() else {
                    break;
                };
                match api::save_seating(open.token.clone(), open.event_id, seats).await {
                    Ok(()) => saved.set(true),
                    Err(e) => {
                        failed.set(Some(message_of(&e)));
                        break;
                    }
                }
            }
            saving.set(false);
        });
    });

    let open = target().is_some();
    let title = target().map(|open| open.title).unwrap_or_default();
    let loaded = chart.read_unchecked().clone().flatten();

    let subtitle = match &loaded {
        Some(Ok((_, arrivals, _))) if arrivals.is_empty() => s.seating_nobody_title.to_owned(),
        Some(Ok((map, arrivals, _))) => {
            let people = s
                .seating_n_people
                .replace("{n}", &arrivals.len().to_string());
            if map.trim().is_empty() {
                format!("{people} · {}", s.seating_no_map)
            } else {
                people
            }
        }
        Some(Err(message)) => message.clone(),
        None => s.loading.to_owned(),
    };

    let status = match (saving(), failed(), saved()) {
        (true, ..) => s.saving.to_owned(),
        (_, Some(message), _) => message,
        (_, None, true) => s.seating_saved.to_owned(),
        _ => String::new(),
    };

    rsx! {
        div {
            class: if open { "seating" } else { "seating seating--closed" },
            // A hidden window is not merely off screen: nothing in it should be
            // reachable by a screen reader or the tab key either.
            "aria-hidden": if open { "false" } else { "true" },
            role: "dialog",
            "aria-label": "{s.seating_title}",

            header { class: "seating__bar",
                IconButton {
                    icon: "close",
                    label: s.seating_close.to_owned(),
                    onclick: move |_| target.set(None),
                }
                div { class: "seating__titles",
                    span { class: "seating__title", "{title}" }
                    span { class: "seating__subtitle", "{subtitle}" }
                }
                div { class: "spacer" }
                if !status.is_empty() {
                    span {
                        class: if failed().is_some() { "seating__status seating__status--error" } else { "seating__status" },
                        role: "status",
                        if failed().is_some() {
                            Icon { name: "error" }
                        }
                        "{status}"
                    }
                }
            }

            div { class: "seating__stage",
                // Never conditional and never moved: the engine binds to this
                // element once and holds it for the life of the page.
                canvas { id: CANVAS_ID, class: "seating__canvas", tabindex: "0" }
                match (open, &loaded) {
                    (true, None) => rsx! {
                        div { class: "seating__overlay",
                            Loading { label: s.seating_loading.to_owned() }
                        }
                    },
                    (true, Some(Ok((_, arrivals, _)))) if arrivals.is_empty() => rsx! {
                        div { class: "seating__overlay",
                            EmptyState {
                                icon: "group",
                                title: s.seating_nobody_title.to_owned(),
                                body: s.seating_nobody_body.to_owned(),
                            }
                        }
                    },
                    _ => rsx! {},
                }
            }

            p { class: "seating__hint", "{s.seating_hint}" }
        }
    }
}

/// Builds one chart's tokens and hands them to the board.
#[cfg(target_arch = "wasm32")]
async fn draw(chart: Chart, arranged: Signal<Option<Vec<SeatPlacement>>>) {
    use crate::seating::board;

    let (map, arrivals, seats) = chart;
    // Tokens are rasterised at the display's own resolution so the names on them
    // are not a blurry approximation on a retina screen. The cap keeps a very
    // dense display from turning a long guest list into a pile of large
    // textures.
    let scale = web_sys::window()
        .map(|window| window.device_pixel_ratio() as f32)
        .unwrap_or(1.0)
        .clamp(1.0, 3.0);

    let map = match map.trim() {
        "" => None,
        url => board::map_image(url).await,
    };
    let tokens = arrivals
        .into_iter()
        .filter_map(|arrival| {
            Some(board::Pending {
                guest_id: arrival.guest_id,
                seat_index: arrival.seat_index,
                at: seating::placement_of(&seats, &arrival),
                image: board::token_image(
                    &arrival.label(),
                    seating::hue_for(arrival.guest_id),
                    scale,
                )?,
            })
        })
        .collect();

    board::show(CANVAS_ID, board::Plan { map, tokens }, arranged);
}

/// On the server the window is markup and nothing else: it renders so that the
/// canvas is in the document the instant the page hydrates, but there is no
/// screen here to draw on.
#[cfg(not(target_arch = "wasm32"))]
async fn draw(_chart: Chart, _arranged: Signal<Option<Vec<SeatPlacement>>>) {}

#[cfg(target_arch = "wasm32")]
fn hide() {
    crate::seating::board::hide();
}

#[cfg(not(target_arch = "wasm32"))]
fn hide() {}
