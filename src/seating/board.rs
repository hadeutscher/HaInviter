//! The seating chart's drawing surface: a [haboard] scene on a canvas.
//!
//! haboard is a GPU sprite engine driven by a [`winit`] event loop, and a
//! browser page gets exactly one event loop, bound for good to the canvas it was
//! created with. That single fact shapes everything here:
//!
//! * The loop is started once, on the first chart anyone opens, and then runs
//!   for the life of the page. Closing the window hides the canvas rather than
//!   stopping anything, and [`hide`] is what stops the frames.
//! * The loop owns the scene, so the rest of the application cannot reach in and
//!   change it. Charts are handed over through [`show`] and picked up on the
//!   next turn of the loop; arrangements come back the other way through a
//!   signal, which is the one thing a Dioxus component can be written to from
//!   outside its own render.
//! * A new chart means a new scene, and a scene owns its engine, so switching
//!   events rebuilds both. haboard's collection can be added to but not emptied,
//!   and a stale guest left on the chart would be worse than a moment's rebuild.
//!
//! [haboard]: https://crates.io/crates/haboard

use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

use dioxus::prelude::{Signal, WritableExt};
use haboard::{Drawable, Engine, ImageData, Scene, SceneMode};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, TouchPhase, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys},
    window::{Window, WindowId},
};

use crate::{
    seating::{self, Rect, TOKEN_H, TOKEN_W},
    types::SeatPlacement,
};

/// Z-order of the venue plan. Everyone stands on top of it, and nothing the
/// scene does — including bringing a clicked token to the front — ever produces
/// a lower value.
const BACKDROP_Z: f32 = -1.0;

// ---------------------------------------------------------------------------
// What gets drawn
// ---------------------------------------------------------------------------

/// Which of the two things on the chart a [`Piece`] is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The plan of the venue.
    Backdrop,
    /// One arriving person.
    Token { guest_id: i64, seat_index: i64 },
}

/// One drawable on the chart.
struct Piece {
    kind: Kind,
    x: f32,
    y: f32,
    z: f32,
    w: f32,
    h: f32,
    image: ImageData,
}

impl Piece {
    fn is_backdrop(&self) -> bool {
        self.kind == Kind::Backdrop
    }
}

impl Drawable for Piece {
    fn x(&self) -> f32 {
        self.x
    }

    fn y(&self) -> f32 {
        self.y
    }

    fn width(&self) -> f32 {
        self.w
    }

    fn height(&self) -> f32 {
        self.h
    }

    fn z(&self) -> f32 {
        self.z
    }

    fn set_z(&mut self, z: f32) {
        if !self.is_backdrop() {
            self.z = z;
        }
    }

    fn image(&self) -> ImageData {
        self.image.clone()
    }

    fn set_position(&mut self, x: f32, y: f32) {
        if !self.is_backdrop() {
            self.x = x;
            self.y = y;
        }
    }

    fn locked(&self) -> bool {
        self.is_backdrop()
    }

    /// The plan is scenery, not furniture.
    ///
    /// Refusing every hit is what keeps it from being picked up, rubber-band
    /// selected, nudged with the arrow keys or deleted — in either scene mode,
    /// and without the chart having to guard each of those separately. The
    /// token case is the trait's own bounding-box rule, restated because
    /// overriding the method replaces it.
    fn hit_test_point(&self, px: f32, py: f32) -> bool {
        !self.is_backdrop()
            && px >= self.x
            && px < self.x + self.w
            && py >= self.y
            && py < self.y + self.h
    }

    fn hit_test_rect(&self, rx: f32, ry: f32, rw: f32, rh: f32) -> bool {
        !self.is_backdrop()
            && !(rx >= self.x + self.w
                || rx + rw <= self.x
                || ry >= self.y + self.h
                || ry + rh <= self.y)
    }

    /// Nobody can be copied into existence. The guest list decides who is in the
    /// room; the chart only decides where they stand, so Ctrl+V has nothing to
    /// paste.
    fn try_clone(&self) -> Option<Self> {
        None
    }
}

// ---------------------------------------------------------------------------
// What the board is asked to draw
// ---------------------------------------------------------------------------

/// One person's token, before the board has decided where it goes.
pub struct Pending {
    pub guest_id: i64,
    pub seat_index: i64,
    pub image: ImageData,
    /// Where this person was last left, as a fraction of the venue plan, or
    /// `None` for someone who has never been placed and so waits in the tray.
    pub at: Option<(f64, f64)>,
}

/// One event's chart, ready to draw.
pub struct Plan {
    /// The venue plan and the size it was drawn at, or `None` for a blank sheet.
    pub map: Option<(ImageData, f32, f32)>,
    pub tokens: Vec<Pending>,
}

// ---------------------------------------------------------------------------
// The handover
// ---------------------------------------------------------------------------

thread_local! {
    /// The chart waiting to be drawn, left here by [`show`] and collected by the
    /// board on its next turn round the loop.
    static NEXT: RefCell<Option<Plan>> = const { RefCell::new(None) };
    /// Where the board reports an arrangement worth saving.
    static SINK: RefCell<Option<Signal<Option<Vec<SeatPlacement>>>>> = const { RefCell::new(None) };
    /// Whether the chart is on screen. A hidden canvas is not worth a frame.
    static ON_SCREEN: Cell<bool> = const { Cell::new(false) };
    /// Whether the one event loop this page gets has been started.
    static RUNNING: Cell<bool> = const { Cell::new(false) };
}

/// Draws `plan` on the canvas with the given element id, starting the event loop
/// if this is the first chart the page has opened.
///
/// `sink` is written every time the arrangement changes, which is how the saved
/// chart follows what is on screen.
pub fn show(canvas_id: &str, plan: Plan, sink: Signal<Option<Vec<SeatPlacement>>>) {
    SINK.with(|slot| *slot.borrow_mut() = Some(sink));
    NEXT.with(|slot| *slot.borrow_mut() = Some(plan));
    ON_SCREEN.set(true);
    if !RUNNING.get() {
        start(canvas_id);
    }
}

/// Takes the chart off screen. The loop keeps running — the page only gets the
/// one — but stops asking for frames.
pub fn hide() {
    ON_SCREEN.set(false);
}

fn start(canvas_id: &str) {
    let Some(canvas) = canvas(canvas_id) else {
        return complain("seating: the chart's canvas is not in the document");
    };
    let event_loop = match EventLoop::<Ready>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(e) => return complain(&format!("seating: no event loop: {e}")),
    };
    RUNNING.set(true);
    event_loop.set_control_flow(ControlFlow::Poll);
    let proxy = event_loop.create_proxy();
    event_loop.spawn_app(Board {
        canvas,
        proxy,
        window: None,
        scene: None,
        next: None,
        building: false,
        map_size: None,
        stage: Rect::default(),
        scale: 1.0,
        deferred: false,
        pointers: 0,
        refit_pending: false,
    });
}

fn canvas(id: &str) -> Option<HtmlCanvasElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id(id)?
        .dyn_into()
        .ok()
}

/// Reports a problem the chart cannot recover from. There is no other log on
/// this side of the wire, and a blank rectangle with nothing in the console is
/// the worst way to find out the GPU is unavailable.
fn complain(message: &str) {
    web_sys::console::error_1(&JsValue::from_str(message));
}

// ---------------------------------------------------------------------------
// The board
// ---------------------------------------------------------------------------

/// An engine that has finished initialising, with the chart it was built for.
struct Ready {
    engine: Engine,
    plan: Plan,
}

struct Board {
    canvas: HtmlCanvasElement,
    proxy: EventLoopProxy<Ready>,
    window: Option<Arc<Window>>,
    scene: Option<Box<Scene<Piece>>>,
    /// A chart that should be on screen but is not yet.
    next: Option<Plan>,
    /// Whether an engine is being built for `next` right now.
    building: bool,
    /// The venue plan's own size, kept so the stage can be refitted on a resize.
    map_size: Option<(f32, f32)>,
    /// The plan's current rectangle: the frame every placement is measured
    /// against.
    stage: Rect,
    /// Device pixel ratio, so a token is the same size to the eye on any screen.
    ///
    /// Token rectangles follow it, but their textures do not: those are
    /// rasterised once, at the ratio in force when the chart was built. Zooming
    /// in therefore costs sharpness until the chart is reopened. Closing that
    /// properly needs a way to re-upload one drawable's image, which haboard
    /// does not yet expose.
    scale: f32,
    /// Whether a nudge is being held back until the key it came from is
    /// released. See [`commits`].
    deferred: bool,
    /// How many pointers are down: one for a held mouse button, one per finger.
    pointers: u32,
    /// Whether a refit is owed once the last of them lifts. See
    /// [`track_pointers`](Board::track_pointers).
    refit_pending: bool,
}

impl Board {
    /// Starts building an engine for the chart that is waiting, if any.
    fn build(&mut self) {
        if self.building || self.next.is_none() {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(plan) = self.next.take() else {
            return;
        };
        // Release the old surface before the new engine asks the same canvas for
        // one.
        self.scene = None;
        self.building = true;
        let proxy = self.proxy.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let engine = Engine::new(window).await;
            let _ = proxy.send_event(Ready { engine, plan });
        });
    }

    /// Fills a fresh scene with the venue plan and everyone on it.
    fn install(&mut self, engine: Engine, plan: Plan) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let mut scene = Scene::new(engine, Vec::new(), SceneMode::Edit);
        // The canvas has had its CSS size all along, but the `Resized` event
        // that said so arrived while there was no scene to receive it.
        scene.resize(window.inner_size());

        let (width, height) = scene.size();
        let (width, height) = (width as f32, height as f32);
        self.scale = window.scale_factor() as f32;
        self.map_size = plan.map.as_ref().map(|&(_, w, h)| (w, h));
        self.stage = seating::stage_rect(width, height, self.map_size, self.scale);

        if let Some((image, ..)) = plan.map {
            scene.drawables.push(Piece {
                kind: Kind::Backdrop,
                x: self.stage.x,
                y: self.stage.y,
                z: BACKDROP_Z,
                w: self.stage.w,
                h: self.stage.h,
                image,
            });
        }

        let (w, h) = (TOKEN_W * self.scale, TOKEN_H * self.scale);
        let mut waiting = 0usize;
        for token in plan.tokens {
            let (x, y) = match token.at {
                Some((nx, ny)) => seating::place(self.stage, nx, ny),
                None => {
                    let slot = seating::tray_slot(waiting, height, self.scale);
                    waiting += 1;
                    slot
                }
            };
            scene.drawables.push(Piece {
                kind: Kind::Token {
                    guest_id: token.guest_id,
                    seat_index: token.seat_index,
                },
                x,
                y,
                z: 0.0,
                w,
                h,
                image: token.image,
            });
        }

        scene.render();
        self.scene = Some(Box::new(scene));
    }

    /// Refits the plan after the window changes shape, carrying everyone on it
    /// along by the fraction of the plan they were standing on.
    fn refit(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let scale = window.scale_factor() as f32;
        let map_size = self.map_size;
        let was = self.stage;
        let Some(scene) = &mut self.scene else {
            return;
        };
        let (width, height) = scene.size();
        let now = seating::stage_rect(width as f32, height as f32, map_size, scale);
        if now == was && scale == self.scale {
            return;
        }
        // A token is a fixed size to the eye, not a fixed number of pixels, so
        // its rectangle is re-derived whenever the device pixel ratio moves
        // under it — which browser zoom does, as does dragging the window to a
        // monitor of a different density.
        let (w, h) = (TOKEN_W * scale, TOKEN_H * scale);
        for piece in scene.drawables.iter_mut() {
            if piece.is_backdrop() {
                piece.x = now.x;
                piece.y = now.y;
                piece.w = now.w;
                piece.h = now.h;
            } else {
                let (nx, ny) = seating::normalise(was, piece.x, piece.y);
                (piece.x, piece.y) = seating::place(now, nx, ny);
                piece.w = w;
                piece.h = h;
            }
        }
        self.stage = now;
        self.scale = scale;
    }

    /// Counts pointers going down and coming up.
    ///
    /// A refit moves every token so it stays on the same spot of the venue
    /// plan. haboard, though, captures a drag's starting positions in pixels
    /// when the pointer goes down and recomputes `start + delta` on every move,
    /// so anything the chart does to a dragged token underneath a live gesture
    /// is discarded on the very next move — leaving it placed against a stage
    /// that no longer exists, permanently, because no later refit will disturb
    /// it. A refit arriving mid-gesture is therefore held until the last pointer
    /// lifts.
    ///
    /// Deferring is not merely safer, it is also the correct frame: the venue
    /// plan does not move while the refit is held, so the spot the guest was
    /// dropped on is the spot on the *old* stage, which is exactly what the
    /// deferred refit measures against.
    fn track_pointers(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.pointers += 1,
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => self.pointers = self.pointers.saturating_sub(1),
            WindowEvent::Touch(touch) => match touch.phase {
                TouchPhase::Started => self.pointers += 1,
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    self.pointers = self.pointers.saturating_sub(1)
                }
                TouchPhase::Moved => {}
            },
            // A window that loses focus never sees the button come back up.
            WindowEvent::Focused(false) => self.pointers = 0,
            _ => {}
        }
    }

    /// Hands the arrangement now on screen back to the application.
    fn publish(&self) {
        let Some(scene) = &self.scene else {
            return;
        };
        let stage = self.stage;
        let seats: Vec<SeatPlacement> = scene
            .drawables
            .iter()
            .filter_map(|piece| match piece.kind {
                Kind::Token {
                    guest_id,
                    seat_index,
                } => {
                    let (x, y) = seating::normalise(stage, piece.x, piece.y);
                    Some(SeatPlacement {
                        guest_id,
                        seat_index,
                        x,
                        y,
                    })
                }
                Kind::Backdrop => None,
            })
            .collect();
        SINK.with(|slot| {
            if let Some(mut sink) = *slot.borrow() {
                sink.set(Some(seats));
            }
        });
    }
}

/// What handling `event` means for saving.
enum Commit {
    /// A change finished; publish it.
    Now,
    /// A change happened but more are coming; hold it until they stop.
    Defer,
    /// Nothing worth saving.
    No,
}

/// Whether handling `event` finished a change worth saving.
///
/// A held arrow key repeats at the keyboard's own rate and every repeat is a
/// real nudge, so treating each as finished asks the server to rewrite the whole
/// chart tens of times a second for what the guest arranging it experiences as
/// one gesture. Repeats are deferred instead: the position that matters is the
/// one the key comes up on, and [`publish`] sends the entire arrangement anyway,
/// so nothing is lost by waiting for it.
///
/// [`publish`]: Board::publish
fn commits(event: &WindowEvent) -> Commit {
    match event {
        WindowEvent::MouseInput {
            state: ElementState::Released,
            button: MouseButton::Left,
            ..
        } => Commit::Now,
        WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    state: ElementState::Pressed,
                    repeat,
                    ..
                },
            ..
        } => {
            if *repeat {
                Commit::Defer
            } else {
                Commit::Now
            }
        }
        WindowEvent::Touch(touch) => {
            if matches!(touch.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                Commit::Now
            } else {
                Commit::No
            }
        }
        _ => Commit::No,
    }
}

/// Whether `event` ends a run of key repeats, and so releases whatever
/// [`Commit::Defer`] has been holding.
///
/// A key coming up is never "handled" by the scene — it only acts on presses —
/// so this is asked before the handled check rather than through it. Losing
/// focus counts too: a window that goes away mid-nudge still has to save.
fn flushes(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::KeyboardInput {
            event: winit::event::KeyEvent {
                state: ElementState::Released,
                ..
            },
            ..
        } | WindowEvent::Focused(false)
    )
}

impl ApplicationHandler<Ready> for Board {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes().with_canvas(Some(self.canvas.clone()));
        match event_loop.create_window(attributes) {
            Ok(window) => self.window = Some(Arc::new(window)),
            Err(e) => return complain(&format!("seating: no window: {e}")),
        }
        self.build();
    }

    fn user_event(&mut self, _: &ActiveEventLoop, ready: Ready) {
        self.building = false;
        self.install(ready.engine, ready.plan);
        // A second chart may have arrived while the first was still building.
        self.build();
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(plan) = NEXT.with(|slot| slot.borrow_mut().take()) {
            self.next = Some(plan);
            self.build();
        }
        if ON_SCREEN.get()
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::RedrawRequested) {
            if let Some(scene) = &mut self.scene {
                scene.render();
            }
            return;
        }
        let handled = match &mut self.scene {
            Some(scene) => scene.handle_window_event(&event),
            None => return,
        };
        self.track_pointers(&event);
        if matches!(event, WindowEvent::Resized(_)) {
            self.refit_pending = true;
        }
        if self.refit_pending && self.pointers == 0 {
            self.refit_pending = false;
            self.refit();
        }
        if flushes(&event) && self.deferred {
            self.deferred = false;
            self.publish();
            return;
        }
        if handled {
            match commits(&event) {
                Commit::Now => {
                    self.deferred = false;
                    self.publish();
                }
                Commit::Defer => self.deferred = true,
                Commit::No => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rasterising
// ---------------------------------------------------------------------------
//
// Text is the one thing a sprite engine cannot draw, so the browser draws it.
// Painting a token on a 2D canvas and handing the pixels to the GPU also gets
// bidirectional text, font fallback and emoji right — none of which belongs in
// this application.

/// Paints one guest token: a rounded chip in the guest's colour with their name
/// across it.
pub fn token_image(label: &str, hue: f64, scale: f32) -> Option<ImageData> {
    let (width, height) = (TOKEN_W as f64, TOKEN_H as f64);
    let (canvas, ctx) = surface(
        (TOKEN_W * scale).round() as u32,
        (TOKEN_H * scale).round() as u32,
    )?;
    ctx.scale(scale as f64, scale as f64).ok()?;

    rounded_rect(
        &ctx,
        1.0,
        1.0,
        width - 2.0,
        height - 2.0,
        height / 2.0 - 1.0,
    );
    ctx.set_fill_style_str(&format!("hsl({hue}deg 70% 90%)"));
    ctx.fill();
    ctx.set_stroke_style_str(&format!("hsl({hue}deg 45% 42%)"));
    ctx.set_line_width(1.5);
    ctx.stroke();

    ctx.set_fill_style_str(&format!("hsl({hue}deg 55% 22%)"));
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    ctx.set_font("600 13px system-ui, -apple-system, \"Segoe UI\", sans-serif");
    // A name longer than the chip is squeezed rather than clipped: a squashed
    // "Konstantinopoulos family" is still readable, half of one is not.
    ctx.fill_text_with_max_width(label, width / 2.0, height / 2.0, width - 20.0)
        .ok()?;

    pixels(&ctx, canvas.width(), canvas.height())
}

/// Fetches the venue plan and hands it over as pixels, with the size it was
/// drawn at.
///
/// The browser decodes it rather than the engine so that its dimensions are
/// known before the stage is laid out: the plan's shape is what decides the
/// rectangle every placement is measured against.
pub async fn map_image(url: &str) -> Option<(ImageData, f32, f32)> {
    let image = load(url).await?;
    let (width, height) = (image.natural_width(), image.natural_height());
    if width == 0 || height == 0 {
        return None;
    }
    let (canvas, ctx) = surface(width, height)?;
    ctx.draw_image_with_html_image_element(&image, 0.0, 0.0)
        .ok()?;
    let data = pixels(&ctx, canvas.width(), canvas.height())?;
    Some((data, width as f32, height as f32))
}

/// Waits for one image element to finish loading.
async fn load(url: &str) -> Option<HtmlImageElement> {
    let image = HtmlImageElement::new().ok()?;
    // Without this a plan served from another origin taints the canvas and its
    // pixels cannot be read back at all. With it, a server that permits the read
    // works and one that does not fails cleanly, which is the difference between
    // a missing backdrop and a missing chart.
    image.set_cross_origin(Some("anonymous"));
    let settled = js_sys::Promise::new(&mut |resolve, reject| {
        image.set_onload(Some(&resolve));
        image.set_onerror(Some(&reject));
    });
    image.set_src(url);
    wasm_bindgen_futures::JsFuture::from(settled).await.ok()?;
    Some(image)
}

fn surface(width: u32, height: u32) -> Option<(HtmlCanvasElement, CanvasRenderingContext2d)> {
    let canvas: HtmlCanvasElement = web_sys::window()?
        .document()?
        .create_element("canvas")
        .ok()?
        .dyn_into()
        .ok()?;
    canvas.set_width(width.max(1));
    canvas.set_height(height.max(1));
    let ctx: CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
    Some((canvas, ctx))
}

fn pixels(ctx: &CanvasRenderingContext2d, width: u32, height: u32) -> Option<ImageData> {
    let data = ctx
        .get_image_data(0.0, 0.0, width as f64, height as f64)
        .ok()?;
    Some(ImageData::rgba(width, height, data.data().0))
}

fn rounded_rect(ctx: &CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
    ctx.begin_path();
    ctx.move_to(x + r, y);
    ctx.line_to(x + w - r, y);
    ctx.quadratic_curve_to(x + w, y, x + w, y + r);
    ctx.line_to(x + w, y + h - r);
    ctx.quadratic_curve_to(x + w, y + h, x + w - r, y + h);
    ctx.line_to(x + r, y + h);
    ctx.quadratic_curve_to(x, y + h, x, y + h - r);
    ctx.line_to(x, y + r);
    ctx.quadratic_curve_to(x, y, x + r, y);
    ctx.close_path();
}
