//! What a request is doing, drawn as bodies of space: a planet and its
//! moon while one runs, a thin bar for its steps, and at the end of the
//! status line the cache's orbit, a planet coloured by how full the context
//! is and a comet that goes round once over the cache's life, coloured by
//! what is left of it.

use std::f32::consts::{PI, TAU};

use bevy_egui::egui::{
    Color32, CornerRadius, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, vec2,
};

use crate::bullets::{BODIES, orbit, rim, sphere, with_moon};
use crate::theme::{ACCENT, CYAN, DIM, EDGE, GREEN, RED, YELLOW, mix};

/// Seconds the moon of a running request takes to go round.
const WORKING_ORBIT: f32 = 2.4;

/// A planet and its moon, turning while a request runs. The window wakes
/// often enough then to move it.
pub(crate) fn working(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(26.0, 18.0), Sense::hover());
    let time = (ui.input(|i| i.time) % 3600.0) as f32;
    let mut shapes = Vec::new();
    with_moon(
        &mut shapes,
        rect.center(),
        5.4,
        (ACCENT, mix(ACCENT, Color32::BLACK, 0.8)),
        time / WORKING_ORBIT * TAU,
    );
    ui.painter().extend(shapes);
}

/// The steps of a request worked in steps, `number` of `of`, as a thin
/// bar `width` points long: those done lit, the current one with a glint
/// running along it, those to come dark.
pub(crate) fn steps(ui: &mut Ui, number: u8, of: u8, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 3.0), Sense::hover());
    let time = (ui.input(|i| i.time) % 3600.0) as f32;
    ui.painter().extend(step_shapes(rect, number, of, time));
}

fn step_shapes(rect: Rect, number: u8, of: u8, time: f32) -> Vec<Shape> {
    const GAP: f32 = 4.0;
    let count = f32::from(of.max(1));
    let each = (rect.width() - GAP * (count - 1.0)) / count;
    let mut out = Vec::new();
    for step in 1..=of.max(1) {
        let left = rect.left() + f32::from(step - 1) * (each + GAP);
        let piece = Rect::from_min_size(Pos2::new(left, rect.top()), vec2(each, rect.height()));
        let fill = match step.cmp(&number) {
            std::cmp::Ordering::Less => ACCENT.gamma_multiply(0.75),
            std::cmp::Ordering::Equal => ACCENT.gamma_multiply(0.35),
            std::cmp::Ordering::Greater => EDGE,
        };
        out.push(Shape::rect_filled(piece, CornerRadius::same(1), fill));
        if step == number {
            // A glint runs along the step being worked on.
            let at = (time / 1.6).fract();
            let x = piece.left() + piece.width() * at;
            let glint = Rect::from_min_max(
                Pos2::new((x - 14.0).max(piece.left()), piece.top()),
                Pos2::new(x.min(piece.right()), piece.bottom()),
            );
            out.push(Shape::rect_filled(glint, CornerRadius::same(1), ACCENT));
        }
    }
    out
}

/// The cache's orbit: a planet coloured by how full the context is, from
/// 0 to 1, and a comet that has gone round as much as the cache's life has
/// passed, `left` of `life` seconds. Either may be unknown.
pub(crate) fn cache(ui: &mut Ui, full: Option<f32>, cache: Option<(u64, u64)>) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(36.0, 18.0), Sense::hover());
    ui.painter()
        .extend(cache_shapes(rect.center(), full, cache));
    response
}

/// The colour of a context `full`, from 0 to 1: cool while there is room,
/// warm as it fills, red when nearly full.
fn context_colour(full: f32) -> Color32 {
    let (cool, _) = BODIES[0];
    if full < 0.5 {
        mix(cool, GREEN, full * 2.0)
    } else if full < 0.8 {
        mix(GREEN, YELLOW, (full - 0.5) / 0.3)
    } else {
        mix(YELLOW, RED, ((full - 0.8) / 0.2).min(1.0))
    }
}

/// The colour of a cache with `share` of its life left.
fn comet_colour(share: f32) -> Color32 {
    if share <= 0.0 {
        DIM
    } else if share < 0.2 {
        RED
    } else if share < 0.5 {
        YELLOW
    } else {
        CYAN
    }
}

fn cache_shapes(centre: Pos2, full: Option<f32>, cache: Option<(u64, u64)>) -> Vec<Shape> {
    let (rx, ry, tilt) = (15.0, 6.0, -0.12);
    let mut out = Vec::new();
    // The orbit, faint.
    let path = (0..=32)
        .map(|i| centre + orbit(rx, ry, tilt, i as f32 / 32.0 * TAU))
        .collect();
    out.push(Shape::line(path, Stroke::new(0.8, EDGE)));
    let planet = full.map_or(DIM, context_colour);
    let colours = (planet, mix(planet, Color32::BLACK, 0.82));
    let Some((left, life)) = cache.filter(|(_, life)| *life > 0) else {
        sphere(&mut out, centre, 4.6, colours);
        rim(&mut out, centre, 4.6, colours.0);
        return out;
    };
    let share = left as f32 / life as f32;
    // From the top, round once over the cache's life.
    let angle = -PI / 2.0 + (1.0 - share) * TAU;
    let colour = comet_colour(share);
    let behind = angle.sin() < 0.0;
    let comet = |out: &mut Vec<Shape>| {
        if share > 0.0 {
            // Its tail, fading behind it along the orbit.
            for step in 1..=6 {
                let back = angle - step as f32 * 0.09;
                out.push(Shape::circle_filled(
                    centre + orbit(rx, ry, tilt, back),
                    1.6 - step as f32 * 0.18,
                    colour.gamma_multiply(0.6 - step as f32 * 0.09),
                ));
            }
        }
        out.push(Shape::circle_filled(
            centre + orbit(rx, ry, tilt, angle),
            1.9,
            colour,
        ));
    };
    if behind {
        comet(&mut out);
    }
    sphere(&mut out, centre, 4.6, colours);
    rim(&mut out, centre, 4.6, colours.0);
    if !behind {
        comet(&mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_warms_as_it_fills_and_the_comet_as_the_cache_runs_out() {
        assert_eq!(context_colour(0.0), BODIES[0].0);
        assert_eq!(context_colour(0.5), GREEN);
        assert_eq!(context_colour(1.0), RED);
        assert_eq!(comet_colour(1.0), CYAN);
        assert_eq!(comet_colour(0.3), YELLOW);
        assert_eq!(comet_colour(0.1), RED);
        assert_eq!(comet_colour(0.0), DIM);
    }

    #[test]
    fn the_comet_starts_at_the_top_and_has_a_tail_only_while_the_cache_lives() {
        let centre = Pos2::new(100.0, 50.0);
        let fresh = cache_shapes(centre, Some(0.2), Some((3600, 3600)));
        let cold = cache_shapes(centre, Some(0.2), Some((0, 3600)));
        let none = cache_shapes(centre, None, None);
        // The orbit, the sphere's discs and glint, its rim: and the comet.
        assert_eq!(fresh.len(), none.len() + 7);
        assert_eq!(cold.len(), none.len() + 1);
        // Fresh, the comet's head is at the top of the orbit.
        let Some(head) = fresh.iter().find_map(|shape| match shape {
            Shape::Circle(c) if (c.radius - 1.9).abs() < 1e-6 => Some(c),
            _ => None,
        }) else {
            panic!("the comet's head: {fresh:?}");
        };
        assert!((head.center.x - centre.x).abs() < 1.0 && head.center.y < centre.y);
    }

    #[test]
    fn the_steps_done_are_lit_and_the_current_one_glints() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 3.0));
        let shapes = step_shapes(rect, 2, 4, 0.5);
        // Four pieces and a glint.
        assert_eq!(shapes.len(), 5);
    }
}
