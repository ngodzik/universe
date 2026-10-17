//! A reply's bullets drawn as small bodies of space: lit spheres that turn,
//! or planets with a moon or a ring about them. They move only while one is
//! in view: drawing one leaves a mark that the window reads to wake about
//! twelve times a second, and to sleep again once none is.

use std::f32::consts::{PI, TAU};

use bevy_egui::egui::{Color32, Id, Pos2, Sense, Shape, Stroke, Ui, Vec2, vec2};
use universe_ui::markdown::Bullets;

use crate::theme::mix;

/// Left in egui's memory when a moving bullet was drawn in view.
pub(crate) const MOVING: &str = "moving-bullets";

/// The room a bullet takes, in points: a moon's orbit fits in it.
pub(crate) const WIDTH: f32 = 18.0;

/// Seconds a sphere takes to turn once.
const TURN: f32 = 12.0;

/// Seconds a moon takes to go round.
const ORBIT: f32 = 6.0;

/// Where the light comes from: up and to the left, as a fraction of the
/// radius.
const LIGHT: Vec2 = vec2(-0.36, -0.42);

/// A body's lit side and its dark one, by depth.
const BODIES: [(Color32, Color32); 3] = [
    (
        Color32::from_rgb(132, 196, 255),
        Color32::from_rgb(16, 36, 84),
    ),
    (
        Color32::from_rgb(244, 168, 116),
        Color32::from_rgb(74, 30, 18),
    ),
    (
        Color32::from_rgb(206, 168, 244),
        Color32::from_rgb(46, 26, 74),
    ),
];

/// A moon's, and a cratered body's.
const MOON: (Color32, Color32) = (
    Color32::from_rgb(214, 214, 222),
    Color32::from_rgb(52, 54, 64),
);

/// Draws the bullet of an item `depth` deep beside a first line `line`
/// points high. Nothing is drawn out of view, and only a moving one in
/// view keeps the window awake.
pub(crate) fn bullet(ui: &mut Ui, style: Bullets, depth: usize, line: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, line), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let time = (ui.input(|i| i.time) % 3600.0) as f32;
    ui.painter()
        .extend(shapes(style, depth, rect.center(), time));
    if style.moves() {
        ui.ctx().data_mut(|d| d.insert_temp(Id::new(MOVING), true));
    }
}

/// The shapes of a bullet `depth` deep centred on `centre` at `time`, in
/// seconds. None for plain bullets, which are characters.
pub(crate) fn shapes(style: Bullets, depth: usize, centre: Pos2, time: f32) -> Vec<Shape> {
    let mut out = Vec::new();
    let colours = BODIES[depth % BODIES.len()];
    match style {
        Bullets::Plain => {}
        Bullets::Spheres => {
            let radius = [4.6, 3.9, 3.3][depth.min(2)];
            sphere(&mut out, centre, radius, colours);
            meridians(&mut out, centre, radius, colours.1, time / TURN * TAU);
            rim(&mut out, centre, radius, colours.0);
        }
        Bullets::Planets => match depth % 3 {
            // A planet and its moon, which passes behind it and before it.
            0 => {
                let radius = 4.2;
                let angle = time / ORBIT * TAU;
                let moon = centre + orbit(radius * 1.7, radius * 0.6, -0.3, angle);
                let behind = angle.sin() < 0.0;
                if behind {
                    sphere(&mut out, moon, 1.3, MOON);
                }
                sphere(&mut out, centre, radius, colours);
                rim(&mut out, centre, radius, colours.0);
                if !behind {
                    sphere(&mut out, moon, 1.5, MOON);
                }
            }
            // A ringed planet: the ring's far half behind it.
            1 => {
                let radius = 3.6;
                let ring = |from: f32, to: f32| {
                    let points = (0..=16)
                        .map(|i| {
                            let angle = from + (to - from) * i as f32 / 16.0;
                            centre + orbit(radius * 2.1, radius * 0.55, -0.35, angle)
                        })
                        .collect();
                    Shape::line(points, Stroke::new(1.1, colours.0.gamma_multiply(0.75)))
                };
                out.push(ring(PI, TAU));
                sphere(&mut out, centre, radius, colours);
                out.push(ring(0.0, PI));
            }
            // A small moon, its craters turned to the light.
            _ => {
                let radius = 3.0;
                sphere(&mut out, centre, radius, MOON);
                for (offset, size) in [(vec2(0.9, 0.7), 0.7), (vec2(-0.5, 1.4), 0.5)] {
                    out.push(Shape::circle_filled(
                        centre + offset,
                        size,
                        MOON.1.gamma_multiply(0.8),
                    ));
                }
            }
        },
    }
    out
}

/// A sphere lit from [`LIGHT`]: discs ever smaller and brighter toward the
/// light, and a glint.
fn sphere(out: &mut Vec<Shape>, centre: Pos2, radius: f32, (lit, dark): (Color32, Color32)) {
    const STEPS: usize = 6;
    for i in 0..STEPS {
        let t = i as f32 / (STEPS - 1) as f32;
        out.push(Shape::circle_filled(
            centre + LIGHT * radius * t,
            radius * (1.0 - 0.74 * t),
            mix(dark, lit, t.powf(0.7)),
        ));
    }
    out.push(Shape::circle_filled(
        centre + LIGHT * radius * 1.1,
        (radius * 0.2).max(0.5),
        Color32::from_white_alpha(140),
    ));
}

/// Four meridians a quarter turn apart, `phase` round: those on the near
/// side, fainter toward the edge, show the sphere turning.
fn meridians(out: &mut Vec<Shape>, centre: Pos2, radius: f32, colour: Color32, phase: f32) {
    for k in 0..4 {
        let longitude = phase + k as f32 * PI / 2.0;
        let near = longitude.cos();
        if near <= 0.05 {
            continue;
        }
        let points = (0..=10)
            .map(|i| {
                let latitude = -PI / 2.0 + PI * i as f32 / 10.0;
                centre
                    + vec2(
                        radius * longitude.sin() * latitude.cos(),
                        radius * latitude.sin(),
                    ) * 0.96
            })
            .collect();
        out.push(Shape::line(
            points,
            Stroke::new(0.7, colour.gamma_multiply(0.55 * near)),
        ));
    }
}

/// A thin glow about a body, its air.
fn rim(out: &mut Vec<Shape>, centre: Pos2, radius: f32, colour: Color32) {
    out.push(Shape::circle_stroke(
        centre,
        radius + 0.5,
        Stroke::new(0.8, colour.gamma_multiply(0.35)),
    ));
}

/// Where on a tilted ellipse `angle` is, from its centre: `rx` across,
/// `ry` deep, its near side down.
fn orbit(rx: f32, ry: f32, tilt: f32, angle: f32) -> Vec2 {
    let (x, y) = (rx * angle.cos(), ry * angle.sin());
    let (sin, cos) = tilt.sin_cos();
    vec2(x * cos - y * sin, x * sin + y * cos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_egui::egui::Rect;

    fn bounds(shapes: &[Shape]) -> Rect {
        shapes
            .iter()
            .map(Shape::visual_bounding_rect)
            .fold(Rect::NOTHING, |a, b| a.union(b))
    }

    #[test]
    fn plain_bullets_are_characters_and_draw_nothing() {
        assert!(shapes(Bullets::Plain, 0, Pos2::ZERO, 1.0).is_empty());
    }

    #[test]
    fn spheres_turn_and_moons_go_round_but_a_small_moon_rests() {
        let at = |style, depth, time| format!("{:?}", shapes(style, depth, Pos2::ZERO, time));
        assert_ne!(at(Bullets::Spheres, 0, 0.0), at(Bullets::Spheres, 0, 1.0));
        assert_ne!(at(Bullets::Planets, 0, 0.0), at(Bullets::Planets, 0, 1.0));
        assert_eq!(at(Bullets::Planets, 2, 0.0), at(Bullets::Planets, 2, 1.0));
    }

    #[test]
    fn every_bullet_keeps_to_its_room_at_any_time() {
        let room = Rect::from_center_size(Pos2::ZERO, vec2(WIDTH, WIDTH));
        for style in [Bullets::Spheres, Bullets::Planets] {
            for depth in 0..4 {
                for step in 0..24 {
                    let drawn = bounds(&shapes(style, depth, Pos2::ZERO, step as f32 * 0.5));
                    assert!(room.contains_rect(drawn), "{style:?} {depth}: {drawn:?}");
                }
            }
        }
    }
}
