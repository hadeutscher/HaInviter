//! The seating chart: who stands where on a plan of the venue.
//!
//! The arithmetic lives here, apart from the drawing, for two reasons. It is the
//! part with rules worth stating — one token per arriving person, positions kept
//! as fractions of the map rather than pixels, a tray for whoever has not been
//! placed — and keeping it free of the GPU means it compiles, and is tested, on
//! the server build as well as in the browser.
//!
//! [`board`] is the other half: a [haboard] scene that draws these tokens and
//! lets them be dragged. It exists only on `wasm32`, because that is the only
//! target this application has a screen on.
//!
//! [haboard]: https://crates.io/crates/haboard

// Only the browser has a chart to draw, so on the native build none of this is
// called. It is still compiled and still tested there, which is the reason it
// lives outside `board` in the first place.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

#[cfg(target_arch = "wasm32")]
pub mod board;

use crate::types::{GuestDto, Rsvp, SeatPlacement};

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------
//
// All in CSS pixels. Everything the board draws is scaled by the display's
// device pixel ratio at the last moment, so a token is the same size to the eye
// on a laptop and on a phone.

/// Width of one guest token.
pub const TOKEN_W: f32 = 148.0;
/// Height of one guest token.
pub const TOKEN_H: f32 = 38.0;
/// Width of the tray along the leading edge, where people wait to be placed.
pub const TRAY_W: f32 = 168.0;
/// Breathing room between the map and the edges of the drawing surface.
const MARGIN: f32 = 14.0;
/// Vertical gap between two tokens queued in the tray.
const TRAY_GAP: f32 = 6.0;
/// How far each overflow column of the tray is offset from the one before it.
const TRAY_CASCADE: f32 = 12.0;

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// An axis-aligned rectangle of the drawing surface, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The rectangle the venue map occupies inside a `width` × `height` surface.
///
/// The tray is subtracted first, then the map is fitted into what is left with
/// its aspect ratio intact and centred — a floor plan that has been photographed
/// slightly crooked should still look like the room, not like the window. With
/// no map (`map` is `None`) the whole remainder is the stage and the chart is a
/// blank sheet, which is a perfectly good way to seat a small party.
///
/// This rectangle is the frame every placement is measured against, so it is
/// also what makes a chart survive a resize: the same fractions land in the same
/// places on the plan whatever shape the window is.
pub fn stage_rect(width: f32, height: f32, map: Option<(f32, f32)>, scale: f32) -> Rect {
    let margin = MARGIN * scale;
    let tray = TRAY_W * scale;
    let free = Rect {
        x: tray,
        y: margin,
        w: (width - tray - margin).max(1.0),
        h: (height - 2.0 * margin).max(1.0),
    };
    match map {
        Some((mw, mh)) if mw > 0.0 && mh > 0.0 => {
            let zoom = (free.w / mw).min(free.h / mh);
            let (w, h) = (mw * zoom, mh * zoom);
            Rect {
                x: free.x + (free.w - w) / 2.0,
                y: free.y + (free.h - h) / 2.0,
                w,
                h,
            }
        }
        _ => free,
    }
}

/// Where the `n`-th unplaced person waits, in physical pixels.
///
/// The tray is one token wide and fills downwards. A queue longer than the
/// window is tall starts a second column, offset rather than moved clear of the
/// first: overlapping tokens are still draggable one by one, whereas a column
/// pushed onto the map would be actively in the way.
pub fn tray_slot(n: usize, height: f32, scale: f32) -> (f32, f32) {
    let margin = MARGIN * scale;
    let step = (TOKEN_H + TRAY_GAP) * scale;
    let rows = (((height - 2.0 * margin) / step).floor() as usize).max(1);
    let (column, row) = (n / rows, n % rows);
    (
        margin + column as f32 * TRAY_CASCADE * scale,
        margin + row as f32 * step,
    )
}

/// Turns a pixel position on the surface into a fraction of the venue map.
pub fn normalise(stage: Rect, x: f32, y: f32) -> (f64, f64) {
    (
        ((x - stage.x) / stage.w.max(1.0)) as f64,
        ((y - stage.y) / stage.h.max(1.0)) as f64,
    )
}

/// Turns a fraction of the venue map back into a pixel position.
pub fn place(stage: Rect, nx: f64, ny: f64) -> (f32, f32) {
    (stage.x + nx as f32 * stage.w, stage.y + ny as f32 * stage.h)
}

// ---------------------------------------------------------------------------
// People
// ---------------------------------------------------------------------------

/// Largest party one guest may occupy on the chart. The RSVP form already caps
/// a party at 50; this is the same bound restated where the tokens are made, so
/// a hand-edited database row cannot ask the browser for ten thousand sprites.
const MAX_PARTY: i64 = 50;

/// One person who said they are coming.
///
/// A guest is an invitation; an arrival is a chair. Someone who replied "three
/// of us" is one guest and three arrivals, and the chart is drawn in arrivals,
/// because that is what the room has to hold.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrival {
    pub guest_id: i64,
    /// The name on the invitation, shared by everyone in the party.
    pub name: String,
    /// Which of this guest's party this is, counting from zero.
    pub seat_index: i64,
    /// How many are coming on this invitation.
    pub party_size: i64,
}

impl Arrival {
    /// What the token says. A lone guest is just their name; one of a party also
    /// carries their place in it, so three tokens reading "The Cohens" can be
    /// told apart once they are spread across two tables.
    pub fn label(&self) -> String {
        if self.party_size <= 1 {
            self.name.clone()
        } else {
            format!(
                "{} · {}/{}",
                self.name,
                self.seat_index + 1,
                self.party_size
            )
        }
    }
}

/// Everyone the chart has to place: one [`Arrival`] per person on every
/// invitation that came back "attending".
///
/// Guests who declined or have not answered are deliberately absent. The chart
/// answers "where does everyone sit", and nobody who is not coming sits anywhere.
pub fn arrivals(guests: &[GuestDto]) -> Vec<Arrival> {
    let mut out = Vec::new();
    for guest in guests.iter().filter(|g| g.status == Rsvp::Attending) {
        let party_size = guest.party_size.clamp(1, MAX_PARTY);
        for seat_index in 0..party_size {
            out.push(Arrival {
                guest_id: guest.id,
                name: guest.name.clone(),
                seat_index,
                party_size,
            });
        }
    }
    out
}

/// Finds where an arrival was last left, if anywhere.
pub fn placement_of(seats: &[SeatPlacement], arrival: &Arrival) -> Option<(f64, f64)> {
    seats
        .iter()
        .find(|s| s.guest_id == arrival.guest_id && s.seat_index == arrival.seat_index)
        .map(|s| (s.x, s.y))
}

/// A colour for one guest's tokens, as a hue in degrees.
///
/// Everyone on the same invitation gets the same colour, so a family reads as a
/// family at a glance. Successive guest ids are pushed roughly a golden angle
/// apart rather than one degree, which is what stops two people added in the
/// same paste from being indistinguishable.
pub fn hue_for(guest_id: i64) -> f64 {
    const GOLDEN_ANGLE: i64 = 137;
    (guest_id.wrapping_mul(GOLDEN_ANGLE).rem_euclid(360)) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest(id: i64, name: &str, status: Rsvp, party_size: i64) -> GuestDto {
        GuestDto {
            id,
            name: name.to_owned(),
            token: format!("t{id}"),
            max_party_size: 10,
            status,
            party_size,
            note: String::new(),
            responded_at: String::new(),
            phone: String::new(),
            invite_sent_at: String::new(),
        }
    }

    #[test]
    fn a_party_of_three_is_three_tokens() {
        let arrivals = arrivals(&[guest(1, "The Cohens", Rsvp::Attending, 3)]);
        assert_eq!(arrivals.len(), 3);
        assert_eq!(
            arrivals.iter().map(|a| a.seat_index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(arrivals[1].label(), "The Cohens · 2/3");
    }

    #[test]
    fn only_people_who_accepted_are_drawn() {
        let list = arrivals(&[
            guest(1, "Dana", Rsvp::Attending, 2),
            guest(2, "Yuval", Rsvp::Declined, 4),
            guest(3, "Roni", Rsvp::Pending, 4),
        ]);
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|a| a.guest_id == 1));
        assert_eq!(list[0].label(), "Dana · 1/2");
    }

    #[test]
    fn an_attending_guest_always_takes_at_least_one_chair() {
        // A party size of zero is what a guest who accepted before the party
        // question existed looks like; they still have to sit somewhere.
        let list = arrivals(&[guest(1, "Dana", Rsvp::Attending, 0)]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label(), "Dana");
        assert_eq!(
            arrivals(&[guest(1, "Crowd", Rsvp::Attending, 900)]).len(),
            50
        );
    }

    #[test]
    fn the_map_keeps_its_shape_inside_the_stage() {
        // A wide map in a tall window is letterboxed, not stretched.
        let stage = stage_rect(1168.0, 800.0, Some((200.0, 100.0)), 1.0);
        assert!((stage.w / stage.h - 2.0).abs() < 0.001);
        // …and it stays clear of the tray.
        assert!(stage.x >= TRAY_W);
        assert!(stage.x + stage.w <= 1168.0);
    }

    #[test]
    fn without_a_map_the_stage_is_everything_beside_the_tray() {
        let stage = stage_rect(1000.0, 600.0, None, 1.0);
        assert_eq!(stage.x, TRAY_W);
        assert_eq!(stage.y, MARGIN);
        assert_eq!(stage.w, 1000.0 - TRAY_W - MARGIN);
    }

    #[test]
    fn a_position_survives_the_window_changing_shape() {
        let wide = stage_rect(1400.0, 700.0, Some((4.0, 3.0)), 1.0);
        let narrow = stage_rect(700.0, 1000.0, Some((4.0, 3.0)), 2.0);
        let (x, y) = place(wide, 0.25, 0.75);
        let (nx, ny) = normalise(wide, x, y);
        assert!((nx - 0.25).abs() < 1e-5 && (ny - 0.75).abs() < 1e-5);
        // The same fractions land a quarter and three quarters across whatever
        // rectangle the map ends up occupying.
        let (x, y) = place(narrow, nx, ny);
        assert!((x - (narrow.x + narrow.w * 0.25)).abs() < 1e-3);
        assert!((y - (narrow.y + narrow.h * 0.75)).abs() < 1e-3);
    }

    #[test]
    fn the_tray_fills_downwards_then_starts_another_column() {
        let height = 2.0 * MARGIN + 4.0 * (TOKEN_H + TRAY_GAP);
        let (x0, y0) = tray_slot(0, height, 1.0);
        let (x1, y1) = tray_slot(1, height, 1.0);
        assert_eq!(x0, x1);
        assert!(y1 > y0);
        // The fifth token cannot fit in a column of four, so it starts the next.
        let (x4, y4) = tray_slot(4, height, 1.0);
        assert!(x4 > x0);
        assert_eq!(y4, y0);
    }

    #[test]
    fn a_family_shares_one_colour_and_neighbours_do_not() {
        assert_eq!(hue_for(7), hue_for(7));
        assert!((hue_for(7) - hue_for(8)).abs() > 30.0);
    }
}
