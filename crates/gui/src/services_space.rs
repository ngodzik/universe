//! The services in space, as the services view first shows them: each
//! service a star, coloured by what it is (blue an API, violet MCP tools,
//! orange an app, cyan the cloud, grey outside the project), larger the
//! more it serves and the more it is linked; the routes or tools it serves
//! moons around it; those that reach no other gathered at the heart, the
//! others in shells around it, each outside what it reaches; each link an
//! arc in the colour of its way, comets running along it from the service
//! that reaches to the one reached. The scene shares the universe's camera
//! and sky, as the plan's and the database's do.
//!
//! A service pointed at lights its links and names what it serves; a click
//! glides to it and shows it whole.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, FontId, Painter, Rect};
use universe_codemap::{CodeMap, Service, Services, ToolKind, Via};

use crate::api_view;
use crate::space::{self, Naming, OnScreen, Rig, Shells, hdr, tilt};
use crate::theme::{ACCENT, CYAN, DIM, LINK, MAGENTA, TEXT, YELLOW};
use crate::universe::{Space, Universe, UniverseCamera};
use crate::universe_view::shadowed;

/// The most routes or tools a star's moons show; the rest make one moon
/// more.
const MAX_MOONS: usize = 12;

/// A star's radius: the smallest, how much larger what it serves and its
/// links make it, and the largest.
const SMALLEST: f32 = 0.6;
const LARGER: f32 = 0.3;
const LARGEST: f32 = 2.0;

/// How far out from a star's surface its moons turn, how fast, in radians
/// a second, and a moon's radius.
const RING: f32 = 0.8;
const SPIN: f32 = 0.25;
const MOON: f32 = 0.12;

/// The colour of what is outside the project, of an image used as it is,
/// and of the moon of what is left out, as directions in HDR.
const GREY: [f32; 3] = [0.55, 0.6, 0.7];
const PALE: [f32; 3] = [0.8, 0.86, 1.0];

/// How fast the comets run along a link, in lengths a second, and the most
/// a link carries.
const COMET_SPEED: f32 = 0.22;
const COMETS: usize = 4;

/// `colour` as a direction in HDR: its linear colour, its brightest channel
/// made 1, for a glow to set how bright.
fn direction(colour: Color32) -> [f32; 3] {
    let linear = Color::srgb_u8(colour.r(), colour.g(), colour.b()).to_linear();
    let most = linear.red.max(linear.green).max(linear.blue).max(1e-3);
    [linear.red / most, linear.green / most, linear.blue / most]
}

/// A service's colour: what it is.
fn colour(service: &Service) -> [f32; 3] {
    if service.cloud.is_some() {
        direction(CYAN)
    } else if service.external {
        GREY
    } else if !service.platforms.is_empty() {
        direction(ACCENT)
    } else if !service.operations.is_empty() {
        direction(LINK)
    } else if !service.tools.is_empty() {
        direction(MAGENTA)
    } else {
        PALE
    }
}

/// A way's colour, as the flat view draws it.
fn way(via: Via) -> [f32; 3] {
    direction(match via {
        Via::OpenApi => LINK,
        Via::Mcp => MAGENTA,
        Via::Http => YELLOW,
        Via::Cloud => CYAN,
    })
}

/// What a moon stands for: a route the service serves, by its place in the
/// map's, a tool or prompt it offers, by its place in the service's, or
/// those left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Served {
    Route(usize),
    Tool(usize),
    Rest,
}

/// Where a moon turns: what it stands for, and at what angle on its ring.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MoonAt {
    served: Served,
    angle: f32,
}

/// The moons of `service`: the routes it serves, else the tools it offers,
/// at most `MAX_MOONS` and one more for the rest, evenly around its ring.
fn moons(service: &Service) -> Vec<MoonAt> {
    let mut served: Vec<Served> = if service.operations.is_empty() {
        (0..service.tools.len()).map(Served::Tool).collect()
    } else {
        service
            .operations
            .iter()
            .copied()
            .map(Served::Route)
            .collect()
    };
    if served.len() > MAX_MOONS {
        served.truncate(MAX_MOONS);
        served.push(Served::Rest);
    }
    let count = served.len().max(1) as f32;
    served
        .into_iter()
        .enumerate()
        .map(|(i, served)| MoonAt {
            served,
            angle: i as f32 * TAU / count,
        })
        .collect()
}

/// Where the services stand: each in the shell of its layer, outside those
/// it reaches, larger the more it serves and the more it is linked,
/// gathered with those of its repository or folder, those without one with
/// those of their kind.
fn shells(services: &Services, moons: &[Vec<MoonAt>]) -> Shells {
    let count = services.services.len();
    let joins: Vec<(usize, usize)> = services.links.iter().map(|l| (l.from, l.to)).collect();
    let mut degree = vec![0_usize; count];
    for &(from, to) in &joins {
        degree[from] += 1;
        degree[to] += 1;
    }
    let size: Vec<f32> = services
        .services
        .iter()
        .zip(&degree)
        .map(|(s, d)| {
            let weight = (s.operations.len() + s.tools.len() + d) as f32;
            (SMALLEST + LARGER * weight.ln_1p()).min(LARGEST)
        })
        .collect();
    let reach = size
        .iter()
        .zip(moons)
        .map(|(size, moons)| {
            if moons.is_empty() {
                size + 0.4
            } else {
                size + RING + MOON
            }
        })
        .collect();
    let mut names: Vec<String> = Vec::new();
    let group = services
        .services
        .iter()
        .map(|s| {
            let name = match &s.folder {
                Some(folder) => folder.split('/').next().unwrap_or_default().to_owned(),
                None if s.cloud.is_some() => "\0cloud".to_owned(),
                None if s.external => "\0outside".to_owned(),
                None => "\0images".to_owned(),
            };
            names.iter().position(|n| *n == name).unwrap_or_else(|| {
                names.push(name);
                names.len() - 1
            })
        })
        .collect();
    Shells::new(space::layers(count, &joins), size, reach, group, joins)
}

/// What colours a mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tone {
    /// A star, by its service.
    Star(usize),
    /// A route's moon, by its method's colour.
    Route(Color32),
    /// A tool's moon.
    Tool,
    /// The moon of what is left out.
    Rest,
}

/// How a star and its moons are lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Shade {
    Lit,
    /// The service pointed at.
    Picked,
    /// A service the one pointed at has no link with.
    Dim,
}

/// Marks what belongs to the services' scene.
#[derive(Component)]
pub(crate) struct InServices;

/// A star with its ring, by its service's place.
#[derive(Component)]
pub(crate) struct Star(usize);

/// The ring of moons around star `service`.
#[derive(Component)]
pub(crate) struct Ring(usize);

/// What a mesh shows of a service: the star itself, or a moon.
#[derive(Component, Clone, Copy)]
pub(crate) enum Piece {
    Star(usize),
    Moon { service: usize, served: Served },
}

impl Piece {
    fn service(self) -> usize {
        match self {
            Self::Star(service) | Self::Moon { service, .. } => service,
        }
    }
}

/// The services in space, as the panels and the systems share them.
#[derive(Resource, Default)]
pub(crate) struct ServicesSpace {
    /// The services laid out, and the routes' methods, by their place in
    /// the map's.
    services: Option<Services>,
    methods: HashMap<usize, String>,
    shells: Option<Shells>,
    /// Each star's moons.
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
    /// The service pointed at or chosen, and the one chosen, for the
    /// camera.
    focus: Option<usize>,
    chosen: Option<usize>,
    pub(crate) rig: Rig,
    /// Where each star is on screen, the moons of the one pointed at with
    /// what they stand for, and the middles of its links.
    on_screen: Vec<Option<OnScreen>>,
    moons_on_screen: Vec<(Served, Vec2)>,
    links_on_screen: Vec<(usize, Vec2)>,
}

impl ServicesSpace {
    /// Takes `services`, read from `map`, when they are other than those
    /// laid out, and lays them out anew.
    pub(crate) fn take(&mut self, services: &Services, map: &CodeMap) {
        if self.services.as_ref() == Some(services) {
            return;
        }
        self.moons = services.services.iter().map(moons).collect();
        self.methods = self
            .moons
            .iter()
            .flatten()
            .filter_map(|m| match m.served {
                Served::Route(r) => Some((r, map.operations.get(r)?.method.clone())),
                _ => None,
            })
            .collect();
        self.shells = Some(shells(services, &self.moons));
        self.services = Some(services.clone());
        self.respawn = true;
        self.focus = None;
        self.chosen = None;
        self.rig.reframe();
        self.on_screen.clear();
        self.moons_on_screen.clear();
        self.links_on_screen.clear();
    }

    /// The service under `at`, on the window.
    pub(crate) fn under(&self, at: egui::Pos2) -> Option<usize> {
        space::pick(&self.on_screen, Vec2::new(at.x, at.y))
    }

    /// Where the middles of the links of the service pointed at are on the
    /// window, by their place among the links.
    pub(crate) fn links_on_screen(&self) -> &[(usize, Vec2)] {
        &self.links_on_screen
    }

    /// Lights the service `focus`, and frames the one `chosen`, when they
    /// change.
    pub(crate) fn light(&mut self, focus: Option<usize>, chosen: Option<usize>) {
        if self.focus != focus {
            self.focus = focus;
            self.reshade = true;
        }
        if self.chosen != chosen {
            self.chosen = chosen;
            self.rig.reframe();
        }
    }

    /// Whether services `a` and `b` are linked.
    fn linked(&self, a: usize, b: usize) -> bool {
        self.shells.as_ref().is_some_and(|s| s.joined(a, b))
    }

    /// How star `t` and its moons are lit.
    fn shade(&self, t: usize) -> Shade {
        match self.focus {
            Some(f) if f == t => Shade::Picked,
            Some(f) if self.linked(f, t) => Shade::Lit,
            Some(_) => Shade::Dim,
            None => Shade::Lit,
        }
    }

    /// What colours `piece`.
    fn tone(&self, piece: Piece) -> Tone {
        match piece {
            Piece::Star(s) => Tone::Star(s),
            Piece::Moon {
                served: Served::Route(r),
                ..
            } => Tone::Route(api_view::method_colour(
                self.methods.get(&r).map_or("", String::as_str),
            )),
            Piece::Moon {
                served: Served::Tool(_),
                ..
            } => Tone::Tool,
            Piece::Moon {
                served: Served::Rest,
                ..
            } => Tone::Rest,
        }
    }

    /// The material of `tone` shaded `shade`, made the first time asked:
    /// its colour on the side the light reaches, and its own glow.
    fn material(
        &mut self,
        tone: Tone,
        shade: Shade,
        assets: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        let [r, g, b] = match tone {
            Tone::Star(s) => self
                .services
                .as_ref()
                .and_then(|all| all.services.get(s))
                .map_or(PALE, colour),
            Tone::Route(method) => direction(method),
            Tone::Tool => direction(MAGENTA),
            Tone::Rest => GREY,
        };
        let star = matches!(tone, Tone::Star(_));
        let (base, glow) = match shade {
            Shade::Picked if star => (0.5, 1.4),
            Shade::Picked => (0.4, 1.6),
            Shade::Lit if star => (0.4, 0.5),
            Shade::Lit => (0.3, 0.8),
            Shade::Dim => (0.05, 0.05),
        };
        self.materials
            .entry((tone, shade))
            .or_insert_with(|| {
                assets.add(StandardMaterial {
                    base_color: Color::linear_rgb(r * base, g * base, b * base),
                    emissive: LinearRgba::rgb(r * glow, g * glow, b * glow),
                    perceptual_roughness: 0.7,
                    ..default()
                })
            })
            .clone()
    }

    /// How star `t`'s ring is turned: tilted its own way, and turned about
    /// its axis from an angle of its own.
    fn ring_rotation(&self, t: usize) -> Quat {
        tilt(t) * Quat::from_rotation_y(self.spin * SPIN + t as f32 * 2.399_963)
    }

    /// Where `moon` of star `t` is, from its centre.
    fn moon_offset(&self, shells: &Shells, t: usize, moon: &MoonAt) -> Vec3 {
        let local = Vec3::new(moon.angle.cos(), 0.0, moon.angle.sin()) * (shells.size(t) + RING);
        self.ring_rotation(t) * local
    }
}

/// Puts up the scene when shown and laid out, takes it down when left.
pub(crate) fn spawn(
    mut commands: Commands,
    mut space: ResMut<ServicesSpace>,
    universe: Res<Universe>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<InServices>>,
    mut sphere: Local<Option<Handle<Mesh>>>,
) {
    let wanted = universe.space == Some(Space::Services) && space.shells.is_some();
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
    // Lit from above and aside, for each star to show its roundness.
    commands.spawn((
        DirectionalLight {
            illuminance: 3_000.0,
            ..default()
        },
        Transform::from_xyz(40.0, 90.0, 25.0).looking_at(Vec3::ZERO, Vec3::Y),
        InServices,
    ));
    let lit = space.material(Tone::Rest, Shade::Lit, &mut assets);
    for t in 0..shells.len() {
        let size = shells.size(t);
        let moons = space.moons[t].clone();
        commands
            .spawn((
                Transform::from_translation(shells.centre(t)),
                Visibility::Visible,
                Star(t),
                InServices,
            ))
            .with_children(|star| {
                star.spawn((
                    Mesh3d(sphere.clone()),
                    MeshMaterial3d(lit.clone()),
                    Transform::from_scale(Vec3::splat(size)),
                    Piece::Star(t),
                ));
                star.spawn((Transform::default(), Visibility::Inherited, Ring(t)))
                    .with_children(|ring| {
                        for moon in &moons {
                            let at =
                                Vec3::new(moon.angle.cos(), 0.0, moon.angle.sin()) * (size + RING);
                            ring.spawn((
                                Mesh3d(sphere.clone()),
                                MeshMaterial3d(lit.clone()),
                                Transform::from_translation(at).with_scale(Vec3::splat(MOON)),
                                Piece::Moon {
                                    service: t,
                                    served: moon.served,
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

/// What keeps the stars, their rings and the camera apart, for one system
/// to move all three.
type StarsOnly = (Without<UniverseCamera>, Without<Ring>);
type RingsOnly = (Without<UniverseCamera>, Without<Star>);
type CameraOnly = (With<UniverseCamera>, Without<Star>, Without<Ring>);

/// Each frame while shown: the stars' next steps, the rings' turns, the
/// shades, the links and their comets, the camera, and where the names go.
#[allow(clippy::too_many_arguments)]
pub(crate) fn animate(
    mut space: ResMut<ServicesSpace>,
    universe: Res<Universe>,
    time: Res<Time>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut stars: Query<(&Star, &mut Transform), StarsOnly>,
    mut rings: Query<(&Ring, &mut Transform), RingsOnly>,
    mut pieces: Query<(&Piece, &mut MeshMaterial3d<StandardMaterial>)>,
    mut camera: Query<CameraParts, CameraOnly>,
    mut gizmos: Gizmos,
) {
    if universe.space != Some(Space::Services) || !space.spawned {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let t_now = time.elapsed_secs_wrapped();
    if let Some(shells) = space.shells.as_mut()
        && shells.settle()
    {
        for (star, mut transform) in &mut stars {
            transform.translation = shells.centre(star.0);
        }
    }
    // The rings turn while nothing is pointed at, for each moon to stay by
    // its name.
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
            let shade = shades.get(piece.service()).copied().unwrap_or(Shade::Lit);
            let handle = space.material(tone, shade, &mut assets);
            if material.0 != handle {
                material.0 = handle;
            }
        }
    }
    let (Some(shells), Some(services)) = (space.shells.clone(), space.services.clone()) else {
        return;
    };

    // The ring of the service pointed at, the path its moons turn on.
    if let Some(f) = space
        .focus
        .filter(|f| space.moons.get(*f).is_some_and(|m| !m.is_empty()))
    {
        let around = Isometry3d::new(shells.centre(f), tilt(f) * Quat::from_rotation_x(FRAC_PI_2));
        let own = colour(&services.services[f]);
        gizmos
            .circle(
                around,
                shells.size(f) + RING,
                Color::from(hdr(own, 1.0, 0.35)),
            )
            .resolution(64);
    }

    // The links: from the service that reaches, bowing away from the
    // heart, in to the one reached, comets running along each its way.
    // Those of the service pointed at lit; the others faint.
    let mut middles = Vec::new();
    for (i, link) in services.links.iter().enumerate() {
        let (from, to) = (shells.centre(link.from), shells.centre(link.to));
        let a = from + (to - from).normalize_or(Vec3::Y) * shells.size(link.from);
        let b = to + (from - to).normalize_or(Vec3::Y) * shells.size(link.to);
        let points = space::arc(a, b, from, shells.size(link.from), link.from == link.to);
        let colour = way(link.via);
        let lit = space.focus.is_some_and(|f| f == link.from || f == link.to);
        let alpha = match space.focus {
            Some(_) if lit => 0.95,
            Some(_) => 0.05,
            None => 0.45,
        };
        let bright = if lit { 2.4 } else { 1.0 };
        gizmos.linestrip_gradient((0..=24).map(|s| {
            let t = s as f32 / 24.0;
            let colour = hdr(colour, bright, alpha).mix(&hdr(colour, bright * 0.5, alpha * 0.7), t);
            (space::bezier(points, t), Color::from(colour))
        }));
        // More comets the more a link carries.
        let comets = (1 + link.operations.len().min(COMETS - 1)).min(COMETS);
        for c in 0..comets {
            let t =
                (t_now * COMET_SPEED + c as f32 / comets as f32 + i as f32 * 0.37).fract() * 0.95;
            gizmos.line(
                space::bezier(points, t),
                space::bezier(points, t + 0.04),
                Color::from(hdr(colour, if lit { 7.0 } else { 3.5 }, alpha.max(0.3))),
            );
        }
        if lit {
            middles.push((i, space::bezier(points, 0.5)));
        }
    }

    // The camera: drifting while nothing is pointed at, turned by a drag,
    // gliding to frame the whole, or the service chosen.
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
                Some((m.served, at))
            })
            .collect(),
        None => Vec::new(),
    };
    space.links_on_screen = middles
        .into_iter()
        .filter_map(|(i, at)| Some((i, view.project(at)?)))
        .collect();
}

/// The services in space, drawn over their stars in `rect` under the
/// header the view draws, from `top` down: their names, and what the one
/// pointed at serves around its ring.
pub(crate) fn draw(
    painter: &Painter,
    rect: Rect,
    top: f32,
    space: &ServicesSpace,
    services: &Services,
    map: &CodeMap,
) {
    let canvas = painter.with_clip_rect(Rect::from_min_max(egui::pos2(rect.left(), top), rect.max));
    let focus = space.focus;
    let related = |t: usize| focus.is_some_and(|f| space.linked(f, t));
    space::names(
        painter,
        &canvas,
        &space.on_screen,
        |t| services.services[t].name.as_str(),
        &Naming {
            focus,
            related: &related,
            kept: &[],
            few: services.services.len() <= 40,
        },
    );

    // What the service pointed at serves, on both sides of its ring.
    if let Some(f) = focus
        && let Some(service) = services.services.get(f)
        && let Some(star) = space.on_screen.get(f).copied().flatten()
    {
        let size = 12.0;
        space::beside(
            &canvas,
            star,
            &space.moons_on_screen,
            size + 3.0,
            // The details cover the right.
            space.chosen.is_some().then_some(true),
            |painter, at, leftwards, served| match served {
                Served::Route(r) => {
                    if let Some(operation) = map.operations.get(r) {
                        space::parts(
                            painter,
                            at,
                            leftwards,
                            &[
                                (
                                    &operation.method,
                                    FontId::monospace(size * 0.9),
                                    api_view::method_colour(&operation.method),
                                ),
                                (
                                    &space::shorten(&operation.path, 40),
                                    FontId::monospace(size),
                                    TEXT,
                                ),
                            ],
                        );
                    }
                }
                Served::Tool(t) => {
                    if let Some(tool) = service.tools.get(t) {
                        let kind = match tool.kind {
                            ToolKind::Tool => "tool",
                            ToolKind::Prompt => "prompt",
                        };
                        space::parts(
                            painter,
                            at,
                            leftwards,
                            &[
                                (&tool.name, FontId::monospace(size), TEXT),
                                (kind, FontId::proportional(size * 0.9), DIM),
                            ],
                        );
                    }
                }
                Served::Rest => {
                    let served = service.operations.len().max(service.tools.len());
                    shadowed(
                        painter,
                        at,
                        if leftwards {
                            Align2::RIGHT_CENTER
                        } else {
                            Align2::LEFT_CENTER
                        },
                        &format!("and {} more", served.saturating_sub(MAX_MOONS)),
                        FontId::proportional(size),
                        DIM,
                    );
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_codemap::{ServiceLink, Tool};

    fn service(name: &str, folder: Option<&str>, operations: usize, tools: usize) -> Service {
        Service {
            name: name.to_owned(),
            folder: folder.map(str::to_owned),
            external: folder.is_none(),
            operations: (0..operations).collect(),
            tools: (0..tools)
                .map(|i| Tool {
                    name: format!("tool{i}"),
                    kind: ToolKind::Tool,
                    summary: String::new(),
                    path: String::new(),
                    line: 1,
                })
                .collect(),
            cloud: None,
            platforms: Vec::new(),
        }
    }

    fn link(from: usize, to: usize, via: Via) -> ServiceLink {
        ServiceLink {
            from,
            to,
            via,
            operations: Vec::new(),
            evidence: Vec::new(),
        }
    }

    #[test]
    fn a_service_stands_outside_what_it_reaches_with_what_it_serves_around_it() {
        // A front end reaches an API, which reaches tools and a service
        // outside.
        let services = Services {
            services: vec![
                service("web", Some("web"), 0, 0),
                service("api", Some("api"), 30, 0),
                service("tools", Some("tools"), 0, 3),
                service("mail", None, 0, 0),
            ],
            links: vec![
                link(0, 1, Via::OpenApi),
                link(1, 2, Via::Mcp),
                link(1, 3, Via::Http),
            ],
            centre: Some(1),
        };
        let moons: Vec<Vec<MoonAt>> = services.services.iter().map(moons).collect();
        let mut shells = shells(&services, &moons);
        for _ in 0..3_000 {
            if shells.settled() {
                break;
            }
            shells.step();
        }
        let from_middle = |t: usize| shells.centre(t).length();
        assert!(from_middle(0) > from_middle(1));
        assert!(from_middle(1) > from_middle(2));
        assert!(from_middle(1) > from_middle(3));
        // The API's routes are its moons, at most a dozen and one for the
        // rest; the tools' service turns its tools; the front end none.
        assert_eq!(moons[1].len(), MAX_MOONS + 1);
        assert_eq!(moons[1].last().map(|m| m.served), Some(Served::Rest));
        assert_eq!(moons[2].len(), 3);
        assert!(moons[0].is_empty());
        // Serving more and linked more, the API is the largest.
        assert!((0..4).all(|t| t == 1 || shells.size(1) > shells.size(t)));
        // Each kind its colour.
        assert_eq!(colour(&services.services[1]), direction(LINK));
        assert_eq!(colour(&services.services[2]), direction(MAGENTA));
        assert_eq!(colour(&services.services[3]), GREY);
    }

    #[test]
    fn the_same_services_are_laid_out_once() {
        let services = Services {
            services: vec![service("api", Some("api"), 1, 0)],
            links: Vec::new(),
            centre: None,
        };
        let map = CodeMap::default();
        let mut space = ServicesSpace::default();
        space.take(&services, &map);
        assert!(space.respawn);
        space.respawn = false;
        space.take(&services, &map);
        assert!(!space.respawn);
    }
}
