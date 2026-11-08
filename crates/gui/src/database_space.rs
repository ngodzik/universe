//! The database in space, as the database view first shows it: each table
//! a planet in its file's colour, larger the more tables point at it, its
//! keys moons on a ring close around it and its other columns on a ring
//! beyond; the tables that point at none gathered at the heart, the others
//! in shells around it by their foreign keys, each outside what it points
//! at and near it, and near its file's other tables; each foreign key an
//! arc from the moon of its column in to the one it points at, bowing away
//! from the heart. The scene shares the universe's camera and sky: the
//! camera drifts, turns on a drag and comes closer with the wheel, and the
//! moons turn while nothing is pointed at.
//!
//! A table pointed at lights its keys, the tables they join and its
//! columns' names; a click glides to it and shows it whole, a double click
//! opens its model.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::path::PathBuf;
use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, FontId, Rect, Response, Stroke, Ui};
use universe_codemap::{Column, ForeignKey, Schema, Table};
use universe_ui::App;

use crate::database_view::{self, DETAILS_WIDTH, DatabaseView};
use crate::theme::{CYAN, DIM, TEXT, YELLOW};
use crate::universe::{PALETTE, Universe, UniverseCamera};
use crate::universe_view::shadowed;

/// The most columns a planet's moons show; the rest make one moon more.
const MAX_MOONS: usize = 16;

/// A planet's radius: the smallest, how much larger the tables pointing at
/// it make it, and the largest.
const SMALLEST: f32 = 0.55;
const LARGER: f32 = 0.16;
const LARGEST: f32 = 1.4;

/// How far out from a planet's surface its rings of moons turn: the keys',
/// then the other columns'.
const INNER: f32 = 0.5;
const OUTER: f32 = 1.0;

/// A moon's radius: a key's, and another column's.
const KEY_MOON: f32 = 0.15;
const MOON: f32 = 0.11;

/// How fast the rings turn, in radians a second: the keys', and the
/// others'.
const INNER_SPIN: f32 = 0.35;
const OUTER_SPIN: f32 = 0.2;

/// The room kept between two planets' outer rings, how much more room the
/// heart and a shell take than their planets would packed tight, how far a
/// shell is at least beyond the one inside, and how far within it a planet
/// may stand nearer or farther.
const ROOM: f32 = 1.2;
const PACK: f32 = 1.7;
const STEP: f32 = 3.0;
const SLACK: f32 = 1.0;

/// How the planets find their places: how hard planets push each other
/// away, a foreign key pulls its tables together, a file gathers its
/// tables, and an orbit holds its planets to its middle.
const PUSH: f32 = 0.35;
const JOIN: f32 = 0.06;
const KIN: f32 = 0.025;
const HOLD: f32 = 0.02;

/// How far a planet may move in a step at first, how much less each step,
/// and below what it has settled.
const HEAT: f32 = 1.5;
const COOLING: f32 = 0.985;
const SETTLED: f32 = 0.01;

/// The pairs of planets a frame's steps may weigh: fewer steps a frame for
/// a larger schema, for the window to stay smooth while it settles.
const PAIRS_PER_FRAME: usize = 240_000;

/// The camera's nearest, and the farthest it frames the whole from.
const NEAREST: f32 = 4.0;
const FARTHEST: f32 = 420.0;

/// The colours of the keys, as directions in HDR: primary keys gold,
/// foreign keys cyan, the moon of the columns left out grey.
const GOLD: [f32; 3] = [1.0, 0.7, 0.2];
const TEAL: [f32; 3] = [0.2, 0.8, 1.0];
const GREY: [f32; 3] = [0.4, 0.44, 0.55];
/// The colour of the arcs leaving the table pointed at: the 2D view's
/// accent, as a direction in HDR.
const ORANGE: [f32; 3] = [1.0, 0.45, 0.25];

/// The `i`th of `n` directions spread evenly over a sphere.
fn fibonacci(i: usize, n: usize) -> Vec3 {
    let y = 1.0 - 2.0 * (i as f32 + 0.5) / n.max(1) as f32;
    let across = (1.0 - y * y).max(0.0).sqrt();
    let angle = i as f32 * 2.399_963;
    Vec3::new(across * angle.cos(), y, across * angle.sin())
}

/// Where the planets are: each in the shell of its layer around the core,
/// and in it near the tables it is joined to by a foreign key and its
/// file's others.
#[derive(Debug, Clone)]
pub(crate) struct Orbits {
    /// Each planet's shell, from the core.
    orbit: Vec<usize>,
    /// Where each is.
    spots: Vec<Vec3>,
    /// Each one's radius, and how far out its outer ring reaches.
    size: Vec<f32>,
    reach: Vec<f32>,
    /// Each one's file, by its place among the files.
    file: Vec<usize>,
    files: usize,
    /// The pairs of tables a foreign key joins, once each, the lower
    /// first, a table never with itself.
    joins: Vec<(usize, usize)>,
    /// How near the middle and how far from it the planets of each shell
    /// stand: those of the heart from the middle itself, those of each
    /// other shell beyond the one inside.
    bands: Vec<(f32, f32)>,
    /// How far a planet may move in the next step.
    heat: f32,
}

impl Orbits {
    /// The orbits of `schema`, in their first places: each file's tables
    /// toward a side of their own, in the shell of their layer.
    pub(crate) fn new(schema: &Schema) -> Self {
        let count = schema.tables.len();
        let mut joins: Vec<(usize, usize)> = Vec::new();
        let mut pointers: Vec<Vec<usize>> = vec![Vec::new(); count];
        for key in schema.foreign_keys().iter().filter(|k| k.from != k.to) {
            let pair = (key.from.min(key.to), key.from.max(key.to));
            if !joins.contains(&pair) {
                joins.push(pair);
            }
            if !pointers[key.to].contains(&key.from) {
                pointers[key.to].push(key.from);
            }
        }
        let size: Vec<f32> = pointers
            .iter()
            .map(|p| (SMALLEST + LARGER * (p.len() as f32).sqrt()).min(LARGEST))
            .collect();
        let reach: Vec<f32> = size.iter().map(|s| s + OUTER + MOON).collect();
        let mut paths: Vec<&str> = schema.tables.iter().map(|t| t.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        let file: Vec<usize> = schema
            .tables
            .iter()
            .map(|t| paths.binary_search(&t.path.as_str()).unwrap_or(0))
            .collect();
        let orbit = database_view::layers(schema);
        // The heart a ball as large as its planets need, each shell a
        // sphere as large as its planets need, a step beyond the one inside.
        let orbits = orbit.iter().copied().max().map_or(0, |o| o + 1);
        let mut bands: Vec<(f32, f32)> = Vec::with_capacity(orbits);
        for o in 0..orbits {
            let cells: Vec<f32> = (0..count)
                .filter(|t| orbit[*t] == o)
                .map(|t| 2.0 * reach[t] + ROOM)
                .collect();
            let band = match bands.last() {
                None => {
                    let volume: f32 = cells.iter().map(|c| c.powi(3)).sum();
                    (0.0, (volume * PACK * 3.0 / (4.0 * PI)).cbrt())
                }
                Some((_, inside)) => {
                    let area: f32 = cells.iter().map(|c| c * c).sum();
                    let radius = (area * PACK / (4.0 * PI)).sqrt().max(inside + STEP);
                    (radius, radius + SLACK)
                }
            };
            bands.push(band);
        }
        let files = paths.len().max(1);
        let spots = (0..count)
            .map(|t| {
                let (start, end) = bands[orbit[t]];
                // Toward its file's side, each a little aside, for no two
                // to start on one spot.
                let way =
                    (fibonacci(file[t], files) + fibonacci(t, count) * 0.35).normalize_or(Vec3::Y);
                let radius = if orbit[t] == 0 {
                    end * 0.5
                } else {
                    (start + end) * 0.5
                };
                way * radius
            })
            .collect();
        Self {
            orbit,
            spots,
            size,
            reach,
            file,
            files,
            joins,
            bands,
            heat: HEAT,
        }
    }

    /// How many planets.
    fn len(&self) -> usize {
        self.spots.len()
    }

    /// Whether the planets have found their places.
    pub(crate) fn settled(&self) -> bool {
        self.heat < SETTLED
    }

    /// Moves each planet a step toward its place: away from the others in
    /// its shell, where they would stand in each other, and a little from
    /// the rest; toward those it is joined to, the rest of its file and the
    /// middle of its shell; never out of it.
    pub(crate) fn step(&mut self) {
        let count = self.len();
        let mut push = vec![Vec3::ZERO; count];
        for a in 0..count {
            for b in a + 1..count {
                let apart = self.spots[a] - self.spots[b];
                let distance = apart.length();
                // Two planets on one spot part along a way of their own.
                let direction = if distance < 1e-3 {
                    fibonacci((a * 31 + b) % 97, 97)
                } else {
                    apart / distance
                };
                let distance = distance.max(0.05);
                // Shells near each other leave too little room between them
                // for a planet not to stand in its neighbour's.
                let weight = match self.orbit[a].abs_diff(self.orbit[b]) {
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
        let mut middles = vec![(Vec3::ZERO, 0_usize); self.files];
        for (spot, &file) in self.spots.iter().zip(&self.file) {
            middles[file].0 += *spot;
            middles[file].1 += 1;
        }
        for (t, push) in push.iter_mut().enumerate() {
            let (sum, members) = middles[self.file[t]];
            let middle = sum / members.max(1) as f32;
            let (start, end) = self.bands[self.orbit[t]];
            let held = if self.orbit[t] == 0 {
                0.0
            } else {
                (start + end) * 0.5
            };
            let spot = self.spots[t];
            let out = spot.normalize_or(Vec3::Y);
            *push += (middle - spot) * KIN + out * (held - spot.length()) * HOLD;
        }
        for (t, push) in push.into_iter().enumerate() {
            let spot = self.spots[t] + push.clamp_length_max(self.heat);
            let (nearest, farthest) = self.bands[self.orbit[t]];
            let out = spot.normalize_or(fibonacci(t, count));
            self.spots[t] = out * spot.length().clamp(nearest, farthest);
        }
        self.heat *= COOLING;
    }

    /// The centre of planet `t`.
    pub(crate) fn centre(&self, t: usize) -> Vec3 {
        self.spots[t]
    }

    /// Whether tables `a` and `b` are joined by a foreign key.
    fn joined(&self, a: usize, b: usize) -> bool {
        self.joins.contains(&(a.min(b), a.max(b)))
    }

    /// The middle of the box the planets fill, and how far from it the
    /// farthest one's outer ring reaches.
    fn bounds(&self) -> (Vec3, f32) {
        let low = self.spots.iter().copied().fold(Vec3::MAX, Vec3::min);
        let high = self.spots.iter().copied().fold(Vec3::MIN, Vec3::max);
        let middle = if self.spots.is_empty() {
            Vec3::ZERO
        } else {
            (low + high) * 0.5
        };
        let reach = (0..self.len())
            .map(|t| self.spots[t].distance(middle) + self.reach[t])
            .fold(0.0_f32, f32::max);
        (middle, reach)
    }
}

/// Where a moon turns: its column, `None` for the one of the columns left
/// out; whether on the ring of the keys; at what angle on its ring.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MoonAt {
    column: Option<usize>,
    inner: bool,
    angle: f32,
}

/// The moons of `table`: its keys on the inner ring, the primary first,
/// its other columns on the outer one, and one more there for those left
/// out; each ring's moons evenly around it.
fn moons(table: &Table) -> Vec<MoonAt> {
    let (keys, rest): (Vec<usize>, Vec<usize>) = database_view::listed(table, MAX_MOONS)
        .into_iter()
        .partition(|c| {
            let column = &table.columns[*c];
            column.primary || column.references.is_some()
        });
    let mut rest: Vec<Option<usize>> = rest.into_iter().map(Some).collect();
    if table.columns.len() > MAX_MOONS {
        rest.push(None);
    }
    let ring = |columns: Vec<Option<usize>>, inner: bool| {
        let count = columns.len().max(1) as f32;
        columns
            .into_iter()
            .enumerate()
            .map(move |(i, column)| MoonAt {
                column,
                inner,
                angle: i as f32 * TAU / count,
            })
    };
    ring(keys.into_iter().map(Some).collect(), true)
        .chain(ring(rest, false))
        .collect()
}

/// The moon of `column` among `moons`, or that of the columns left out.
fn moon_of(moons: &[MoonAt], column: usize) -> Option<&MoonAt> {
    moons
        .iter()
        .find(|m| m.column == Some(column))
        .or_else(|| moons.iter().find(|m| m.column.is_none()))
}

/// What colours a mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tone {
    /// A planet, in its file's colour.
    Body(usize),
    Primary,
    Foreign,
    /// Another column's moon, in its file's colour.
    Column(usize),
    /// The moon of the columns left out.
    Rest,
}

/// How a planet and its moons are lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Shade {
    Lit,
    /// The table pointed at.
    Picked,
    /// A table the one pointed at is not joined to.
    Dim,
    /// A table the search leaves out.
    Faded,
}

/// The material of a mesh of `tone` shaded `shade`: its colour on the side
/// the light reaches, and its own glow, the keys' brighter, to bloom.
fn material(tone: Tone, shade: Shade) -> StandardMaterial {
    let [r, g, b] = match tone {
        Tone::Body(file) | Tone::Column(file) => PALETTE[file % PALETTE.len()],
        Tone::Primary => GOLD,
        Tone::Foreign => TEAL,
        Tone::Rest => GREY,
    };
    let key = matches!(tone, Tone::Primary | Tone::Foreign);
    let body = matches!(tone, Tone::Body(_));
    let (base, glow) = match shade {
        Shade::Picked if key => (0.4, 3.4),
        Shade::Picked if body => (0.5, 1.2),
        Shade::Picked => (0.4, 0.9),
        Shade::Lit if key => (0.3, 1.6),
        Shade::Lit if body => (0.45, 0.4),
        Shade::Lit => (0.3, 0.32),
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

/// `[r, g, b]` made `brightness` times brighter, with an alpha of its own.
fn hdr([r, g, b]: [f32; 3], brightness: f32, alpha: f32) -> LinearRgba {
    LinearRgba::new(r * brightness, g * brightness, b * brightness, alpha)
}

/// The point at `t`, from 0 to 1, of the cubic Bézier curve of `points`.
fn bezier(points: [Vec3; 4], t: f32) -> Vec3 {
    let u = 1.0 - t;
    points[0] * (u * u * u)
        + points[1] * (3.0 * u * u * t)
        + points[2] * (3.0 * u * t * t)
        + points[3] * (t * t * t)
}

/// Where a planet is on screen, in logical pixels: its centre, its radius
/// and how far its outer ring reaches there, where its name goes, and how
/// far it is from the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OnScreen {
    centre: Vec2,
    radius: f32,
    reach: f32,
    label: Vec2,
    depth: f32,
}

/// The planet under `at`: of those it falls on, the nearest the camera;
/// else, of the rings it falls in, the one it is the nearest the middle of.
fn pick(on_screen: &[Option<OnScreen>], at: Vec2) -> Option<usize> {
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
fn spread(ys: &[f32], gap: f32) -> Vec<f32> {
    let mut spread: Vec<f32> = Vec::with_capacity(ys.len());
    for &y in ys {
        let y = spread.last().map_or(y, |above| y.max(above + gap));
        spread.push(y);
    }
    let back = (ys.iter().sum::<f32>() - spread.iter().sum::<f32>()) / ys.len().max(1) as f32;
    spread.iter().map(|y| y + back).collect()
}

/// Marks what belongs to the database's scene.
#[derive(Component)]
pub(crate) struct InSchema;

/// A planet with its rings, by its table's place in the schema.
#[derive(Component)]
pub(crate) struct Planet(usize);

/// A ring of moons around planet `table`: the keys', or the others'.
#[derive(Component)]
pub(crate) struct Ring {
    table: usize,
    inner: bool,
}

/// What a mesh shows of a table: the planet itself, or a moon, of a column
/// or, `None`, of those left out.
#[derive(Component, Clone, Copy)]
pub(crate) enum Piece {
    Body(usize),
    Moon { table: usize, column: Option<usize> },
}

impl Piece {
    fn table(self) -> usize {
        match self {
            Self::Body(table) | Self::Moon { table, .. } => table,
        }
    }
}

/// The database in space, as the panels and the systems share it.
#[derive(Resource)]
pub(crate) struct DatabaseSpace {
    schema: Option<Arc<Schema>>,
    keys: Vec<ForeignKey>,
    orbits: Option<Orbits>,
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
    /// What the search keeps, the table pointed at or chosen, and the one
    /// chosen, for the camera.
    kept: Vec<bool>,
    focus: Option<usize>,
    chosen: Option<usize>,
    /// The camera: around which point, from which angles, how far.
    centre: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// How far the camera is heading, and whether the wheel set it since
    /// the view last framed.
    target_distance: f32,
    zoomed: bool,
    /// What the panels report of the mouse, for the next frame.
    drag: Vec2,
    zoom: f32,
    /// How much of the window's width the panels cover on the right, and
    /// the details on the left.
    pub(crate) covered: f32,
    left: f32,
    /// Where each planet is on screen, and the moons of the one pointed at
    /// with their columns.
    on_screen: Vec<Option<OnScreen>>,
    columns_on_screen: Vec<(Option<usize>, Vec2)>,
}

impl Default for DatabaseSpace {
    fn default() -> Self {
        Self {
            schema: None,
            keys: Vec::new(),
            orbits: None,
            moons: Vec::new(),
            spawned: false,
            respawn: false,
            materials: HashMap::new(),
            reshade: false,
            spin: 0.0,
            kept: Vec::new(),
            focus: None,
            chosen: None,
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
            on_screen: Vec::new(),
            columns_on_screen: Vec::new(),
        }
    }
}

impl DatabaseSpace {
    /// Takes `schema`, when another was read, and lays it out anew.
    fn take(&mut self, schema: &Arc<Schema>) {
        if self.schema.as_ref().is_some_and(|s| Arc::ptr_eq(s, schema)) {
            return;
        }
        self.moons = schema.tables.iter().map(moons).collect();
        self.orbits = Some(Orbits::new(schema));
        self.keys = schema.foreign_keys();
        self.schema = Some(Arc::clone(schema));
        self.respawn = true;
        self.kept.clear();
        self.focus = None;
        self.chosen = None;
        self.zoomed = false;
        self.on_screen.clear();
        self.columns_on_screen.clear();
    }

    /// Lights what `kept`, the table `focus` and the one `chosen` say, when
    /// they change.
    fn light(&mut self, kept: &[bool], focus: Option<usize>, chosen: Option<usize>) {
        if self.kept != kept || self.focus != focus {
            kept.clone_into(&mut self.kept);
            self.focus = focus;
            self.reshade = true;
        }
        if self.chosen != chosen {
            self.chosen = chosen;
            self.zoomed = false;
        }
    }

    /// How planet `t` and its moons are lit.
    fn shade(&self, t: usize) -> Shade {
        if !self.kept.get(t).copied().unwrap_or(true) {
            return Shade::Faded;
        }
        match (self.focus, &self.orbits) {
            (Some(f), _) if f == t => Shade::Picked,
            (Some(f), Some(orbits)) if orbits.joined(f, t) => Shade::Lit,
            (Some(_), _) => Shade::Dim,
            (None, _) => Shade::Lit,
        }
    }

    /// What colours `piece`.
    fn tone(&self, piece: Piece) -> Tone {
        let file = self.orbits.as_ref().map_or(0, |o| o.file[piece.table()]);
        let column = match piece {
            Piece::Body(_) => return Tone::Body(file),
            Piece::Moon { table, column } => column.and_then(|c| {
                let table = self.schema.as_ref()?.tables.get(table)?;
                table.columns.get(c)
            }),
        };
        match column {
            None => Tone::Rest,
            Some(c) if c.primary => Tone::Primary,
            Some(c) if c.references.is_some() => Tone::Foreign,
            Some(_) => Tone::Column(file),
        }
    }

    /// The material of `tone` shaded `shade`, made the first time asked.
    fn material(
        &mut self,
        tone: Tone,
        shade: Shade,
        assets: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.materials
            .entry((tone, shade))
            .or_insert_with(|| assets.add(material(tone, shade)))
            .clone()
    }

    /// How far planet `t`'s ring has turned: each planet's from an angle of
    /// its own, the keys' faster.
    fn turn(&self, t: usize, inner: bool) -> f32 {
        let phase = t as f32 * 2.399_963;
        if inner {
            self.spin * INNER_SPIN + phase
        } else {
            self.spin * OUTER_SPIN - phase
        }
    }

    /// How planet `t`'s ring is turned: tilted as the planet's rings are,
    /// and turned about their axis.
    fn ring_rotation(&self, t: usize, inner: bool) -> Quat {
        tilt(t) * Quat::from_rotation_y(self.turn(t, inner))
    }

    /// Where `moon` of planet `t` is, from its centre.
    fn moon_offset(&self, orbits: &Orbits, t: usize, moon: &MoonAt) -> Vec3 {
        let radius = orbits.size[t] + if moon.inner { INNER } else { OUTER };
        let local = Vec3::new(moon.angle.cos(), 0.0, moon.angle.sin()) * radius;
        self.ring_rotation(t, moon.inner) * local
    }

    /// Where the moon of `column` of planet `t` is, that of the columns left
    /// out when it is one; on the planet's surface facing `toward` when no
    /// column is said or none has a moon.
    fn moon_at(&self, orbits: &Orbits, t: usize, column: Option<usize>, toward: Vec3) -> Vec3 {
        let centre = orbits.centre(t);
        match column.and_then(|c| moon_of(&self.moons[t], c)) {
            Some(moon) => centre + self.moon_offset(orbits, t, moon),
            None => {
                let way = (toward - centre).try_normalize().unwrap_or(Vec3::Y);
                centre + way * orbits.size[t]
            }
        }
    }
}

/// How planet `t`'s rings are tilted: each about an axis of its own, for
/// no two neighbours to show theirs the same way.
fn tilt(t: usize) -> Quat {
    Quat::from_rotation_y(t as f32 * 1.1)
        * Quat::from_rotation_x(0.3 + 0.25 * ((t * 7) % 5) as f32 / 4.0)
}

/// Puts up the scene when shown and laid out, takes it down when left.
pub(crate) fn spawn(
    mut commands: Commands,
    mut space: ResMut<DatabaseSpace>,
    universe: Res<Universe>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<InSchema>>,
    mut sphere: Local<Option<Handle<Mesh>>>,
) {
    let wanted = universe.schema && space.orbits.is_some();
    if (!wanted || space.respawn) && space.spawned {
        for entity in &existing {
            commands.entity(entity).despawn();
        }
        space.spawned = false;
    }
    if !wanted || space.spawned {
        return;
    }
    let Some(orbits) = space.orbits.clone() else {
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
        InSchema,
    ));
    let lit = space.material(Tone::Rest, Shade::Lit, &mut assets);
    for t in 0..orbits.len() {
        let size = orbits.size[t];
        let moons = space.moons[t].clone();
        commands
            .spawn((
                Transform::from_translation(orbits.centre(t)),
                Visibility::Visible,
                Planet(t),
                InSchema,
            ))
            .with_children(|planet| {
                planet.spawn((
                    Mesh3d(sphere.clone()),
                    MeshMaterial3d(lit.clone()),
                    Transform::from_scale(Vec3::splat(size)),
                    Piece::Body(t),
                ));
                for inner in [true, false] {
                    let radius = size + if inner { INNER } else { OUTER };
                    let scale = if inner { KEY_MOON } else { MOON };
                    planet
                        .spawn((
                            Transform::default(),
                            Visibility::Inherited,
                            Ring { table: t, inner },
                        ))
                        .with_children(|ring| {
                            for moon in moons.iter().filter(|m| m.inner == inner) {
                                let at =
                                    Vec3::new(moon.angle.cos(), 0.0, moon.angle.sin()) * radius;
                                ring.spawn((
                                    Mesh3d(sphere.clone()),
                                    MeshMaterial3d(lit.clone()),
                                    Transform::from_translation(at).with_scale(Vec3::splat(scale)),
                                    Piece::Moon {
                                        table: t,
                                        column: moon.column,
                                    },
                                ));
                            }
                        });
                }
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

/// Each frame while shown: the planets' next steps, the rings' turns, the
/// shades, the orbits and the keys, the camera, and where the names go.
#[allow(clippy::too_many_arguments)]
pub(crate) fn animate(
    mut space: ResMut<DatabaseSpace>,
    universe: Res<Universe>,
    time: Res<Time>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut planets: Query<(&Planet, &mut Transform), PlanetsOnly>,
    mut rings: Query<(&Ring, &mut Transform), RingsOnly>,
    mut pieces: Query<(&Piece, &mut MeshMaterial3d<StandardMaterial>)>,
    mut camera: Query<CameraParts, CameraOnly>,
    mut gizmos: Gizmos,
) {
    if !universe.schema || !space.spawned {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let t_now = time.elapsed_secs_wrapped();
    if let Some(layout) = space.orbits.as_mut()
        && !layout.settled()
    {
        let pairs = (layout.len() * layout.len() / 2).max(1);
        for _ in 0..(PAIRS_PER_FRAME / pairs).clamp(1, 4) {
            layout.step();
        }
        for (planet, mut transform) in &mut planets {
            transform.translation = layout.centre(planet.0);
        }
    }
    // The rings turn while nothing is pointed at, for each moon to stay by
    // its column's name.
    if space.focus.is_none() {
        space.spin += dt;
    }
    for (ring, mut transform) in &mut rings {
        transform.rotation = space.ring_rotation(ring.table, ring.inner);
    }
    if std::mem::take(&mut space.reshade) {
        let count = space.orbits.as_ref().map_or(0, Orbits::len);
        let shades: Vec<Shade> = (0..count).map(|t| space.shade(t)).collect();
        for (piece, mut material) in &mut pieces {
            let tone = space.tone(*piece);
            let shade = shades.get(piece.table()).copied().unwrap_or(Shade::Lit);
            let handle = space.material(tone, shade, &mut assets);
            if material.0 != handle {
                material.0 = handle;
            }
        }
    }
    let Some(layout) = space.orbits.as_ref() else {
        return;
    };
    // The rings of the table pointed at, the paths its moons turn on.
    if let Some(f) = space.focus.filter(|f| *f < layout.len()) {
        let around = Isometry3d::new(layout.centre(f), tilt(f) * Quat::from_rotation_x(FRAC_PI_2));
        let own = PALETTE[layout.file[f] % PALETTE.len()];
        for (out, colour) in [(INNER, GOLD), (OUTER, own)] {
            gizmos
                .circle(
                    around,
                    layout.size[f] + out,
                    Color::from(hdr(colour, 1.0, 0.35)),
                )
                .resolution(64);
        }
    }

    // The keys: from the moon of the column that points, bowing away from
    // the core, in to the moon of the column pointed at. Those of the table
    // pointed at lit, a spark running along each from the table that points
    // to the one pointed at; the others faint.
    for (i, key) in space.keys.iter().enumerate() {
        let lit = |t: usize| space.kept.get(t).copied().unwrap_or(true);
        let (from, to) = (layout.centre(key.from), layout.centre(key.to));
        let a = space.moon_at(layout, key.from, Some(key.from_column), to);
        let b = space.moon_at(layout, key.to, key.to_column, from);
        let points = if key.from == key.to {
            // A table pointing at itself: a loop out of it, away from the
            // core.
            let out = from.normalize_or(Vec3::Y) * (layout.size[key.from] + 2.0);
            [a, a + out, b + out, b]
        } else {
            let out = ((a + b) * 0.5).normalize_or(Vec3::Y) * (a.distance(b) * 0.25).max(1.0);
            let way = (b - a) * 0.2;
            [a, a + out + way, b + out - way, b]
        };
        let (start, end, alpha, spark) = match space.focus {
            Some(f) if f == key.from => (ORANGE, ORANGE, 0.95, Some(ORANGE)),
            Some(f) if f == key.to => (TEAL, TEAL, 0.95, Some(TEAL)),
            Some(_) => (TEAL, GOLD, 0.05, None),
            None if lit(key.from) && lit(key.to) => (TEAL, GOLD, 0.4, None),
            None => (TEAL, GOLD, 0.05, None),
        };
        let bright = if spark.is_some() { 2.4 } else { 1.0 };
        gizmos.linestrip_gradient((0..=24).map(|s| {
            let t = s as f32 / 24.0;
            let colour = hdr(start, bright, alpha).mix(&hdr(end, bright, alpha), t);
            (bezier(points, t), Color::from(colour))
        }));
        if let Some(colour) = spark {
            let t = (t_now * 0.5 + i as f32 * 0.29).fract() * 0.94;
            gizmos.line(
                bezier(points, t),
                bezier(points, t + 0.06),
                Color::from(hdr(colour, 6.0, 1.0)),
            );
        }
    }

    // The camera: drifting while nothing is pointed at, turned by a drag,
    // gliding to frame the whole, or the table chosen.
    let (middle, reach) = layout.bounds();
    let goal = match space.chosen.filter(|c| *c < layout.len()) {
        Some(c) => (layout.centre(c), (layout.reach[c] * 7.0).max(14.0)),
        None => (middle, (reach * 2.9).clamp(12.0, FARTHEST)),
    };
    let planets_at: Vec<(Vec3, f32, f32)> = (0..layout.len())
        .map(|t| (layout.centre(t), layout.size[t], layout.reach[t]))
        .collect();
    let moons_at: Vec<(Option<usize>, Vec3)> = match space.focus.filter(|f| *f < layout.len()) {
        Some(f) => space.moons[f]
            .iter()
            .map(|m| (m.column, layout.centre(f) + space.moon_offset(layout, f, m)))
            .collect(),
        None => Vec::new(),
    };
    let drag = std::mem::take(&mut space.drag);
    let zoom = std::mem::take(&mut space.zoom);
    if drag == Vec2::ZERO && space.focus.is_none() {
        space.yaw += 0.04 * dt;
    }
    space.yaw -= drag.x * 0.006;
    space.pitch = (space.pitch + drag.y * 0.006).clamp(-1.3, 1.45);
    if zoom != 0.0 {
        space.zoomed = true;
        space.target_distance *= (-zoom * 0.0015).clamp(-1.5, 1.5).exp();
    } else if !space.zoomed {
        space.target_distance = goal.1;
    }
    space.target_distance = space.target_distance.clamp(NEAREST, FARTHEST * 1.4);
    let ease = 1.0 - (-3.0 * dt).exp();
    space.centre = space.centre.lerp(goal.0, ease);
    let target = space.target_distance;
    space.distance += (target - space.distance) * ease;
    let Ok((mut transform, camera, global)) = camera.single_mut() else {
        return;
    };
    let offset = Vec3::new(
        space.yaw.cos() * space.pitch.cos(),
        space.pitch.sin(),
        space.yaw.sin() * space.pitch.cos(),
    ) * space.distance;
    let looking =
        Transform::from_translation(space.centre + offset).looking_at(space.centre, Vec3::Y);
    // Moved sideways, not turned, to stand in the middle of what the panels
    // leave free.
    let aspect = camera
        .logical_viewport_size()
        .map_or(1.6, |size| size.x / size.y.max(1.0));
    let half_width = space.distance * std::f32::consts::FRAC_PI_8.tan() * aspect;
    let shift = looking.right() * ((space.covered - space.left) * half_width);
    *transform = looking.with_translation(looking.translation + shift);
    let eye = transform.translation;
    let right = *looking.right();
    let up = *looking.up();
    let on_screen = |p: Vec3| camera.world_to_viewport(global, p).ok();
    space.on_screen = planets_at
        .iter()
        .map(|(centre, size, reach)| {
            let middle = on_screen(*centre)?;
            Some(OnScreen {
                centre: middle,
                radius: middle.distance(on_screen(*centre + right * *size)?),
                reach: middle.distance(on_screen(*centre + right * *reach)?),
                label: on_screen(*centre + up * (size + 0.35))?,
                depth: eye.distance(*centre),
            })
        })
        .collect();
    space.columns_on_screen = moons_at
        .into_iter()
        .filter_map(|(column, at)| Some((column, on_screen(at)?)))
        .collect();
}

/// A column's name beside its moon, in `size`: its mark, its name, its
/// type; from `at` rightwards, or, `leftwards`, up to it.
fn column_label(
    painter: &egui::Painter,
    at: egui::Pos2,
    leftwards: bool,
    column: &Column,
    size: f32,
) {
    let (mark, mark_colour) = if column.primary {
        ("◆", YELLOW)
    } else if column.references.is_some() {
        ("→", CYAN)
    } else {
        ("·", DIM)
    };
    let kind = database_view::short_type(&column.kind);
    let kind = if kind.is_empty() {
        String::new()
    } else {
        format!("{kind}{}", if column.nullable { "?" } else { "" })
    };
    let name = TEXT.gamma_multiply(if column.nullable { 0.75 } else { 1.0 });
    let parts: [(&str, FontId, Color32); 3] = [
        (mark, FontId::monospace(size), mark_colour),
        (&column.name, FontId::monospace(size), name),
        (&kind, FontId::monospace(size * 0.9), DIM),
    ];
    let widths: Vec<f32> = parts
        .iter()
        .map(|(text, font, colour)| {
            painter
                .layout_no_wrap((*text).to_owned(), font.clone(), *colour)
                .size()
                .x
        })
        .collect();
    let width = widths.iter().sum::<f32>() + 5.0 * (parts.len() - 1) as f32;
    let mut x = if leftwards { at.x - width } else { at.x };
    for ((text, font, colour), width) in parts.into_iter().zip(widths) {
        shadowed(
            painter,
            egui::pos2(x, at.y),
            Align2::LEFT_CENTER,
            text,
            font,
            colour,
        );
        x += width + 5.0;
    }
}

/// The database in space, drawn over its planets in `rect` under the title
/// the view draws: the mouse for the camera, the tables' names, the columns
/// of the table pointed at, the orbits. Returns whether a model was opened,
/// which leaves the view.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut Ui,
    rect: Rect,
    response: &Response,
    space: &mut DatabaseSpace,
    view: &mut DatabaseView,
    schema: &Arc<Schema>,
    kept: &[bool],
    app: &mut App,
) -> bool {
    space.take(schema);
    if response.dragged() {
        let delta = response.drag_delta();
        space.drag += Vec2::new(delta.x, delta.y);
    }
    if response.hovered() {
        space.zoom += ui.input(|i| i.smooth_scroll_delta.y);
    }
    let hovered = response
        .hover_pos()
        .and_then(|at| pick(&space.on_screen, Vec2::new(at.x, at.y)));
    if response.double_clicked()
        && let Some(t) = hovered
    {
        let table = &schema.tables[t];
        app.show_map(None);
        app.open_path_at(PathBuf::from(&table.path), table.line);
        return true;
    }
    if response.clicked() {
        view.chosen = hovered;
    }
    space.light(kept, hovered.or(view.chosen), view.chosen);
    space.left = if view.chosen.is_some() {
        (DETAILS_WIDTH + 24.0) / rect.width().max(1.0)
    } else {
        0.0
    };

    let painter = ui.painter_at(rect);
    let canvas = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 92.0),
        rect.max,
    ));
    let pos = |v: Vec2| egui::pos2(v.x, v.y);

    // The tables' names, above their planets: that of the table pointed at
    // and those it is joined to first, then the nearest; a name is left out
    // where it would cover another, or its planet is too small to read.
    let focus = space.focus;
    let related =
        |t: usize| focus.is_some_and(|f| space.orbits.as_ref().is_some_and(|o| o.joined(f, t)));
    let few = schema.tables.len() <= 40;
    let mut order: Vec<(usize, OnScreen)> = space
        .on_screen
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
        let name = &schema.tables[t].name;
        let galley = painter.layout_no_wrap(name.clone(), font.clone(), colour);
        let area = Rect::from_center_size(
            pos(s.label) - egui::vec2(0.0, galley.size().y / 2.0),
            galley.size() + egui::vec2(8.0, 2.0),
        );
        if !picked && taken.iter().any(|r| r.intersects(area)) {
            continue;
        }
        taken.push(area);
        shadowed(
            &canvas,
            pos(s.label),
            Align2::CENTER_BOTTOM,
            name,
            font,
            colour,
        );
    }

    // The columns of the table pointed at, once its rings are large enough
    // to tell their moons apart: those on the planet's right in a column
    // right of its rings, the others left of them, each tied to its moon by
    // a line.
    if let Some(f) = focus
        && let Some(table) = schema.tables.get(f)
        && let Some(planet) = space.on_screen.get(f).copied().flatten()
        && planet.reach >= 24.0
    {
        let size = 12.0;
        for leftwards in [false, true] {
            let mut side: Vec<(Option<usize>, Vec2)> = space
                .columns_on_screen
                .iter()
                .copied()
                .filter(|(_, at)| (at.x < planet.centre.x) == leftwards)
                .collect();
            side.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));
            let ys: Vec<f32> = side.iter().map(|(_, at)| at.y).collect();
            let edge = planet.reach + 28.0;
            let x = if leftwards {
                planet.centre.x - edge
            } else {
                planet.centre.x + edge
            };
            for ((column, moon), y) in side.into_iter().zip(spread(&ys, size + 3.0)) {
                let at = egui::pos2(x, y);
                let end = at + egui::vec2(if leftwards { 4.0 } else { -4.0 }, 0.0);
                canvas.line_segment([pos(moon), end], Stroke::new(1.0, DIM.gamma_multiply(0.6)));
                match column.and_then(|c| table.columns.get(c)) {
                    Some(column) => column_label(&canvas, at, leftwards, column, size),
                    None => shadowed(
                        &canvas,
                        at,
                        if leftwards {
                            Align2::RIGHT_CENTER
                        } else {
                            Align2::LEFT_CENTER
                        },
                        &format!("and {} more", table.columns.len().saturating_sub(MAX_MOONS)),
                        FontId::proportional(size),
                        DIM,
                    ),
                }
            }
        }
    }

    shadowed(
        &painter,
        rect.left_bottom() + egui::vec2(24.0, -18.0),
        Align2::LEFT_BOTTOM,
        "drag to turn · scroll to come closer · point at a table for its keys and columns · click it to go to it · double-click to open its model · the tables that point at none at the heart, each other outside what it points at · ◆ primary key · → foreign key",
        FontId::proportional(12.0),
        DIM,
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(text: &str) -> Schema {
        universe_codemap::schema(&[("models.py".to_owned(), text.to_owned())])
    }

    fn settled(schema: &Schema) -> Orbits {
        let mut orbits = Orbits::new(schema);
        for _ in 0..2_000 {
            if orbits.settled() {
                break;
            }
            orbits.step();
        }
        assert!(orbits.settled());
        orbits
    }

    const SHOP: &str = r#"
class Customer(Base):
    __tablename__ = "customer"
    id = Column(Integer, primary_key=True)

class Product(Base):
    __tablename__ = "product"
    id = Column(Integer, primary_key=True)

class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True)
    customer_id = Column(Integer, ForeignKey("customer.id"))

class Line(Base):
    __tablename__ = "line"
    id = Column(Integer, primary_key=True)
    order_id = Column(Integer, ForeignKey("orders.id"))
    product_id = Column(Integer, ForeignKey("product.id"))

class Review(Base):
    __tablename__ = "review"
    id = Column(Integer, primary_key=True)
    customer_id = Column(Integer, ForeignKey("customer.id"))
    product_id = Column(Integer, ForeignKey("product.id"))

class Log(Base):
    __tablename__ = "log"
    id = Column(Integer, primary_key=True)

class Setting(Base):
    __tablename__ = "setting"
    id = Column(Integer, primary_key=True)
"#;

    #[test]
    fn a_table_turns_outside_what_it_points_at_and_no_planet_stands_in_another() {
        let schema = schema(SHOP);
        let orbits = settled(&schema);
        let from_middle = |t: usize| orbits.centre(t).length();
        for key in schema.foreign_keys() {
            assert!(
                from_middle(key.from) > from_middle(key.to),
                "{} outside {}",
                schema.tables[key.from].name,
                schema.tables[key.to].name
            );
        }
        for a in 0..orbits.len() {
            for b in a + 1..orbits.len() {
                assert!(
                    orbits.centre(a).distance(orbits.centre(b))
                        >= orbits.reach[a] + orbits.reach[b],
                    "{} and {} stand in each other",
                    schema.tables[a].name,
                    schema.tables[b].name
                );
            }
        }
        // Spread in all three dimensions, not on a plane.
        let height = |t: usize| orbits.centre(t).y.abs();
        assert!((0..orbits.len()).any(|t| height(t) > 1.0));
        // Pointed at by more tables, a planet is larger.
        let size = |name: &str| orbits.size[schema.table(name).unwrap()];
        assert!(size("customer") > size("log"));
        // A table is nearer what it points at than a table it has nothing
        // to do with.
        let apart = |a: &str, b: &str| {
            let (a, b) = (schema.table(a).unwrap(), schema.table(b).unwrap());
            orbits.centre(a).distance(orbits.centre(b))
        };
        assert!(apart("orders", "customer") < apart("orders", "setting"));
    }

    #[test]
    fn the_planet_pointed_at_is_the_nearest_under_the_pointer_or_whose_rings_it_is_in() {
        let planet = |x: f32, depth: f32| {
            Some(OnScreen {
                centre: Vec2::new(x, 100.0),
                radius: 20.0,
                reach: 50.0,
                label: Vec2::new(x, 75.0),
                depth,
            })
        };
        let on_screen = [
            planet(100.0, 50.0),
            planet(110.0, 30.0),
            planet(400.0, 10.0),
        ];
        // On two planets: the nearer the camera.
        assert_eq!(pick(&on_screen, Vec2::new(105.0, 100.0)), Some(1));
        assert_eq!(pick(&on_screen, Vec2::new(85.0, 100.0)), Some(0));
        // In a planet's rings only: that planet.
        assert_eq!(pick(&on_screen, Vec2::new(360.0, 100.0)), Some(2));
        assert_eq!(pick(&on_screen, Vec2::new(250.0, 100.0)), None);
    }

    #[test]
    fn a_planet_rings_its_keys_inside_and_gives_one_moon_to_what_it_leaves_out() {
        let mut columns = String::new();
        for i in 0..20 {
            columns.push_str(&format!("    c{i} = Column(Integer)\n"));
        }
        let text = format!(
            "class Big(Base):\n    __tablename__ = \"big\"\n{columns}    id = Column(Integer, primary_key=True)\n"
        );
        let schema = schema(&text);
        let moons = moons(&schema.tables[0]);
        assert_eq!(moons.len(), MAX_MOONS + 1);
        // The primary key alone on the inner ring.
        assert_eq!(moons.iter().filter(|m| m.inner).count(), 1);
        assert!(moon_of(&moons, 20).is_some_and(|m| m.inner));
        // A column left out has the moon of those left out.
        assert_eq!(moon_of(&moons, 19).map(|m| m.column), Some(None));
        assert_eq!(moon_of(&moons, 3).map(|m| m.column), Some(Some(3)));
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
