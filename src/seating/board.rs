//! The seating chart's drawing surface: a [haboard] scene on a canvas.
//!
//! haboard's core knows nothing about windowing, so the chart owns everything
//! around it — the canvas, the DOM listeners, the resize observer and the frame
//! loop — and feeds the scene [`Input`]s. Nothing here adopts an event loop, and
//! winit is not in the dependency graph at all.
//!
//! Two consequences worth knowing:
//!
//! * The scene is ordinary state, not something a foreign loop owns, so the
//!   chart is handed over by calling [`show`] and the arrangement comes back
//!   through a signal because that is how a Dioxus component is written to from
//!   outside its own render — not because anything is being smuggled past a
//!   framework.
//! * `Engine::from_canvas` is asynchronous, since asking for a GPU adapter and
//!   device are both futures in a browser. That is the one piece of ceremony the
//!   design cannot remove, and it shows up here as a scene that is `None` until
//!   it is ready.
//!
//! [haboard]: https://crates.io/crates/haboard

use std::{cell::RefCell, rc::Rc};

use dioxus::prelude::{Signal, WritableExt};
use haboard::{Drawable, Engine, ImageData, Input, PointerId, PointerPhase, Scene, SceneMode, web};
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use web_sys::{
    HtmlCanvasElement, KeyboardEvent, PointerEvent, ResizeObserver, ResizeObserverBoxOptions,
    ResizeObserverOptions,
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

/// What a token says and what colour it says it in.
///
/// Kept so the token can be drawn again at a different resolution: browser zoom
/// changes the device pixel ratio, and a label rasterised for the old one is
/// blurry at the new.
#[derive(Clone)]
struct Face {
    label: String,
    hue: f64,
}

/// One drawable on the chart: the venue plan, or one arriving person.
struct Piece {
    /// `None` for the venue plan, which has no label and cannot be touched.
    face: Option<Face>,
    guest_id: i64,
    seat_index: i64,
    x: f32,
    y: f32,
    z: f32,
    w: f32,
    h: f32,
    image: ImageData,
}

impl Piece {
    fn is_backdrop(&self) -> bool {
        self.face.is_none()
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
    pub label: String,
    pub hue: f64,
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
// The board
// ---------------------------------------------------------------------------

struct Board {
    canvas: HtmlCanvasElement,
    /// `None` until the GPU has finished coming up.
    scene: Option<Scene<Piece>>,
    /// A chart that should be on screen but is not yet.
    next: Option<Plan>,
    /// Whether an engine is being built right now.
    building: bool,
    /// The venue plan's own size, kept so the stage can be refitted.
    map_size: Option<(f32, f32)>,
    /// The plan's current rectangle: the frame every placement is measured
    /// against.
    stage: Rect,
    /// Device pixel ratio. Token rectangles and their textures both follow it,
    /// so a token is the same size, and equally sharp, at any zoom level.
    scale: f32,
    /// Whether a refit is owed once the drag it would disturb has ended.
    refit_pending: bool,
    /// Whether the chart is on screen. A hidden canvas is not worth a frame.
    on_screen: bool,
    /// Where an arrangement worth saving is reported.
    sink: Option<Signal<Option<Vec<SeatPlacement>>>>,
}

impl Board {
    /// Feeds one input to the scene and acts on what it reports.
    ///
    /// Returns whether the scene handled it, which is what decides whether the
    /// browser's own default behaviour is suppressed.
    fn input(&mut self, input: Input) -> bool {
        let Some(scene) = &mut self.scene else {
            return false;
        };
        let response = scene.handle(input);
        if response.commit.is_now() {
            self.publish();
        }
        // A refit held back while a drag was live can run as soon as it ends.
        if self.refit_pending && !self.is_dragging() {
            self.refit();
        }
        response.handled
    }

    fn is_dragging(&self) -> bool {
        self.scene.as_ref().is_some_and(|scene| scene.is_dragging())
    }

    /// Takes the chart's surface to the canvas's current size, re-rasterising
    /// every token if the device pixel ratio moved.
    fn resized(&mut self) {
        // The size the surface believes it has is what decides whether the
        // backing store needs touching: assigning `width`/`height` clears the
        // canvas, so it must not be done for a size it already is.
        let current = self.scene.as_ref().map_or((0, 0), |scene| scene.size());
        if let Some((width, height)) = web::resize_canvas_backing_store(&self.canvas, current) {
            self.input(Input::Resize { width, height });
        }

        let scale = (web::device_pixel_ratio() as f32).clamp(1.0, 3.0);
        if scale != self.scale {
            self.scale = scale;
            self.rescale_tokens();
        }
        // Held until the gesture it would disturb has finished: haboard keeps a
        // drag's starting positions in pixels, so moving a token underneath a
        // live drag is discarded on the next move and leaves it placed against
        // a stage that no longer exists.
        if self.is_dragging() {
            self.refit_pending = true;
        } else {
            self.refit();
        }
    }

    /// Draws every token again at the current ratio.
    ///
    /// A label is pixels by the time haboard sees it, so zooming in cannot
    /// sharpen one that was rasterised for a coarser display — it has to be
    /// redrawn and re-uploaded.
    fn rescale_tokens(&mut self) {
        let Some(scene) = &mut self.scene else {
            return;
        };
        let (w, h) = (TOKEN_W * self.scale, TOKEN_H * self.scale);
        let faces: Vec<_> = scene
            .drawables
            .iter_with_ids()
            .filter_map(|(id, piece)| piece.face.clone().map(|face| (id, face)))
            .collect();
        for (id, face) in faces {
            let Some(image) = token_image(&face.label, face.hue, self.scale) else {
                continue;
            };
            if let Some(piece) = scene.drawables.get_mut(id) {
                piece.image = image;
                piece.w = w;
                piece.h = h;
            }
            scene.drawables.refresh_image(id);
        }
        scene.request_redraw();
    }

    /// Refits the plan after the surface changes shape, carrying everyone on it
    /// along by the fraction of the plan they were standing on.
    fn refit(&mut self) {
        self.refit_pending = false;
        let (map_size, was, scale) = (self.map_size, self.stage, self.scale);
        let Some(scene) = &mut self.scene else {
            return;
        };
        let (width, height) = scene.size();
        let now = seating::stage_rect(width as f32, height as f32, map_size, scale);
        if now == was {
            return;
        }
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
        scene.request_redraw();
        self.stage = now;
    }

    /// Fills a fresh scene with the venue plan and everyone on it.
    fn install(&mut self, engine: Engine, plan: Plan) {
        let mut scene = Scene::new(engine, Vec::new(), SceneMode::Edit);
        let (width, height) = scene.size();
        let (width, height) = (width as f32, height as f32);

        self.map_size = plan.map.as_ref().map(|&(_, w, h)| (w, h));
        self.stage = seating::stage_rect(width, height, self.map_size, self.scale);

        if let Some((image, ..)) = plan.map {
            scene.drawables.push(Piece {
                face: None,
                guest_id: 0,
                seat_index: 0,
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
            let Some(image) = token_image(&token.label, token.hue, self.scale) else {
                continue;
            };
            let (x, y) = match token.at {
                Some((nx, ny)) => seating::place(self.stage, nx, ny),
                None => {
                    let slot = seating::tray_slot(waiting, height, self.scale);
                    waiting += 1;
                    slot
                }
            };
            scene.drawables.push(Piece {
                face: Some(Face {
                    label: token.label,
                    hue: token.hue,
                }),
                guest_id: token.guest_id,
                seat_index: token.seat_index,
                x,
                y,
                z: 0.0,
                w,
                h,
                image,
            });
        }

        scene.request_redraw();
        self.scene = Some(scene);
    }

    /// Hands the arrangement now on screen back to the application.
    fn publish(&self) {
        let (Some(scene), Some(mut sink)) = (&self.scene, self.sink) else {
            return;
        };
        let stage = self.stage;
        let seats: Vec<SeatPlacement> = scene
            .drawables
            .iter()
            .filter(|piece| !piece.is_backdrop())
            .map(|piece| {
                let (x, y) = seating::normalise(stage, piece.x, piece.y);
                SeatPlacement {
                    guest_id: piece.guest_id,
                    seat_index: piece.seat_index,
                    x,
                    y,
                }
            })
            .collect();
        sink.set(Some(seats));
    }

    /// Draws a frame, if there is anything new to look at.
    fn frame(&mut self) {
        if !self.on_screen {
            return;
        }
        if let Some(scene) = &mut self.scene
            && scene.needs_redraw()
        {
            scene.render();
        }
    }
}

// ---------------------------------------------------------------------------
// The one board a page has
// ---------------------------------------------------------------------------

thread_local! {
    /// The live chart. A page shows at most one, and it outlives any single
    /// render of the component that opened it.
    static BOARD: RefCell<Option<Rc<RefCell<Board>>>> = const { RefCell::new(None) };
}

/// Draws `plan` on the canvas with the given element id.
///
/// `sink` is written every time the arrangement changes, which is how the saved
/// chart follows what is on screen.
pub fn show(canvas_id: &str, plan: Plan, sink: Signal<Option<Vec<SeatPlacement>>>) {
    let board = match BOARD.with(|slot| slot.borrow().clone()) {
        Some(board) => board,
        None => {
            let Some(board) = start(canvas_id) else {
                return;
            };
            board
        }
    };
    {
        let mut board = board.borrow_mut();
        board.sink = Some(sink);
        board.on_screen = true;
        board.next = Some(plan);
    }
    build(&board);
}

/// Takes the chart off screen, saving anything a deferred nudge still owes.
pub fn hide() {
    let Some(board) = BOARD.with(|slot| slot.borrow().clone()) else {
        return;
    };
    let owed = {
        let mut board = board.borrow_mut();
        board.on_screen = false;
        board
            .scene
            .as_mut()
            .is_some_and(|scene| scene.take_pending_commit())
    };
    // A run of nudges is normally flushed by the key release that ends it. A
    // window closed mid-run never sees that release, and the change is already
    // applied to the scene, so it has to be saved on the way out.
    if owed {
        board.borrow().publish();
    }
}

/// Builds the engine for whatever chart is waiting, if one is.
///
/// A new chart means a new scene, and a scene owns its engine, so switching
/// events rebuilds both — haboard could now be asked to swap the contents in
/// place, but rebuilding also re-reads the canvas size and the pixel ratio,
/// which is what an event opened on a different screen needs.
fn build(board: &Rc<RefCell<Board>>) {
    let (canvas, plan) = {
        let mut state = board.borrow_mut();
        if state.building || state.next.is_none() {
            return;
        }
        let Some(plan) = state.next.take() else {
            return;
        };
        // Release the old surface before the new engine asks the same canvas
        // for one.
        state.scene = None;
        state.building = true;
        state.scale = (web::device_pixel_ratio() as f32).clamp(1.0, 3.0);
        (state.canvas.clone(), plan)
    };

    let board = Rc::clone(board);
    wasm_bindgen_futures::spawn_local(async move {
        // Nothing has a surface yet, so a zero "current" size asks for the
        // backing store to be set unconditionally.
        let size = web::resize_canvas_backing_store(&canvas, (0, 0))
            .unwrap_or_else(|| web::canvas_physical_size(&canvas));
        match Engine::from_canvas(canvas, size).await {
            Ok(engine) => {
                let mut state = board.borrow_mut();
                state.building = false;
                state.install(engine, plan);
            }
            Err(e) => {
                board.borrow_mut().building = false;
                // Distinguishing these matters: no adapter at all means the
                // browser has neither WebGPU nor WebGL and the person needs a
                // newer one; a device request refused after an adapter was
                // found is our bug, not theirs.
                complain(&format!("seating: the chart cannot be drawn: {e}"));
                return;
            }
        }
        // A second chart may have arrived while the first was building.
        build(&board);
    });
}

fn start(canvas_id: &str) -> Option<Rc<RefCell<Board>>> {
    let canvas: HtmlCanvasElement = web_sys::window()?
        .document()?
        .get_element_by_id(canvas_id)?
        .dyn_into()
        .ok()?;

    let board = Rc::new(RefCell::new(Board {
        canvas: canvas.clone(),
        scene: None,
        next: None,
        building: false,
        map_size: None,
        stage: Rect::default(),
        scale: (web::device_pixel_ratio() as f32).clamp(1.0, 3.0),
        refit_pending: false,
        on_screen: false,
        sink: None,
    }));

    listen(&board, &canvas);
    observe(&board, &canvas);
    animate(&board);

    BOARD.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&board)));
    Some(board)
}

/// Reports a problem the chart cannot recover from. There is no other log on
/// this side of the wire, and a blank rectangle with nothing in the console is
/// the worst way to find out the GPU is unavailable.
fn complain(message: &str) {
    web_sys::console::error_1(&JsValue::from_str(message));
}

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------
//
// haboard translates a DOM event into its own vocabulary but deliberately does
// not attach the listeners, because where they go is a policy the host owns.
// Both of the choices below are ours: the canvas is focusable and keys are read
// from it rather than from the window, so a closed chart cannot swallow the
// arrow keys of the page around it; and a pointer is a mouse or a finger
// according to `pointerType`, so a stylus drives the rubber band rather than
// being treated as one more finger.

/// Attaches the pointer and keyboard listeners to the canvas.
fn listen(board: &Rc<RefCell<Board>>, canvas: &HtmlCanvasElement) {
    for (name, phase) in [
        ("pointerdown", PointerPhase::Down),
        ("pointermove", PointerPhase::Move),
        ("pointerup", PointerPhase::Up),
        ("pointercancel", PointerPhase::Cancel),
    ] {
        let board = Rc::clone(board);
        let target = canvas.clone();
        let handler = Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
            let id = if event.pointer_type() == "touch" {
                PointerId::Touch(event.pointer_id() as u64)
            } else {
                PointerId::Mouse
            };
            let (x, y) = web::pointer_position(&event, &target);
            let mut state = board.borrow_mut();
            state.input(Input::Modifiers(web::modifiers_from_pointer_event(&event)));
            let handled = state.input(Input::Pointer { id, phase, x, y });
            drop(state);
            if phase == PointerPhase::Down {
                // Keeps the gesture alive when the pointer leaves the canvas,
                // and gives the canvas the keyboard so arrow keys reach the
                // scene rather than scrolling the page behind it.
                let _ = target.set_pointer_capture(event.pointer_id());
                let _ = target.unchecked_ref::<web_sys::HtmlElement>().focus();
            }
            if handled {
                event.prevent_default();
            }
        });
        let _ = canvas.add_event_listener_with_callback(name, handler.as_ref().unchecked_ref());
        handler.forget();
    }

    for (name, pressed) in [("keydown", true), ("keyup", false)] {
        let board = Rc::clone(board);
        let handler = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
            let mut state = board.borrow_mut();
            state.input(Input::Modifiers(web::modifiers_from_keyboard_event(&event)));
            let handled = state.input(Input::Key {
                key: web::key_from_event(&event),
                pressed,
                repeat: event.repeat(),
            });
            drop(state);
            if handled {
                event.prevent_default();
            }
        });
        let _ = canvas.add_event_listener_with_callback(name, handler.as_ref().unchecked_ref());
        handler.forget();
    }
}

/// Watches the canvas for a change of size — or of pixel ratio.
///
/// `device-pixel-content-box` rather than the default: zooming the page leaves
/// the CSS box untouched while the backing store has to change, and a
/// content-box observer simply never fires for it.
fn observe(board: &Rc<RefCell<Board>>, canvas: &HtmlCanvasElement) {
    let board = Rc::clone(board);
    let handler = Closure::<dyn FnMut()>::new(move || {
        board.borrow_mut().resized();
    });
    let Ok(observer) = ResizeObserver::new(handler.as_ref().unchecked_ref()) else {
        complain("seating: the chart cannot watch its canvas for resizes");
        return;
    };
    let options = ResizeObserverOptions::new();
    options.set_box(ResizeObserverBoxOptions::DevicePixelContentBox);
    observer.observe_with_options(canvas, &options);
    handler.forget();
    // The observer must outlive this call; it is never disconnected because the
    // canvas lives as long as the page does.
    std::mem::forget(observer);
}

/// Drives the frame loop.
fn animate(board: &Rc<RefCell<Board>>) {
    let board = Rc::clone(board);
    let next = Rc::new(RefCell::new(None::<Closure<dyn FnMut()>>));
    let again = Rc::clone(&next);
    *next.borrow_mut() = Some(Closure::<dyn FnMut()>::new(move || {
        board.borrow_mut().frame();
        if let Some(window) = web_sys::window()
            && let Some(callback) = again.borrow().as_ref()
        {
            let _ = window.request_animation_frame(callback.as_ref().unchecked_ref());
        }
    }));
    if let Some(window) = web_sys::window()
        && let Some(callback) = next.borrow().as_ref()
    {
        let _ = window.request_animation_frame(callback.as_ref().unchecked_ref());
    }
    std::mem::forget(next);
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
async fn load(url: &str) -> Option<web_sys::HtmlImageElement> {
    let image = web_sys::HtmlImageElement::new().ok()?;
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

fn surface(
    width: u32,
    height: u32,
) -> Option<(HtmlCanvasElement, web_sys::CanvasRenderingContext2d)> {
    let canvas: HtmlCanvasElement = web_sys::window()?
        .document()?
        .create_element("canvas")
        .ok()?
        .dyn_into()
        .ok()?;
    canvas.set_width(width.max(1));
    canvas.set_height(height.max(1));
    let ctx: web_sys::CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
    Some((canvas, ctx))
}

fn pixels(ctx: &web_sys::CanvasRenderingContext2d, width: u32, height: u32) -> Option<ImageData> {
    let data = ctx
        .get_image_data(0.0, 0.0, width as f64, height as f64)
        .ok()?;
    Some(ImageData::rgba(width, height, data.data().0))
}

fn rounded_rect(ctx: &web_sys::CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
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
