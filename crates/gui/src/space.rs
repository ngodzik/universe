//! What the views drawn in space share: bodies laid out in shells around a
//! heart, by their layers, near those they are joined to and their group's
//! others; the camera that drifts and turns around them; the body under the
//! pointer; the bodies' names, and names spread on both sides of one.

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, FontId, Painter, Rect, Stroke};

use crate::theme::{DIM, TEXT};
use crate::universe_view::shadowed;

/// The room kept between two bodies' reach, how much more room the heart
/// and a shell take than their bodies would packed tight, how far a shell
/// is at least beyond the one inside, and how far within it a body may
/// stand nearer or farther.
const ROOM: f32 = 1.2;
const PACK: f32 = 1.7;
const STEP: f32 = 3.0;
const SLACK: f32 = 1.0;

/// How the bodies find their places: how hard bodies push each other away,
/// a join pulls its bodies together, a group gathers its bodies, and a
/// shell holds its bodies to its middle.
const PUSH: f32 = 0.35;
const JOIN: f32 = 0.06;
const KIN: f32 = 0.025;
const HOLD: f32 = 0.02;

/// How far a body may move in a step at first, how much less each step,
/// and below what it has settled.
const HEAT: f32 = 1.5;
const COOLING: f32 = 0.985;
const SETTLED: f32 = 0.01;

/// The pairs of bodies a frame's steps may weigh: fewer steps a frame for a
/// larger scene, for the window to stay smooth while it settles.
const PAIRS_PER_FRAME: usize = 240_000;

/// The camera's nearest, and the farthest it frames the whole from.
const NEAREST: f32 = 4.0;
const FARTHEST: f32 = 420.0;

/// The `i`th of `n` directions spread evenly over a sphere.
pub(crate) fn fibonacci(i: usize, n: usize) -> Vec3 {
    let y = 1.0 - 2.0 * (i as f32 + 0.5) / n.max(1) as f32;
    let across = (1.0 - y * y).max(0.0).sqrt();
    let angle = i as f32 * 2.399_963;
    Vec3::new(across * angle.cos(), y, across * angle.sin())
}

/// `[r, g, b]` made `brightness` times brighter, with an alpha of its own.
pub(crate) fn hdr([r, g, b]: [f32; 3], brightness: f32, alpha: f32) -> LinearRgba {
    LinearRgba::new(r * brightness, g * brightness, b * brightness, alpha)
}

/// The point at `t`, from 0 to 1, of the cubic Bézier curve of `points`.
pub(crate) fn bezier(points: [Vec3; 4], t: f32) -> Vec3 {
    let u = 1.0 - t;
    points[0] * (u * u * u)
        + points[1] * (3.0 * u * u * t)
        + points[2] * (3.0 * u * t * t)
        + points[3] * (t * t * t)
}

/// The curve of a link from `a` to `b`, bowing away from the heart, or, for
/// a body joined to itself at `centre`, looping out of it.
pub(crate) fn arc(a: Vec3, b: Vec3, centre: Vec3, size: f32, itself: bool) -> [Vec3; 4] {
    if itself {
        let out = centre.normalize_or(Vec3::Y) * (size + 2.0);
        [a, a + out, b + out, b]
    } else {
        let out = ((a + b) * 0.5).normalize_or(Vec3::Y) * (a.distance(b) * 0.25).max(1.0);
        let way = (b - a) * 0.2;
        [a, a + out + way, b + out - way, b]
    }
}

/// How body `t`'s rings are tilted: each about an axis of its own, for no
/// two neighbours to show theirs the same way.
pub(crate) fn tilt(t: usize) -> Quat {
    Quat::from_rotation_y(t as f32 * 1.1)
        * Quat::from_rotation_x(0.3 + 0.25 * ((t * 7) % 5) as f32 / 4.0)
}

/// Where the bodies are: each in the shell of its layer around the heart,
/// those of layer 0 in the heart itself, and in it near the bodies it is
/// joined to and its group's others.
#[derive(Debug, Clone)]
pub(crate) struct Shells {
    /// Each body's layer.
    layer: Vec<usize>,
    /// Where each is.
    spots: Vec<Vec3>,
    /// Each one's radius, and how far out what turns around it reaches.
    size: Vec<f32>,
    reach: Vec<f32>,
    /// Each one's group, and how many groups.
    group: Vec<usize>,
    groups: usize,
    /// The pairs of bodies joined, once each, the lower first, a body never
    /// with itself.
    joins: Vec<(usize, usize)>,
    /// How near the middle and how far from it the bodies of each layer
    /// stand: those of the heart from the middle itself, those of each
    /// shell beyond the one inside.
    bands: Vec<(f32, f32)>,
    /// How far a body may move in the next step.
    heat: f32,
}

impl Shells {
    /// The bodies of each `layer`, `size` and `reach` and `group`, joined by
    /// `joins`, in their first places: each group's toward a side of its
    /// own, in the shell of its layer.
    pub(crate) fn new(
        layer: Vec<usize>,
        size: Vec<f32>,
        reach: Vec<f32>,
        group: Vec<usize>,
        joins: impl IntoIterator<Item = (usize, usize)>,
    ) -> Self {
        let count = layer.len();
        let mut pairs: Vec<(usize, usize)> = joins
            .into_iter()
            .filter(|(a, b)| a != b && *a < count && *b < count)
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        // The heart a ball as large as its bodies need, each shell a sphere
        // as large as its bodies need, a step beyond the one inside.
        let layers = layer.iter().copied().max().map_or(0, |l| l + 1);
        let mut bands: Vec<(f32, f32)> = Vec::with_capacity(layers);
        for l in 0..layers {
            let cells: Vec<f32> = (0..count)
                .filter(|t| layer[*t] == l)
                .map(|t| 2.0 * reach[t] + ROOM)
                .collect();
            let band = match bands.last() {
                None => {
                    let volume: f32 = cells.iter().map(|c| c.powi(3)).sum();
                    (
                        0.0,
                        (volume * PACK * 3.0 / (4.0 * std::f32::consts::PI)).cbrt(),
                    )
                }
                Some((_, inside)) => {
                    let area: f32 = cells.iter().map(|c| c * c).sum();
                    let radius = (area * PACK / (4.0 * std::f32::consts::PI))
                        .sqrt()
                        .max(inside + STEP);
                    (radius, radius + SLACK)
                }
            };
            bands.push(band);
        }
        let groups = group.iter().copied().max().map_or(1, |g| g + 1);
        let spots = (0..count)
            .map(|t| {
                let (nearest, farthest) = bands[layer[t]];
                // Toward its group's side, each a little aside, for no two
                // to start on one spot.
                let way = (fibonacci(group[t], groups) + fibonacci(t, count) * 0.35)
                    .normalize_or(Vec3::Y);
                let radius = if layer[t] == 0 {
                    farthest * 0.5
                } else {
                    (nearest + farthest) * 0.5
                };
                way * radius
            })
            .collect();
        Self {
            layer,
            spots,
            size,
            reach,
            group,
            groups,
            joins: pairs,
            bands,
            heat: HEAT,
        }
    }

    /// How many bodies.
    pub(crate) fn len(&self) -> usize {
        self.spots.len()
    }

    /// Whether the bodies have found their places.
    pub(crate) fn settled(&self) -> bool {
        self.heat < SETTLED
    }

    /// The steps of a frame while the bodies have not found their places:
    /// as many as a frame has room for. Returns whether they moved.
    pub(crate) fn settle(&mut self) -> bool {
        if self.settled() {
            return false;
        }
        let pairs = (self.len() * self.len() / 2).max(1);
        for _ in 0..(PAIRS_PER_FRAME / pairs).clamp(1, 4) {
            self.step();
        }
        true
    }

    /// Moves each body a step toward its place: away from the others, the
    /// more so in its own shell and the next, where they would stand in
    /// each other; toward those it is joined to, the rest of its group and
    /// the middle of its shell; never out of it.
    pub(crate) fn step(&mut self) {
        let count = self.len();
        let mut push = vec![Vec3::ZERO; count];
        for a in 0..count {
            for b in a + 1..count {
                let apart = self.spots[a] - self.spots[b];
                let distance = apart.length();
                // Two bodies on one spot part along a way of their own.
                let direction = if distance < 1e-3 {
                    fibonacci((a * 31 + b) % 97, 97)
                } else {
                    apart / distance
                };
                let distance = distance.max(0.05);
                // Shells near each other leave too little room between them
                // for a body not to stand in its neighbour's.
                let weight = match self.layer[a].abs_diff(self.layer[b]) {
                    0 => 1.0,
                    1 => 0.5,
                    _ => 0.1,
                };
                let room = self.reach[a] + self.reach[b] + ROOM;
                let mut force = weight * PUSH * room * room / (distance * distance);
                if distance < room {
                    force += (room - distance) * 0.5;
                }
                push[a] += direction * force;
                push[b] -= direction * force;
            }
        }
        for &(a, b) in &self.joins {
            let pull = (self.spots[b] - self.spots[a]) * JOIN;
            push[a] += pull;
            push[b] -= pull;
        }
        let mut middles = vec![(Vec3::ZERO, 0_usize); self.groups];
        for (spot, &group) in self.spots.iter().zip(&self.group) {
            middles[group].0 += *spot;
            middles[group].1 += 1;
        }
        for (t, push) in push.iter_mut().enumerate() {
            let (sum, members) = middles[self.group[t]];
            let middle = sum / members.max(1) as f32;
            let (nearest, farthest) = self.bands[self.layer[t]];
            let held = if self.layer[t] == 0 {
                0.0
            } else {
                (nearest + farthest) * 0.5
            };
            let spot = self.spots[t];
            let out = spot.normalize_or(Vec3::Y);
            *push += (middle - spot) * KIN + out * (held - spot.length()) * HOLD;
        }
        for (t, push) in push.into_iter().enumerate() {
            let spot = self.spots[t] + push.clamp_length_max(self.heat);
            let (nearest, farthest) = self.bands[self.layer[t]];
            let out = spot.normalize_or(fibonacci(t, count));
            self.spots[t] = out * spot.length().clamp(nearest, farthest);
        }
        self.heat *= COOLING;
    }

    /// The centre of body `t`.
    pub(crate) fn centre(&self, t: usize) -> Vec3 {
        self.spots[t]
    }

    /// Body `t`'s radius.
    pub(crate) fn size(&self, t: usize) -> f32 {
        self.size[t]
    }

    /// How far out what turns around body `t` reaches.
    pub(crate) fn reach(&self, t: usize) -> f32 {
        self.reach[t]
    }

    /// Body `t`'s group.
    pub(crate) fn group(&self, t: usize) -> usize {
        self.group[t]
    }

    /// Whether bodies `a` and `b` are joined.
    pub(crate) fn joined(&self, a: usize, b: usize) -> bool {
        self.joins.binary_search(&(a.min(b), a.max(b))).is_ok()
    }

    /// The middle of the box the bodies fill, and how far from it the
    /// farthest one's reach goes.
    pub(crate) fn bounds(&self) -> (Vec3, f32) {
        if self.spots.is_empty() {
            return (Vec3::ZERO, 0.0);
        }
        let low = self.spots.iter().copied().fold(Vec3::MAX, Vec3::min);
        let high = self.spots.iter().copied().fold(Vec3::MIN, Vec3::max);
        let middle = (low + high) * 0.5;
        let reach = (0..self.len())
            .map(|t| self.spots[t].distance(middle) + self.reach[t])
            .fold(0.0_f32, f32::max);
        (middle, reach)
    }

    /// Where body `t` is on screen, as `view` projects it.
    pub(crate) fn on_screen(&self, t: usize, view: &View) -> Option<OnScreen> {
        let centre = self.spots[t];
        let middle = view.project(centre)?;
        Some(OnScreen {
            centre: middle,
            radius: middle.distance(view.project(centre + view.right * self.size[t])?),
            reach: middle.distance(view.project(centre + view.right * self.reach[t])?),
            label: view.project(centre + view.up * (self.size[t] + 0.35))?,
            depth: view.eye.distance(centre),
        })
    }
}

/// The camera of a scene in space: around which point, from which angles,
/// how far; what the panels report of the mouse, and how much of the window
/// they cover.
#[derive(Debug, Clone)]
pub(crate) struct Rig {
    centre: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// How far the camera is heading, and whether the wheel set it since
    /// the scene last framed.
    target_distance: f32,
    zoomed: bool,
    /// What the panels report of the mouse, for the next frame.
    pub(crate) drag: Vec2,
    pub(crate) zoom: f32,
    /// How much of the window's width the panels cover on the right, and
    /// the details on the left.
    pub(crate) covered: f32,
    pub(crate) left: f32,
}

impl Default for Rig {
    fn default() -> Self {
        Self {
            centre: Vec3::ZERO,
            yaw: 0.7,
            pitch: 0.35,
            distance: 260.0,
            target_distance: 260.0,
            zoomed: false,
            drag: Vec2::ZERO,
            zoom: 0.0,
            covered: 0.0,
            left: 0.0,
        }
    }
}

/// How the camera sees the scene this frame: where it is, its right and its
/// up, and how a point projects onto the window.
pub(crate) struct View<'a> {
    pub(crate) eye: Vec3,
    pub(crate) right: Vec3,
    pub(crate) up: Vec3,
    camera: &'a Camera,
    global: &'a GlobalTransform,
}

impl View<'_> {
    /// Where `point` is on the window, in logical pixels.
    pub(crate) fn project(&self, point: Vec3) -> Option<Vec2> {
        self.camera.world_to_viewport(self.global, point).ok()
    }
}

impl Rig {
    /// Frames the goal anew, forgetting how far the wheel took the camera.
    pub(crate) fn reframe(&mut self) {
        self.zoomed = false;
    }

    /// The mouse over the scene's `response`: a drag turns, the wheel comes
    /// closer.
    pub(crate) fn follow(&mut self, ui: &egui::Ui, response: &egui::Response) {
        if response.dragged() {
            let delta = response.drag_delta();
            self.drag += Vec2::new(delta.x, delta.y);
        }
        if response.hovered() {
            self.zoom += ui.input(|i| i.smooth_scroll_delta.y);
        }
    }

    /// Moves the camera a frame of `dt` toward `goal`, a point and how far
    /// to see it from: drifting round it while `drift`, turned by a drag,
    /// and moved sideways to stand in the middle of what the panels leave
    /// free.
    pub(crate) fn aim<'a>(
        &mut self,
        goal: (Vec3, f32),
        drift: bool,
        dt: f32,
        transform: &mut Transform,
        camera: &'a Camera,
        global: &'a GlobalTransform,
    ) -> View<'a> {
        let drag = std::mem::take(&mut self.drag);
        let zoom = std::mem::take(&mut self.zoom);
        if drag == Vec2::ZERO && drift {
            self.yaw += 0.04 * dt;
        }
        self.yaw -= drag.x * 0.006;
        self.pitch = (self.pitch + drag.y * 0.006).clamp(-1.3, 1.45);
        if zoom != 0.0 {
            self.zoomed = true;
            self.target_distance *= (-zoom * 0.0015).clamp(-1.5, 1.5).exp();
        } else if !self.zoomed {
            self.target_distance = goal.1.clamp(12.0, FARTHEST);
        }
        self.target_distance = self.target_distance.clamp(NEAREST, FARTHEST * 1.4);
        let ease = 1.0 - (-3.0 * dt).exp();
        self.centre = self.centre.lerp(goal.0, ease);
        self.distance += (self.target_distance - self.distance) * ease;
        let offset = Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        ) * self.distance;
        let looking =
            Transform::from_translation(self.centre + offset).looking_at(self.centre, Vec3::Y);
        let aspect = camera
            .logical_viewport_size()
            .map_or(1.6, |size| size.x / size.y.max(1.0));
        let half_width = self.distance * std::f32::consts::FRAC_PI_8.tan() * aspect;
        let shift = looking.right() * ((self.covered - self.left) * half_width);
        *transform = looking.with_translation(looking.translation + shift);
        View {
            eye: transform.translation,
            right: *looking.right(),
            up: *looking.up(),
            camera,
            global,
        }
    }
}

/// Where a body is on screen, in logical pixels: its centre, its radius and
/// how far what turns around it reaches there, where its name goes, and how
/// far it is from the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OnScreen {
    pub(crate) centre: Vec2,
    pub(crate) radius: f32,
    pub(crate) reach: f32,
    pub(crate) label: Vec2,
    pub(crate) depth: f32,
}

/// The body under `at`: of those it falls on, the nearest the camera; else,
/// of those whose reach it falls in, the one it is the nearest the middle
/// of.
pub(crate) fn pick(on_screen: &[Option<OnScreen>], at: Vec2) -> Option<usize> {
    let placed = || {
        on_screen
            .iter()
            .enumerate()
            .filter_map(|(t, s)| Some((t, s.as_ref()?)))
    };
    placed()
        .filter(|(_, s)| s.centre.distance(at) <= s.radius.max(6.0) + 3.0)
        .min_by(|a, b| a.1.depth.total_cmp(&b.1.depth))
        .or_else(|| {
            placed()
                .map(|(t, s)| (t, s, s.centre.distance(at) / s.reach.max(1.0)))
                .filter(|(_, _, near)| *near <= 1.0)
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(t, s, _)| (t, s))
        })
        .map(|(t, _)| t)
}

/// Heights for labels wanted at `ys`, from the highest: in the same order,
/// each at least `gap` below the one before, and the whole moved back for
/// its middle to stay where it was wanted.
pub(crate) fn spread(ys: &[f32], gap: f32) -> Vec<f32> {
    let mut spread: Vec<f32> = Vec::with_capacity(ys.len());
    for &y in ys {
        let y = spread.last().map_or(y, |above| y.max(above + gap));
        spread.push(y);
    }
    let back = (ys.iter().sum::<f32>() - spread.iter().sum::<f32>()) / ys.len().max(1) as f32;
    spread.iter().map(|y| y + back).collect()
}

/// What decides how a body's name is drawn: whether it is the one pointed
/// at, joined to it, kept by the search.
pub(crate) struct Naming<'a> {
    pub(crate) focus: Option<usize>,
    pub(crate) related: &'a dyn Fn(usize) -> bool,
    pub(crate) kept: &'a [bool],
    /// Few enough bodies for every name to be tried.
    pub(crate) few: bool,
}

/// The bodies' names, above them: that of the body pointed at and those
/// joined to it first, then the nearest; a name is left out where it would
/// cover another, or its body is too small to read. Returns where the
/// names went.
pub(crate) fn names<'a>(
    painter: &Painter,
    canvas: &Painter,
    on_screen: &[Option<OnScreen>],
    name: impl Fn(usize) -> &'a str,
    naming: &Naming,
) -> Vec<Rect> {
    let Naming {
        focus,
        related,
        kept,
        few,
    } = *naming;
    let pos = |v: Vec2| egui::pos2(v.x, v.y);
    let mut order: Vec<(usize, OnScreen)> = on_screen
        .iter()
        .enumerate()
        .filter_map(|(t, s)| Some((t, (*s)?)))
        .collect();
    order.sort_by(|(a, sa), (b, sb)| {
        (focus != Some(*a), !related(*a))
            .cmp(&(focus != Some(*b), !related(*b)))
            .then(sa.depth.total_cmp(&sb.depth))
    });
    let mut taken: Vec<Rect> = Vec::new();
    for (t, s) in order {
        let picked = focus == Some(t);
        let large = s.radius > 4.0;
        let lit = kept.get(t).copied().unwrap_or(true);
        if !(picked || related(t) || ((large || few) && lit && focus.is_none())) {
            continue;
        }
        let font = FontId::proportional(if picked { 16.0 } else { 12.5 });
        let colour = if picked {
            TEXT
        } else if related(t) || focus.is_none() {
            TEXT.gamma_multiply(0.8)
        } else {
            DIM
        };
        let text = name(t);
        let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), colour);
        let area = Rect::from_center_size(
            pos(s.label) - egui::vec2(0.0, galley.size().y / 2.0),
            galley.size() + egui::vec2(8.0, 2.0),
        );
        if !picked && taken.iter().any(|r| r.intersects(area)) {
            continue;
        }
        taken.push(area);
        shadowed(
            canvas,
            pos(s.label),
            Align2::CENTER_BOTTOM,
            text,
            font,
            colour,
        );
    }
    taken
}

/// Names of what turns around `body`, once it is large enough to tell them
/// apart: those on its right in a column right of it, the others left of
/// it, each tied by a line to where it is, at `at`; each drawn by `label`
/// from where its column starts, the left column's ending there.
pub(crate) fn beside<T: Copy>(
    canvas: &Painter,
    body: OnScreen,
    items: &[(T, Vec2)],
    gap: f32,
    mut label: impl FnMut(&Painter, egui::Pos2, bool, T),
) {
    if body.reach < 24.0 {
        return;
    }
    let pos = |v: Vec2| egui::pos2(v.x, v.y);
    for leftwards in [false, true] {
        let mut side: Vec<(T, Vec2)> = items
            .iter()
            .copied()
            .filter(|(_, at)| (at.x < body.centre.x) == leftwards)
            .collect();
        side.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));
        let ys: Vec<f32> = side.iter().map(|(_, at)| at.y).collect();
        let edge = body.reach + 28.0;
        let x = if leftwards {
            body.centre.x - edge
        } else {
            body.centre.x + edge
        };
        for ((item, at), y) in side.into_iter().zip(spread(&ys, gap)) {
            let start = egui::pos2(x, y);
            let end = start + egui::vec2(if leftwards { 4.0 } else { -4.0 }, 0.0);
            canvas.line_segment([pos(at), end], Stroke::new(1.0, DIM.gamma_multiply(0.6)));
            label(canvas, start, leftwards, item);
        }
    }
}

/// A label made of `parts`, each in its font and colour, from `at`
/// rightwards, or, `leftwards`, up to it.
pub(crate) fn parts(
    painter: &Painter,
    at: egui::Pos2,
    leftwards: bool,
    parts: &[(&str, FontId, egui::Color32)],
) {
    let widths: Vec<f32> = parts
        .iter()
        .map(|(text, font, colour)| {
            painter
                .layout_no_wrap((*text).to_owned(), font.clone(), *colour)
                .size()
                .x
        })
        .collect();
    let width = widths.iter().sum::<f32>() + 5.0 * parts.len().saturating_sub(1) as f32;
    let mut x = if leftwards { at.x - width } else { at.x };
    for ((text, font, colour), width) in parts.iter().zip(widths) {
        shadowed(
            painter,
            egui::pos2(x, at.y),
            Align2::LEFT_CENTER,
            text,
            font.clone(),
            *colour,
        );
        x += width + 5.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled(mut shells: Shells) -> Shells {
        for _ in 0..3_000 {
            if shells.settled() {
                break;
            }
            shells.step();
        }
        assert!(shells.settled());
        shells
    }

    #[test]
    fn each_layer_stands_outside_the_one_below_and_no_body_in_another() {
        // Two at the heart, three around them, one beyond, two groups.
        let layer = vec![0, 0, 1, 1, 1, 2];
        let size = vec![1.0, 0.6, 0.6, 0.6, 0.6, 0.6];
        let reach = vec![2.0, 1.5, 1.5, 1.5, 1.5, 1.5];
        let group = vec![0, 1, 0, 1, 1, 0];
        let joins = [(2, 0), (3, 1), (4, 1), (5, 2), (5, 5), (0, 2)];
        let shells = settled(Shells::new(layer.clone(), size, reach, group, joins));
        let from_middle = |t: usize| shells.centre(t).length();
        for (a, b) in [(2, 0), (3, 1), (4, 1), (5, 2)] {
            assert!(from_middle(a) > from_middle(b), "{a} outside {b}");
        }
        for a in 0..shells.len() {
            for b in a + 1..shells.len() {
                let apart = shells.centre(a).distance(shells.centre(b));
                assert!(apart >= shells.reach(a) + shells.reach(b), "{a} in {b}");
            }
        }
        // A join given twice, or a body joined to itself, is one join or
        // none.
        assert!(shells.joined(0, 2) && shells.joined(2, 0));
        assert!(!shells.joined(5, 5));
        // Off a single plane.
        assert!((0..shells.len()).any(|t| shells.centre(t).y.abs() > 0.5));
        // The box holds every body's reach.
        let (middle, reach) = shells.bounds();
        assert!(
            (0..shells.len())
                .all(|t| { shells.centre(t).distance(middle) + shells.reach(t) <= reach + 1e-3 })
        );
    }

    #[test]
    fn the_body_pointed_at_is_the_nearest_under_the_pointer_or_whose_reach_it_is_in() {
        let body = |x: f32, depth: f32| {
            Some(OnScreen {
                centre: Vec2::new(x, 100.0),
                radius: 20.0,
                reach: 50.0,
                label: Vec2::new(x, 75.0),
                depth,
            })
        };
        let on_screen = [body(100.0, 50.0), body(110.0, 30.0), body(400.0, 10.0)];
        // On two bodies: the nearer the camera.
        assert_eq!(pick(&on_screen, Vec2::new(105.0, 100.0)), Some(1));
        assert_eq!(pick(&on_screen, Vec2::new(85.0, 100.0)), Some(0));
        // In a body's reach only: that body.
        assert_eq!(pick(&on_screen, Vec2::new(360.0, 100.0)), Some(2));
        assert_eq!(pick(&on_screen, Vec2::new(250.0, 100.0)), None);
    }

    #[test]
    fn labels_keep_their_order_and_room_around_where_they_were_wanted() {
        let wanted = [100.0, 102.0, 104.0, 160.0];
        let placed = spread(&wanted, 15.0);
        assert!(placed.windows(2).all(|w| w[1] - w[0] >= 15.0 - 1e-4));
        let middle = |ys: &[f32]| ys.iter().sum::<f32>() / ys.len() as f32;
        assert!((middle(&placed) - middle(&wanted)).abs() < 1e-3);
        // Far enough apart, labels stay where they were wanted.
        assert_eq!(spread(&[10.0, 40.0], 15.0), vec![10.0, 40.0]);
    }
}
