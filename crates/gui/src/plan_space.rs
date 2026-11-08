//! The plan in space, as the plan view first shows it: each part of the
//! level shown a planet in its layer's colour, larger the more lines it
//! holds, its longest files moons on a ring around it; the parts that use
//! no other gathered at the heart, the others in shells around it by their
//! layers, each outside what it uses and near it; each link an arc from the
//! part that uses in to the part used, brighter for more imports, magenta
//! when it goes through an API, red around a loop, bowing away from the
//! heart. The scene shares the universe's camera and sky, as the database's
//! does.
//!
//! A part pointed at lights what it uses and what uses it, with how much,
//! and its files' names; a click glides to it and lists its files, a double
//! click opens it onto its own parts.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, FontId, Painter, Rect};
use universe_codemap::{Architecture, CodeMap, ComponentKind, Link, NodeKind};

use crate::plan;
use crate::space::{self, Naming, OnScreen, Rig, Shells, hdr, tilt};
use crate::theme::{ACCENT, CYAN, DIM, MAGENTA, RED, TEXT};
use crate::universe::{Space, Universe, UniverseCamera};
use crate::universe_view::shadowed;

/// The most files a planet's moons show; the rest make one moon more.
const MAX_MOONS: usize = 12;

/// A planet's radius: the smallest, and the largest, the biggest part's.
const SMALLEST: f32 = 0.6;
const LARGEST: f32 = 2.2;

/// How far out from a planet's surface its moons turn, how fast, in
/// radians a second, and a moon's radius, the smallest and the longest
/// file's.
const RING: f32 = 0.9;
const SPIN: f32 = 0.25;
const MOON_SMALLEST: f32 = 0.08;
const MOON_LARGEST: f32 = 0.22;

/// The colour of the moon of the files left out, as a direction in HDR.
const GREY: [f32; 3] = [0.4, 0.44, 0.55];

/// `colour` as a direction in HDR: its linear colour, its brightest channel
/// made 1, for a glow to set how bright.
fn direction(colour: Color32) -> [f32; 3] {
    let linear = Color::srgb_u8(colour.r(), colour.g(), colour.b()).to_linear();
    let most = linear.red.max(linear.green).max(linear.blue).max(1e-3);
    [linear.red / most, linear.green / most, linear.blue / most]
}

/// How many lines file `node` of `map` has.
fn lines(map: &CodeMap, node: usize) -> usize {
    match map.nodes.get(node).map(|n| &n.kind) {
        Some(NodeKind::File { lines, .. }) => *lines,
        _ => 0,
    }
}

/// Where a moon turns: its file, as a node of the map, `None` for the one
/// of the files left out; at what angle on its ring; how large.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MoonAt {
    file: Option<usize>,
    angle: f32,
    size: f32,
}

/// The moons of `component`: none for a file, its longest files otherwise,
/// larger the longer, and one more for those left out, evenly around its
/// ring.
fn moons(component: &universe_codemap::Component, map: &CodeMap) -> Vec<MoonAt> {
    if !plan::opens(component) {
        return Vec::new();
    }
    let longest = component.files.first().map_or(1, |&f| lines(map, f)).max(1) as f32;
    let mut files: Vec<(Option<usize>, f32)> = component
        .files
        .iter()
        .take(MAX_MOONS)
        .map(|&f| {
            let share = (lines(map, f) as f32 / longest).sqrt();
            (
                Some(f),
                MOON_SMALLEST + (MOON_LARGEST - MOON_SMALLEST) * share,
            )
        })
        .collect();
    if component.files.len() > MAX_MOONS {
        files.push((None, MOON_SMALLEST));
    }
    let count = files.len().max(1) as f32;
    files
        .into_iter()
        .enumerate()
        .map(|(i, (file, size))| MoonAt {
            file,
            angle: i as f32 * TAU / count,
            size,
        })
        .collect()
}

/// Where the parts of `design` stand: each in the shell of its layer,
/// larger the more lines it holds, gathered with the parts of its kind.
fn shells(design: &Architecture, moons: &[Vec<MoonAt>]) -> Shells {
    let biggest = design
        .components
        .iter()
        .map(|c| c.lines)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let size: Vec<f32> = design
        .components
        .iter()
        .map(|c| SMALLEST + (LARGEST - SMALLEST) * (c.lines as f32 / biggest).cbrt())
        .collect();
    let reach = size
        .iter()
        .zip(moons)
        .map(|(size, moons)| {
            if moons.is_empty() {
                size + 0.3
            } else {
                size + RING + MOON_LARGEST
            }
        })
        .collect();
    let kind = design
        .components
        .iter()
        .map(|c| match c.kind {
            ComponentKind::File => 0,
            ComponentKind::Folder => 1,
            ComponentKind::Package => 2,
            ComponentKind::Group { .. } => 3,
        })
        .collect();
    Shells::new(
        design.components.iter().map(|c| c.layer).collect(),
        size,
        reach,
        kind,
        design.links.iter().map(|l| (l.from, l.to)),
    )
}

/// What colours a mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tone {
    /// A planet, in its layer's colour.
    Body(usize),
    /// A file's moon, in its part's layer's colour.
    Moon(usize),
    /// The moon of the files left out.
    Rest,
}

/// How a planet and its moons are lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Shade {
    Lit,
    /// The part pointed at.
    Picked,
    /// A part the one pointed at has no link with.
    Dim,
    /// A part the search leaves out.
    Faded,
}

/// The material of a mesh of `tone` shaded `shade`, among `layers` layers:
/// its colour on the side the light reaches, and its own glow, to bloom.
fn material(tone: Tone, shade: Shade, layers: usize) -> StandardMaterial {
    let [r, g, b] = match tone {
        Tone::Body(layer) | Tone::Moon(layer) => direction(plan::layer_colour(layer, layers)),
        Tone::Rest => GREY,
    };
    let body = matches!(tone, Tone::Body(_));
    let (base, glow) = match shade {
        Shade::Picked if body => (0.5, 1.2),
        Shade::Picked => (0.4, 1.4),
        Shade::Lit if body => (0.45, 0.4),
        Shade::Lit => (0.3, 0.5),
        Shade::Dim => (0.06, 0.05),
        Shade::Faded => (0.025, 0.015),
    };
    StandardMaterial {
        base_color: Color::linear_rgb(r * base, g * base, b * base),
        emissive: LinearRgba::rgb(r * glow, g * glow, b * glow),
        perceptual_roughness: 0.7,
        ..default()
    }
}

/// Marks what belongs to the plan's scene.
#[derive(Component)]
pub(crate) struct InPlan;

/// A planet with its ring, by its part's place in the design.
#[derive(Component)]
pub(crate) struct Planet(usize);

/// The ring of moons around planet `part`.
#[derive(Component)]
pub(crate) struct Ring(usize);

/// What a mesh shows of a part: the planet itself, or a moon, of a file
/// or, `None`, of those left out.
#[derive(Component, Clone, Copy)]
pub(crate) enum Piece {
    Body(usize),
    Moon { part: usize, file: Option<usize> },
}

impl Piece {
    fn part(self) -> usize {
        match self {
            Self::Body(part) | Self::Moon { part, .. } => part,
        }
    }
}

/// The plan in space, as the panels and the systems share it.
#[derive(Resource, Default)]
pub(crate) struct PlanSpace {
    /// The design laid out, and which of the plan's designs it is.
    design: Option<Architecture>,
    designs: u64,
    shells: Option<Shells>,
    /// Each planet's moons.
    moons: Vec<Vec<MoonAt>>,
    /// Whether the scene stands, and must stand anew.
    spawned: bool,
    respawn: bool,
    /// The materials made so far, shared by the meshes of a tone and shade.
    materials: HashMap<(Tone, Shade), Handle<StandardMaterial>>,
    /// The meshes must take their shades again.
    reshade: bool,
    /// How far the rings have turned, in seconds of turning.
    spin: f32,
    /// What the search keeps, the part pointed at or chosen, and the one
    /// chosen, for the camera.
    kept: Vec<bool>,
    focus: Option<usize>,
    chosen: Option<usize>,
    pub(crate) rig: Rig,
    /// Where each planet is on screen, the moons of the one pointed at with
    /// their files, and the middles of its links.
    on_screen: Vec<Option<OnScreen>>,
    moons_on_screen: Vec<(Option<usize>, Vec2)>,
    links_on_screen: Vec<(usize, Vec2)>,
}

impl PlanSpace {
    /// Takes `design`, the plan's `designs`th, read from `map`, when it is
    /// another than the one laid out, and lays it out anew.
    pub(crate) fn take(&mut self, design: &Architecture, map: &CodeMap, designs: u64) {
        if self.design.is_some() && self.designs == designs {
            return;
        }
        self.moons = design.components.iter().map(|c| moons(c, map)).collect();
        self.shells = Some(shells(design, &self.moons));
        self.design = Some(design.clone());
        self.designs = designs;
        // The layers' colours change with how many layers there are.
        self.materials.clear();
        self.respawn = true;
        self.kept.clear();
        self.focus = None;
        self.chosen = None;
        self.rig.reframe();
        self.on_screen.clear();
        self.moons_on_screen.clear();
        self.links_on_screen.clear();
    }

    /// The part under `at`, on the window.
    pub(crate) fn under(&self, at: egui::Pos2) -> Option<usize> {
        space::pick(&self.on_screen, Vec2::new(at.x, at.y))
    }

    /// Lights what `kept`, the part `focus` and the one `chosen` say, when
    /// they change.
    pub(crate) fn light(&mut self, kept: &[bool], focus: Option<usize>, chosen: Option<usize>) {
        if self.kept != kept || self.focus != focus {
            kept.clone_into(&mut self.kept);
            self.focus = focus;
            self.reshade = true;
        }
        if self.chosen != chosen {
            self.chosen = chosen;
            self.rig.reframe();
        }
    }

    /// How planet `t` and its moons are lit.
    fn shade(&self, t: usize) -> Shade {
        if !self.kept.get(t).copied().unwrap_or(true) {
            return Shade::Faded;
        }
        match (self.focus, &self.shells) {
            (Some(f), _) if f == t => Shade::Picked,
            (Some(f), Some(shells)) if shells.joined(f, t) => Shade::Lit,
            (Some(_), _) => Shade::Dim,
            (None, _) => Shade::Lit,
        }
    }

    /// What colours `piece`.
    fn tone(&self, piece: Piece) -> Tone {
        let layer = self
            .design
            .as_ref()
            .and_then(|d| d.components.get(piece.part()))
            .map_or(0, |c| c.layer);
        match piece {
            Piece::Body(_) => Tone::Body(layer),
            Piece::Moon { file: Some(_), .. } => Tone::Moon(layer),
            Piece::Moon { file: None, .. } => Tone::Rest,
        }
    }

    /// The material of `tone` shaded `shade`, made the first time asked.
    fn material(
        &mut self,
        tone: Tone,
        shade: Shade,
        assets: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        let layers = self.design.as_ref().map_or(1, |d| d.layers.len());
        self.materials
            .entry((tone, shade))
            .or_insert_with(|| assets.add(material(tone, shade, layers)))
            .clone()
    }

    /// How planet `t`'s ring is turned: tilted its own way, and turned
    /// about its axis from an angle of its own.
    fn ring_rotation(&self, t: usize) -> Quat {
        tilt(t) * Quat::from_rotation_y(self.spin * SPIN + t as f32 * 2.399_963)
    }

    /// Where `moon` of planet `t` is, from its centre.
    fn moon_offset(&self, shells: &Shells, t: usize, moon: &MoonAt) -> Vec3 {
        let local = Vec3::new(moon.angle.cos(), 0.0, moon.angle.sin()) * (shells.size(t) + RING);
        self.ring_rotation(t) * local
    }
}

/// Puts up the scene when shown and laid out, takes it down when left.
pub(crate) fn spawn(
    mut commands: Commands,
    mut space: ResMut<PlanSpace>,
    universe: Res<Universe>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<InPlan>>,
    mut sphere: Local<Option<Handle<Mesh>>>,
) {
    let wanted = universe.space == Some(Space::Plan) && space.shells.is_some();
    if (!wanted || space.respawn) && space.spawned {
        for entity in &existing {
            commands.entity(entity).despawn();
        }
        space.spawned = false;
    }
    if !wanted || space.spawned {
        return;
    }
    let Some(shells) = space.shells.clone() else {
        return;
    };
    space.respawn = false;
    space.spawned = true;
    space.reshade = true;
    let sphere = sphere
        .get_or_insert_with(|| meshes.add(Sphere::new(1.0).mesh().uv(32, 18)))
        .clone();
    // Lit from above and aside, for each planet to show a day and a night.
    commands.spawn((
        DirectionalLight {
            illuminance: 3_000.0,
            ..default()
        },
        Transform::from_xyz(40.0, 90.0, 25.0).looking_at(Vec3::ZERO, Vec3::Y),
        InPlan,
    ));
    let lit = space.material(Tone::Rest, Shade::Lit, &mut assets);
    for t in 0..shells.len() {
        let size = shells.size(t);
        let moons = space.moons[t].clone();
        commands
            .spawn((
                Transform::from_translation(shells.centre(t)),
                Visibility::Visible,
                Planet(t),
                InPlan,
            ))
            .with_children(|planet| {
                planet.spawn((
                    Mesh3d(sphere.clone()),
                    MeshMaterial3d(lit.clone()),
                    Transform::from_scale(Vec3::splat(size)),
                    Piece::Body(t),
                ));
                planet
                    .spawn((Transform::default(), Visibility::Inherited, Ring(t)))
                    .with_children(|ring| {
                        for moon in &moons {
                            let at =
                                Vec3::new(moon.angle.cos(), 0.0, moon.angle.sin()) * (size + RING);
                            ring.spawn((
                                Mesh3d(sphere.clone()),
                                MeshMaterial3d(lit.clone()),
                                Transform::from_translation(at).with_scale(Vec3::splat(moon.size)),
                                Piece::Moon {
                                    part: t,
                                    file: moon.file,
                                },
                            ));
                        }
                    });
            });
    }
}

/// What the camera's query reads and moves.
type CameraParts = (
    &'static mut Transform,
    &'static Camera,
    &'static GlobalTransform,
);

/// What keeps the planets, their rings and the camera apart, for one
/// system to move all three.
type PlanetsOnly = (Without<UniverseCamera>, Without<Ring>);
type RingsOnly = (Without<UniverseCamera>, Without<Planet>);
type CameraOnly = (With<UniverseCamera>, Without<Planet>, Without<Ring>);

/// The colour a link is drawn in: red around a loop, magenta through an
/// API, else `otherwise`.
fn link_colour(link: &Link, otherwise: [f32; 3]) -> [f32; 3] {
    if link.cyclic {
        direction(RED)
    } else if link.imports == 0 {
        direction(MAGENTA)
    } else {
        otherwise
    }
}

/// Each frame while shown: the planets' next steps, the rings' turns, the
/// shades, the links, the camera, and where the names go.
#[allow(clippy::too_many_arguments)]
pub(crate) fn animate(
    mut space: ResMut<PlanSpace>,
    universe: Res<Universe>,
    time: Res<Time>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut planets: Query<(&Planet, &mut Transform), PlanetsOnly>,
    mut rings: Query<(&Ring, &mut Transform), RingsOnly>,
    mut pieces: Query<(&Piece, &mut MeshMaterial3d<StandardMaterial>)>,
    mut camera: Query<CameraParts, CameraOnly>,
    mut gizmos: Gizmos,
) {
    if universe.space != Some(Space::Plan) || !space.spawned {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let t_now = time.elapsed_secs_wrapped();
    if let Some(shells) = space.shells.as_mut()
        && shells.settle()
    {
        for (planet, mut transform) in &mut planets {
            transform.translation = shells.centre(planet.0);
        }
    }
    // The rings turn while nothing is pointed at, for each moon to stay by
    // its file's name.
    if space.focus.is_none() {
        space.spin += dt;
    }
    for (ring, mut transform) in &mut rings {
        transform.rotation = space.ring_rotation(ring.0);
    }
    if std::mem::take(&mut space.reshade) {
        let count = space.shells.as_ref().map_or(0, Shells::len);
        let shades: Vec<Shade> = (0..count).map(|t| space.shade(t)).collect();
        for (piece, mut material) in &mut pieces {
            let tone = space.tone(*piece);
            let shade = shades.get(piece.part()).copied().unwrap_or(Shade::Lit);
            let handle = space.material(tone, shade, &mut assets);
            if material.0 != handle {
                material.0 = handle;
            }
        }
    }
    let (Some(shells), Some(design)) = (space.shells.clone(), space.design.clone()) else {
        return;
    };
    let layers = design.layers.len();

    // The ring of the part pointed at, the path its moons turn on.
    if let Some(f) = space
        .focus
        .filter(|f| space.moons.get(*f).is_some_and(|m| !m.is_empty()))
    {
        let around = Isometry3d::new(shells.centre(f), tilt(f) * Quat::from_rotation_x(FRAC_PI_2));
        let colour = direction(plan::layer_colour(design.components[f].layer, layers));
        gizmos
            .circle(
                around,
                shells.size(f) + RING,
                Color::from(hdr(colour, 1.0, 0.35)),
            )
            .resolution(64);
    }

    // The links: from the part that uses, bowing away from the heart, in to
    // the part used, brighter for more imports. Those of the part pointed
    // at lit, a spark running along each toward the part used; the others
    // faint.
    let most = design
        .links
        .iter()
        .map(|l| l.imports + l.calls)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let mut middles = Vec::new();
    for (i, link) in design.links.iter().enumerate() {
        let lit = |t: usize| space.kept.get(t).copied().unwrap_or(true);
        let (from, to) = (shells.centre(link.from), shells.centre(link.to));
        let a = from + (to - from).normalize_or(Vec3::Y) * shells.size(link.from);
        let b = to + (from - to).normalize_or(Vec3::Y) * shells.size(link.to);
        let points = space::arc(a, b, from, shells.size(link.from), link.from == link.to);
        let weight = ((link.imports + link.calls) as f32).ln_1p() / most.ln_1p();
        let own = direction(plan::layer_colour(
            design.components[link.from].layer,
            layers,
        ));
        let (colour, alpha, spark) = match space.focus {
            Some(f) if f == link.from => (link_colour(link, direction(ACCENT)), 0.95, true),
            Some(f) if f == link.to => (link_colour(link, direction(CYAN)), 0.95, true),
            Some(_) => (link_colour(link, own), 0.04, false),
            None if lit(link.from) && lit(link.to) => {
                let alpha = if link.cyclic {
                    0.35
                } else if link.imports == 0 {
                    0.7
                } else {
                    0.25 + 0.45 * weight
                };
                (link_colour(link, own), alpha, false)
            }
            None => (link_colour(link, own), 0.05, false),
        };
        let bright = if spark { 2.4 } else { 1.0 } * (1.0 + weight);
        gizmos.linestrip_gradient((0..=24).map(|s| {
            let t = s as f32 / 24.0;
            // Fading toward the part used, for the way a link goes to show
            // without a head.
            let colour = hdr(colour, bright, alpha).mix(&hdr(colour, bright * 0.4, alpha * 0.6), t);
            (space::bezier(points, t), Color::from(colour))
        }));
        if spark {
            let t = (t_now * 0.5 + i as f32 * 0.29).fract() * 0.94;
            gizmos.line(
                space::bezier(points, t),
                space::bezier(points, t + 0.06),
                Color::from(hdr(colour, 6.0, 1.0)),
            );
            middles.push((i, space::bezier(points, 0.5)));
        }
    }

    // The camera: drifting while nothing is pointed at, turned by a drag,
    // gliding to frame the whole, or the part chosen.
    let goal = match space.chosen.filter(|c| *c < shells.len()) {
        Some(c) => (shells.centre(c), (shells.reach(c) * 7.0).max(14.0)),
        None => {
            let (middle, reach) = shells.bounds();
            (middle, reach * 2.9)
        }
    };
    let Ok((mut transform, camera, global)) = camera.single_mut() else {
        return;
    };
    let drift = space.focus.is_none();
    let view = space
        .rig
        .aim(goal, drift, dt, &mut transform, camera, global);
    space.on_screen = (0..shells.len())
        .map(|t| shells.on_screen(t, &view))
        .collect();
    space.moons_on_screen = match space.focus.filter(|f| *f < shells.len()) {
        Some(f) => space.moons[f]
            .iter()
            .filter_map(|m| {
                let at = view.project(shells.centre(f) + space.moon_offset(&shells, f, m))?;
                Some((m.file, at))
            })
            .collect(),
        None => Vec::new(),
    };
    space.links_on_screen = middles
        .into_iter()
        .filter_map(|(i, at)| Some((i, view.project(at)?)))
        .collect();
}

/// The plan in space, drawn over its planets in `rect` under the header
/// the view draws: the parts' names, the files of the part pointed at,
/// and how much each of its links holds.
pub(crate) fn draw(
    painter: &Painter,
    rect: Rect,
    space: &PlanSpace,
    design: &Architecture,
    map: &CodeMap,
    project: &str,
    kept: &[bool],
) {
    let canvas = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 112.0),
        rect.max,
    ));
    let focus = space.focus;
    let related =
        |t: usize| focus.is_some_and(|f| space.shells.as_ref().is_some_and(|s| s.joined(f, t)));
    space::names(
        painter,
        &canvas,
        &space.on_screen,
        |t| plan::title(&design.components[t], project),
        &Naming {
            focus,
            related: &related,
            kept,
            few: design.components.len() <= 40,
        },
    );

    // The files of the part pointed at, on both sides of its ring.
    if let Some(f) = focus
        && let Some(component) = design.components.get(f)
        && let Some(planet) = space.on_screen.get(f).copied().flatten()
    {
        let size = 12.0;
        space::beside(
            &canvas,
            planet,
            &space.moons_on_screen,
            size + 3.0,
            |painter, at, leftwards, file| match file.and_then(|n| map.nodes.get(n)) {
                Some(node) => {
                    let shown = node
                        .path
                        .strip_prefix(&component.path)
                        .map_or(node.path.as_str(), |p| p.trim_start_matches('/'));
                    let shown = if shown.is_empty() { node.name() } else { shown };
                    let count = lines(map, file.unwrap_or_default()).to_string();
                    space::parts(
                        painter,
                        at,
                        leftwards,
                        &[
                            (shown, FontId::monospace(size), TEXT),
                            (&count, FontId::monospace(size * 0.9), DIM),
                        ],
                    );
                }
                None => shadowed(
                    painter,
                    at,
                    if leftwards {
                        Align2::RIGHT_CENTER
                    } else {
                        Align2::LEFT_CENTER
                    },
                    &format!(
                        "and {} more",
                        component.files.len().saturating_sub(MAX_MOONS)
                    ),
                    FontId::proportional(size),
                    DIM,
                ),
            },
        );
    }

    // How much each link of the part pointed at holds, halfway along it,
    // but where it would cover another's.
    let mut taken: Vec<Rect> = Vec::new();
    for &(l, at) in &space.links_on_screen {
        let Some(link) = design.links.get(l) else {
            continue;
        };
        let colour = if link.cyclic {
            RED
        } else if link.imports == 0 {
            MAGENTA
        } else if focus == Some(link.from) {
            ACCENT
        } else {
            CYAN
        };
        let at = egui::pos2(at.x, at.y);
        let text = plan::counts(link);
        let size = painter
            .layout_no_wrap(text.clone(), FontId::proportional(11.5), colour)
            .size();
        let area = Rect::from_center_size(at, size + egui::vec2(14.0, 6.0));
        if !canvas.clip_rect().contains(at) || taken.iter().any(|r| r.intersects(area)) {
            continue;
        }
        taken.push(area);
        plan::pill(&canvas, at, &text, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &std::path::Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_part_stands_outside_what_it_uses_and_its_longest_files_turn_around_it() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "web/main.py", "import app.api\n");
        write(
            dir.path(),
            "app/api.py",
            "import core.models\nimport core.db\n",
        );
        write(dir.path(), "core/models.py", &"x = 1\n".repeat(40));
        write(dir.path(), "core/db.py", &"y = 2\n".repeat(10));
        let map = universe_codemap::map(dir.path(), 100);
        let design = universe_codemap::architecture(&map, "");
        let mut space = PlanSpace::default();
        space.take(&design, &map, 1);
        let shells = space.shells.as_mut().unwrap();
        for _ in 0..3_000 {
            if shells.settled() {
                break;
            }
            shells.step();
        }
        let shells = space.shells.as_ref().unwrap();
        let part = |name: &str| {
            design
                .components
                .iter()
                .position(|c| c.name == name)
                .unwrap()
        };
        let from_middle = |name: &str| shells.centre(part(name)).length();
        assert!(from_middle("web") > from_middle("app"));
        assert!(from_middle("app") > from_middle("core"));
        // A folder's files are its moons, the longest the largest; a part
        // with more lines is larger.
        let moons = &space.moons[part("core")];
        assert_eq!(moons.len(), 2);
        assert!(moons[0].size > moons[1].size);
        assert!(shells.size(part("core")) > shells.size(part("web")));
        // The same design taken again is not laid out anew.
        space.respawn = false;
        space.take(&design, &map, 1);
        assert!(!space.respawn);
        space.take(&design, &map, 2);
        assert!(space.respawn);
    }

    #[test]
    fn a_link_is_red_around_a_loop_and_magenta_through_an_api() {
        let link = |imports: usize, cyclic: bool| Link {
            from: 0,
            to: 1,
            imports,
            calls: 1,
            cyclic,
        };
        let own = [0.1, 0.2, 0.3];
        assert_eq!(link_colour(&link(2, false), own), own);
        assert_eq!(link_colour(&link(0, false), own), direction(MAGENTA));
        assert_eq!(link_colour(&link(2, true), own), direction(RED));
        // A colour as a direction: its brightest channel 1.
        let [r, g, b] = direction(ACCENT);
        assert!((r.max(g).max(b) - 1.0).abs() < 1e-5);
    }
}
