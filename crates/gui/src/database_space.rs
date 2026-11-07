//! The database in space, as the database view first shows it: each table
//! a tower of plates, one per column, its keys at the base, wider the more
//! tables point at it; the towers on floors by their foreign keys, those
//! that point at no table on the lowest, each above what it points at and
//! near it, and near its file's other tables, which share its colour; each
//! foreign key an arc from the plate of its column down to the one it
//! points at. The scene shares the universe's camera and sky: the camera
//! drifts, turns on a drag and comes closer with the wheel.
//!
//! A table pointed at lights its keys, the tables they join and its
//! columns' names; a click glides to it and shows it whole, a double click
//! opens its model.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, FontId, Rect, Response, Stroke, Ui};
use universe_codemap::{ForeignKey, Schema};
use universe_ui::App;

use crate::database_view::{self, DETAILS_WIDTH, DatabaseView};
use crate::theme::{CYAN, DIM, TEXT, YELLOW};
use crate::universe::{PALETTE, Universe, UniverseCamera};
use crate::universe_view::shadowed;

/// The most columns a tower stacks; the rest make one plate more, its cap.
const MAX_PLATES: usize = 16;

/// A plate's thickness, and the room between two.
const PLATE: f32 = 0.2;
const PLATE_GAP: f32 = 0.08;

/// The height from one floor to the next: more than the tallest tower, for
/// the keys between them to show.
const FLOOR: f32 = 7.5;

/// A tower's width: the narrowest, how much wider the tables pointing at it
/// make it, and the widest.
const NARROWEST: f32 = 1.4;
const WIDER: f32 = 0.3;
const WIDEST: f32 = 3.2;

/// The room kept between two towers on a floor.
const ROOM: f32 = 1.3;

/// How the towers find their places: how hard towers push each other away,
/// a foreign key pulls its tables together, a file gathers its tables, and
/// the middle holds them all.
const PUSH: f32 = 0.35;
const JOIN: f32 = 0.06;
const KIN: f32 = 0.025;
const GRAVITY: f32 = 0.008;

/// How far a tower may move in a step at first, how much less each step,
/// and below what it has settled.
const HEAT: f32 = 1.5;
const COOLING: f32 = 0.985;
const SETTLED: f32 = 0.01;

/// The pairs of towers a frame's steps may weigh: fewer steps a frame for a
/// larger schema, for the window to stay smooth while it settles.
const PAIRS_PER_FRAME: usize = 240_000;

/// The camera's nearest, and the farthest it frames the whole from.
const NEAREST: f32 = 4.0;
const FARTHEST: f32 = 420.0;

/// The colours of the keys, as directions in HDR: primary keys gold,
/// foreign keys cyan, the cap of the columns left out grey.
const GOLD: [f32; 3] = [1.0, 0.7, 0.2];
const TEAL: [f32; 3] = [0.2, 0.8, 1.0];
const GREY: [f32; 3] = [0.4, 0.44, 0.55];
/// The colour of the arcs leaving the table pointed at: the 2D view's
/// accent, as a direction in HDR.
const ORANGE: [f32; 3] = [1.0, 0.45, 0.25];

/// Where the towers stand: each on the floor of its layer, and on it near
/// the tables it is joined to by a foreign key and its file's others.
#[derive(Debug, Clone)]
pub(crate) struct Towers {
    /// Each tower's floor, from the lowest.
    floor: Vec<usize>,
    /// Where each stands on its floor.
    spots: Vec<Vec2>,
    /// Each one's width and its number of plates.
    width: Vec<f32>,
    rows: Vec<usize>,
    /// Each one's file, by its place among the files.
    file: Vec<usize>,
    files: usize,
    /// The pairs of tables a foreign key joins, once each, the lower
    /// first, a table never with itself.
    joins: Vec<(usize, usize)>,
    /// How far a tower may move in the next step.
    heat: f32,
}

/// The `i`th of `n` points spread evenly over a disc of radius 1.
fn sunflower(i: usize, n: usize) -> Vec2 {
    let r = ((i as f32 + 0.5) / n.max(1) as f32).sqrt();
    Vec2::from_angle(i as f32 * 2.399_963) * r
}

/// The height of a stack of `rows` plates.
fn stack(rows: usize) -> f32 {
    rows as f32 * (PLATE + PLATE_GAP) - PLATE_GAP
}

/// The middle of plate `row`, from the base of its tower.
fn plate_y(row: usize) -> f32 {
    row as f32 * (PLATE + PLATE_GAP) + PLATE * 0.5
}

impl Towers {
    /// The towers of `schema`, each stacking its `plates`, in their first
    /// places: each file's tables together, the files around the middle.
    pub(crate) fn new(schema: &Schema, plates: &[Vec<usize>]) -> Self {
        let count = schema.tables.len();
        let keys = schema.foreign_keys();
        let mut joins: Vec<(usize, usize)> = Vec::new();
        let mut pointers: Vec<Vec<usize>> = vec![Vec::new(); count];
        for key in keys.iter().filter(|k| k.from != k.to) {
            let pair = (key.from.min(key.to), key.from.max(key.to));
            if !joins.contains(&pair) {
                joins.push(pair);
            }
            if !pointers[key.to].contains(&key.from) {
                pointers[key.to].push(key.from);
            }
        }
        let width = pointers
            .iter()
            .map(|p| (NARROWEST + WIDER * (p.len() as f32).sqrt()).min(WIDEST))
            .collect();
        let rows = schema
            .tables
            .iter()
            .zip(plates)
            .map(|(table, plates)| {
                (plates.len() + usize::from(table.columns.len() > plates.len())).max(1)
            })
            .collect();
        let mut paths: Vec<&str> = schema.tables.iter().map(|t| t.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        let file: Vec<usize> = schema
            .tables
            .iter()
            .map(|t| paths.binary_search(&t.path.as_str()).unwrap_or(0))
            .collect();
        // Each file a patch of the disc, its tables spread within it.
        let spread = (count as f32).sqrt() * 2.6;
        let mut placed = vec![0_usize; paths.len()];
        let members: Vec<usize> = (0..paths.len())
            .map(|f| file.iter().filter(|g| **g == f).count())
            .collect();
        let spots = file
            .iter()
            .map(|&f| {
                let i = placed[f];
                placed[f] += 1;
                sunflower(f, paths.len()) * spread
                    + sunflower(i, members[f]) * (members[f] as f32).sqrt() * 1.8
            })
            .collect();
        Self {
            floor: database_view::layers(schema),
            spots,
            width,
            rows,
            file,
            files: paths.len(),
            joins,
            heat: HEAT,
        }
    }

    /// How many towers.
    fn len(&self) -> usize {
        self.spots.len()
    }

    /// Whether the towers have found their places.
    pub(crate) fn settled(&self) -> bool {
        self.heat < SETTLED
    }

    /// Moves each tower a step toward its place: away from the others, the
    /// more so on its own floor, where they would stand in each other;
    /// toward those it is joined to, the rest of its file, and the middle.
    pub(crate) fn step(&mut self) {
        let count = self.len();
        let mut push = vec![Vec2::ZERO; count];
        for a in 0..count {
            for b in a + 1..count {
                let apart = self.spots[a] - self.spots[b];
                let distance = apart.length();
                // Two towers on one spot part along a way of their own.
                let direction = if distance < 1e-3 {
                    Vec2::from_angle(a as f32 * 2.399_963 + b as f32)
                } else {
                    apart / distance
                };
                let distance = distance.max(0.05);
                let weight = match self.floor[a].abs_diff(self.floor[b]) {
                    0 => 1.0,
                    1 => 0.25,
                    _ => 0.06,
                };
                // Centre to centre, as far as two squares turned any way
                // need not to touch, and the room between them.
                let room = (self.width[a] + self.width[b]) * 0.71 + ROOM;
                let mut force = weight * PUSH * room * room / (distance * distance);
                if weight >= 1.0 && distance < room {
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
        let mut middles = vec![(Vec2::ZERO, 0_usize); self.files];
        for (spot, &file) in self.spots.iter().zip(&self.file) {
            middles[file].0 += *spot;
            middles[file].1 += 1;
        }
        for (t, push) in push.iter_mut().enumerate() {
            let (sum, members) = middles[self.file[t]];
            let middle = sum / members.max(1) as f32;
            *push += (middle - self.spots[t]) * KIN - self.spots[t] * GRAVITY;
        }
        for (spot, push) in self.spots.iter_mut().zip(push) {
            *spot += push.clamp_length_max(self.heat);
        }
        self.heat *= COOLING;
    }

    /// Where tower `t` stands: the middle of its base.
    pub(crate) fn base(&self, t: usize) -> Vec3 {
        let spot = self.spots[t];
        Vec3::new(spot.x, self.floor[t] as f32 * FLOOR, spot.y)
    }

    /// Tower `t`'s height.
    fn height(&self, t: usize) -> f32 {
        stack(self.rows[t])
    }

    /// Whether tables `a` and `b` are joined by a foreign key.
    fn joined(&self, a: usize, b: usize) -> bool {
        self.joins.contains(&(a.min(b), a.max(b)))
    }

    /// The floors, each with the middle of its towers' bases and how far
    /// from it the farthest of their edges is.
    fn floors(&self) -> Vec<(Vec3, f32)> {
        let floors = self.floor.iter().copied().max().map_or(0, |f| f + 1);
        (0..floors)
            .map(|f| {
                let members: Vec<usize> = (0..self.len()).filter(|t| self.floor[*t] == f).collect();
                let middle = members.iter().map(|t| self.spots[*t]).sum::<Vec2>()
                    / members.len().max(1) as f32;
                let reach = members
                    .iter()
                    .map(|t| self.spots[*t].distance(middle) + self.width[*t] * 0.71)
                    .fold(0.0_f32, f32::max);
                (
                    Vec3::new(middle.x, f as f32 * FLOOR, middle.y),
                    reach + ROOM,
                )
            })
            .collect()
    }

    /// The middle of the whole, and how far from it its farthest point is:
    /// the farthest edge of a floor's ring, or the top of its towers.
    fn extent(&self) -> (Vec3, f32) {
        let floors = self.floors();
        if floors.is_empty() {
            return (Vec3::ZERO, 0.0);
        }
        let tallest = (0..self.len()).map(|t| self.height(t)).fold(0.0, f32::max);
        let middle = floors.iter().map(|(m, _)| *m).sum::<Vec3>() / floors.len() as f32;
        let reach = floors
            .iter()
            .map(|(m, r)| {
                let across = Vec2::new(m.x - middle.x, m.z - middle.z).length() + r;
                let up = (m.y - middle.y).abs() + tallest * 0.5;
                across.hypot(up)
            })
            .fold(0.0_f32, f32::max);
        (middle, reach)
    }
}

/// What colours a plate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tone {
    Primary,
    Foreign,
    /// Another column, in its file's colour.
    File(usize),
    /// The cap of the columns left out.
    Rest,
}

/// How a tower is lit.
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

/// The material of a plate of `tone` shaded `shade`: its colour on the
/// faces the light reaches, and its own glow, the keys' brighter, to bloom.
fn material(tone: Tone, shade: Shade) -> StandardMaterial {
    let [r, g, b] = match tone {
        Tone::Primary => GOLD,
        Tone::Foreign => TEAL,
        Tone::File(file) => PALETTE[file % PALETTE.len()],
        Tone::Rest => GREY,
    };
    let key = matches!(tone, Tone::Primary | Tone::Foreign);
    let (base, glow) = match shade {
        Shade::Picked if key => (0.4, 3.4),
        Shade::Picked => (0.4, 0.9),
        Shade::Lit if key => (0.3, 1.6),
        Shade::Lit => (0.3, 0.32),
        Shade::Dim => (0.06, 0.05),
        Shade::Faded => (0.025, 0.015),
    };
    StandardMaterial {
        base_color: Color::linear_rgb(r * base, g * base, b * base),
        emissive: LinearRgba::rgb(r * glow, g * glow, b * glow),
        perceptual_roughness: 0.6,
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

/// Where a tower is on screen, in logical pixels: the middles of its base
/// and its top, half its width there, where its name goes, and how far it
/// is from the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OnScreen {
    base: Vec2,
    top: Vec2,
    half: f32,
    label: Vec2,
    depth: f32,
}

/// How far `at` is from the segment from `a` to `b`.
fn from_segment(at: Vec2, a: Vec2, b: Vec2) -> f32 {
    let along = b - a;
    let t = if along.length_squared() < 1e-6 {
        0.0
    } else {
        ((at - a).dot(along) / along.length_squared()).clamp(0.0, 1.0)
    };
    at.distance(a + along * t)
}

/// The tower under `at`: of those it falls on, the nearest the camera.
fn pick(on_screen: &[Option<OnScreen>], at: Vec2) -> Option<usize> {
    on_screen
        .iter()
        .enumerate()
        .filter_map(|(t, s)| {
            let s = s.as_ref()?;
            (from_segment(at, s.base, s.top) <= s.half + 3.0).then_some((t, s.depth))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t)
}

/// Marks what belongs to the database's scene.
#[derive(Component)]
pub(crate) struct InSchema;

/// A tower, by its table's place in the schema.
#[derive(Component)]
pub(crate) struct Tower(usize);

/// A plate: its table, and its column, `None` for the cap of those left
/// out.
#[derive(Component)]
pub(crate) struct Plate {
    table: usize,
    column: Option<usize>,
}

/// The database in space, as the panels and the systems share it.
#[derive(Resource)]
pub(crate) struct DatabaseSpace {
    schema: Option<Arc<Schema>>,
    keys: Vec<ForeignKey>,
    towers: Option<Towers>,
    /// The columns each tower stacks, from its base.
    plates: Vec<Vec<usize>>,
    /// Whether the scene stands, and must stand anew.
    spawned: bool,
    respawn: bool,
    /// Each tower's entity, in the schema's order.
    entities: Vec<Entity>,
    /// The materials made so far, shared by the plates of a tone and shade.
    materials: HashMap<(Tone, Shade), Handle<StandardMaterial>>,
    /// The plates must take their shades again.
    reshade: bool,
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
    /// Where each tower is on screen, the names of the columns of the one
    /// pointed at with their plates' height there, and each floor's name.
    on_screen: Vec<Option<OnScreen>>,
    columns_on_screen: Vec<(Option<usize>, Vec2, f32)>,
    floors_on_screen: Vec<Option<Vec2>>,
}

impl Default for DatabaseSpace {
    fn default() -> Self {
        Self {
            schema: None,
            keys: Vec::new(),
            towers: None,
            plates: Vec::new(),
            spawned: false,
            respawn: false,
            entities: Vec::new(),
            materials: HashMap::new(),
            reshade: false,
            kept: Vec::new(),
            focus: None,
            chosen: None,
            centre: Vec3::ZERO,
            yaw: 0.7,
            pitch: 0.42,
            distance: 260.0,
            target_distance: 260.0,
            zoomed: false,
            drag: Vec2::ZERO,
            zoom: 0.0,
            covered: 0.0,
            left: 0.0,
            on_screen: Vec::new(),
            columns_on_screen: Vec::new(),
            floors_on_screen: Vec::new(),
        }
    }
}

impl DatabaseSpace {
    /// Takes `schema`, when another was read, and lays it out anew.
    fn take(&mut self, schema: &Arc<Schema>) {
        if self.schema.as_ref().is_some_and(|s| Arc::ptr_eq(s, schema)) {
            return;
        }
        self.plates = schema
            .tables
            .iter()
            .map(|t| database_view::listed(t, MAX_PLATES))
            .collect();
        self.towers = Some(Towers::new(schema, &self.plates));
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

    /// How tower `t` is lit.
    fn shade(&self, t: usize) -> Shade {
        if !self.kept.get(t).copied().unwrap_or(true) {
            return Shade::Faded;
        }
        match (self.focus, &self.towers) {
            (Some(f), _) if f == t => Shade::Picked,
            (Some(f), Some(towers)) if towers.joined(f, t) => Shade::Lit,
            (Some(_), _) => Shade::Dim,
            (None, _) => Shade::Lit,
        }
    }

    /// What colours `plate`.
    fn tone(&self, plate: &Plate) -> Tone {
        let column = plate.column.and_then(|c| {
            let table = self.schema.as_ref()?.tables.get(plate.table)?;
            table.columns.get(c)
        });
        match column {
            None => Tone::Rest,
            Some(c) if c.primary => Tone::Primary,
            Some(c) if c.references.is_some() => Tone::Foreign,
            Some(_) => Tone::File(self.towers.as_ref().map_or(0, |t| t.file[plate.table])),
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

    /// The plate of `column` in tower `t`, the cap when it is left out, the
    /// base when none is said.
    fn row(&self, t: usize, column: Option<usize>) -> usize {
        let Some(column) = column else {
            return 0;
        };
        let plates = &self.plates[t];
        plates
            .iter()
            .position(|c| *c == column)
            .unwrap_or(plates.len())
    }
}

/// Puts up the scene when shown and laid out, takes it down when left.
pub(crate) fn spawn(
    mut commands: Commands,
    mut space: ResMut<DatabaseSpace>,
    universe: Res<Universe>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<InSchema>>,
    mut cube: Local<Option<Handle<Mesh>>>,
) {
    let wanted = universe.schema && space.towers.is_some();
    if (!wanted || space.respawn) && space.spawned {
        for entity in &existing {
            commands.entity(entity).despawn();
        }
        space.entities.clear();
        space.spawned = false;
    }
    if !wanted || space.spawned {
        return;
    }
    let (Some(towers), Some(schema)) = (space.towers.clone(), space.schema.clone()) else {
        return;
    };
    space.respawn = false;
    space.spawned = true;
    space.reshade = true;
    let cube = cube
        .get_or_insert_with(|| meshes.add(Cuboid::new(1.0, 1.0, 1.0)))
        .clone();
    // Lit from above and aside, for the faces of a plate to read apart.
    commands.spawn((
        DirectionalLight {
            illuminance: 3_000.0,
            ..default()
        },
        Transform::from_xyz(40.0, 90.0, 25.0).looking_at(Vec3::ZERO, Vec3::Y),
        InSchema,
    ));
    let lit = space.material(Tone::Rest, Shade::Lit, &mut assets);
    let mut entities = Vec::with_capacity(towers.len());
    for (t, table) in schema.tables.iter().enumerate() {
        let width = towers.width[t];
        let mut columns: Vec<Option<usize>> = space.plates[t].iter().copied().map(Some).collect();
        if table.columns.len() > columns.len() || columns.is_empty() {
            columns.push(None);
        }
        let entity = commands
            .spawn((
                Transform::from_translation(towers.base(t)),
                Visibility::Visible,
                Tower(t),
                InSchema,
            ))
            .with_children(|tower| {
                for (row, column) in columns.into_iter().enumerate() {
                    tower.spawn((
                        Mesh3d(cube.clone()),
                        MeshMaterial3d(lit.clone()),
                        Transform::from_xyz(0.0, plate_y(row), 0.0)
                            .with_scale(Vec3::new(width, PLATE, width)),
                        Plate { table: t, column },
                    ));
                }
            })
            .id();
        entities.push(entity);
    }
    space.entities = entities;
}

/// What the camera's query reads and moves.
type CameraParts = (
    &'static mut Transform,
    &'static Camera,
    &'static GlobalTransform,
);

/// Each frame while shown: the towers' next steps, their shades, the floors
/// and the keys, the camera, and where the names go.
#[allow(clippy::too_many_arguments)]
pub(crate) fn animate(
    mut space: ResMut<DatabaseSpace>,
    universe: Res<Universe>,
    time: Res<Time>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut towers: Query<(&Tower, &mut Transform), Without<UniverseCamera>>,
    mut plates: Query<(&Plate, &mut MeshMaterial3d<StandardMaterial>)>,
    mut camera: Query<CameraParts, (With<UniverseCamera>, Without<Tower>)>,
    mut gizmos: Gizmos,
) {
    if !universe.schema || !space.spawned {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let t_now = time.elapsed_secs_wrapped();
    if let Some(layout) = space.towers.as_mut()
        && !layout.settled()
    {
        let pairs = (layout.len() * layout.len() / 2).max(1);
        for _ in 0..(PAIRS_PER_FRAME / pairs).clamp(1, 4) {
            layout.step();
        }
        for (tower, mut transform) in &mut towers {
            transform.translation = layout.base(tower.0);
        }
    }
    if std::mem::take(&mut space.reshade) {
        let count = space.towers.as_ref().map_or(0, Towers::len);
        let shades: Vec<Shade> = (0..count).map(|t| space.shade(t)).collect();
        for (plate, mut material) in &mut plates {
            let tone = space.tone(plate);
            let shade = shades.get(plate.table).copied().unwrap_or(Shade::Lit);
            let handle = space.material(tone, shade, &mut assets);
            if material.0 != handle {
                material.0 = handle;
            }
        }
    }
    let Some(layout) = space.towers.as_ref() else {
        return;
    };

    // The floors, faint rings under their towers.
    let floors = layout.floors();
    for (middle, reach) in &floors {
        gizmos
            .circle(
                Isometry3d::new(
                    *middle - Vec3::Y * 0.05,
                    Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                ),
                *reach,
                Color::from(LinearRgba::new(0.35, 0.4, 0.8, 0.22)),
            )
            .resolution(96);
    }

    // The keys: from the plate of the column that points, out of its side
    // facing what it points at, down to the plate pointed at. Those of the
    // table pointed at lit, a spark running along each from the table that
    // points to the one pointed at; the others faint.
    for (i, key) in space.keys.iter().enumerate() {
        let lit = |t: usize| space.kept.get(t).copied().unwrap_or(true);
        let a =
            layout.base(key.from) + Vec3::Y * plate_y(space.row(key.from, Some(key.from_column)));
        let b = layout.base(key.to) + Vec3::Y * plate_y(space.row(key.to, key.to_column));
        let points = if key.from == key.to {
            // A table pointing at itself: a loop out of its side.
            let side = Vec3::X * (layout.width[key.from] * 0.5);
            let out = Vec3::X * 1.8;
            [a + side, a + side + out, b + side + out, b + side]
        } else {
            let flat = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
            let way = flat.try_normalize().unwrap_or(Vec3::X);
            let start = a + way * (layout.width[key.from] * 0.5);
            let end = b - way * (layout.width[key.to] * 0.5);
            let reach = (flat.length() * 0.4).max(1.5);
            [start, start + way * reach, end - way * reach, end]
        };
        let (from, to, alpha, spark) = match space.focus {
            Some(f) if f == key.from => (ORANGE, ORANGE, 0.95, Some(ORANGE)),
            Some(f) if f == key.to => (TEAL, TEAL, 0.95, Some(TEAL)),
            Some(_) => (TEAL, GOLD, 0.05, None),
            None if lit(key.from) && lit(key.to) => (TEAL, GOLD, 0.4, None),
            None => (TEAL, GOLD, 0.05, None),
        };
        let bright = if spark.is_some() { 2.4 } else { 1.0 };
        gizmos.linestrip_gradient((0..=20).map(|s| {
            let t = s as f32 / 20.0;
            let colour = hdr(from, bright, alpha).mix(&hdr(to, bright, alpha), t);
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
    let (middle, extent) = layout.extent();
    let whole = (extent * 2.2).clamp(12.0, FARTHEST);
    let goal = match space.chosen.filter(|c| *c < layout.len()) {
        Some(c) => (
            layout.base(c) + Vec3::Y * layout.height(c) * 0.5,
            (layout.width[c] * 8.0).max(20.0),
        ),
        None => (middle, whole),
    };
    let base_of: Vec<(Vec3, f32, f32)> = (0..layout.len())
        .map(|t| (layout.base(t), layout.height(t), layout.width[t]))
        .collect();
    let floor_rings = floors;
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
    let on_screen = |p: Vec3| camera.world_to_viewport(global, p).ok();
    space.on_screen = base_of
        .iter()
        .map(|(base, height, width)| {
            let middle = *base + Vec3::Y * (height * 0.5);
            let centre = on_screen(middle)?;
            Some(OnScreen {
                base: on_screen(*base)?,
                top: on_screen(*base + Vec3::Y * *height)?,
                half: centre.distance(on_screen(middle + right * (width * 0.5))?),
                label: on_screen(*base + Vec3::Y * (height + 0.5))?,
                depth: eye.distance(middle),
            })
        })
        .collect();
    space.columns_on_screen = match space.focus.filter(|f| *f < base_of.len()) {
        Some(f) => {
            let (base, _, width) = base_of[f];
            let mut columns: Vec<Option<usize>> =
                space.plates[f].iter().copied().map(Some).collect();
            let more = space
                .schema
                .as_ref()
                .and_then(|s| s.tables.get(f))
                .is_some_and(|t| t.columns.len() > columns.len());
            if more {
                columns.push(None);
            }
            columns
                .into_iter()
                .enumerate()
                .filter_map(|(row, column)| {
                    // Clear of the tower's corner, however it is turned.
                    let at = base + Vec3::Y * plate_y(row) + right * (width * 0.71 + 0.15);
                    let here = on_screen(at)?;
                    let next = on_screen(at + Vec3::Y * (PLATE + PLATE_GAP))?;
                    Some((column, here, here.distance(next)))
                })
                .collect()
        }
        None => Vec::new(),
    };
    space.floors_on_screen = floor_rings
        .iter()
        .map(|(middle, reach)| on_screen(*middle + right * *reach))
        .collect();
}

/// The database in space, drawn over its towers in `rect` under the title
/// the view draws: the mouse for the camera, the tables' names, the columns
/// of the table pointed at, the floors. Returns whether a model was
/// opened, which leaves the view.
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

    // The floors' names, at their rings' right.
    for (floor, at) in space.floors_on_screen.iter().enumerate() {
        let Some(at) = at else { continue };
        let name = if floor == 0 {
            "floor 0 · points at no table".to_owned()
        } else {
            format!("floor {floor}")
        };
        shadowed(
            &canvas,
            pos(*at) + egui::vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            &name,
            FontId::proportional(11.5),
            DIM,
        );
    }

    // The tables' names, above their towers: that of the table pointed at
    // and those it is joined to first, then the nearest; a name is left out
    // where it would cover another, or its tower is too small to read.
    let focus = space.focus;
    let related =
        |t: usize| focus.is_some_and(|f| space.towers.as_ref().is_some_and(|l| l.joined(f, t)));
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
        let tall = s.base.distance(s.top) > 30.0;
        let lit = kept.get(t).copied().unwrap_or(true);
        if !(picked || related(t) || ((tall || few) && lit && focus.is_none())) {
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

    // The columns of the table pointed at, once their plates are far enough
    // apart to read: in a column of their own right of the tower, which
    // perspective slants, each tied to its plate by a line.
    if let Some(f) = focus
        && let Some(table) = schema.tables.get(f)
    {
        let spacing = space
            .columns_on_screen
            .iter()
            .map(|(_, _, h)| *h)
            .fold(f32::MAX, f32::min);
        let size = (spacing * 0.9).clamp(10.0, 13.0);
        let left = space
            .columns_on_screen
            .iter()
            .map(|(_, at, _)| at.x)
            .fold(f32::MIN, f32::max)
            + 14.0;
        for (column, edge, _) in space.columns_on_screen.iter().filter(|_| spacing >= 8.0) {
            let at = egui::pos2(left, edge.y);
            canvas.line_segment(
                [pos(*edge), at - egui::vec2(4.0, 0.0)],
                Stroke::new(1.0, DIM.gamma_multiply(0.6)),
            );
            let Some(column) = column.and_then(|c| table.columns.get(c)) else {
                shadowed(
                    &canvas,
                    at,
                    Align2::LEFT_CENTER,
                    &format!(
                        "and {} more",
                        table.columns.len().saturating_sub(MAX_PLATES)
                    ),
                    FontId::proportional(size),
                    DIM,
                );
                continue;
            };
            let (mark, mark_colour) = if column.primary {
                ("◆", YELLOW)
            } else if column.references.is_some() {
                ("→", CYAN)
            } else {
                ("·", DIM)
            };
            let glyph = size * 0.6;
            shadowed(
                &canvas,
                at,
                Align2::LEFT_CENTER,
                mark,
                FontId::monospace(size),
                mark_colour,
            );
            let name_at = egui::pos2(at.x + glyph + 5.0, at.y);
            shadowed(
                &canvas,
                name_at,
                Align2::LEFT_CENTER,
                &column.name,
                FontId::monospace(size),
                TEXT.gamma_multiply(if column.nullable { 0.75 } else { 1.0 }),
            );
            let kind = database_view::short_type(&column.kind);
            if !kind.is_empty() {
                shadowed(
                    &canvas,
                    egui::pos2(
                        name_at.x + column.name.chars().count() as f32 * glyph + 8.0,
                        at.y,
                    ),
                    Align2::LEFT_CENTER,
                    &format!("{kind}{}", if column.nullable { "?" } else { "" }),
                    FontId::monospace(size * 0.9),
                    DIM,
                );
            }
        }
    }

    shadowed(
        &painter,
        rect.left_bottom() + egui::vec2(24.0, -18.0),
        Align2::LEFT_BOTTOM,
        "drag to turn · scroll to come closer · point at a table for its keys and columns · click it to go to it · double-click to open its model · each floor points at those below · ◆ primary key · → foreign key",
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

    fn settled(schema: &Schema) -> Towers {
        let plates: Vec<Vec<usize>> = schema
            .tables
            .iter()
            .map(|t| database_view::listed(t, MAX_PLATES))
            .collect();
        let mut towers = Towers::new(schema, &plates);
        for _ in 0..2_000 {
            if towers.settled() {
                break;
            }
            towers.step();
        }
        assert!(towers.settled());
        towers
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
    fn a_table_stands_above_what_it_points_at_and_no_tower_stands_in_another() {
        let schema = schema(SHOP);
        let towers = settled(&schema);
        for key in schema.foreign_keys() {
            assert!(
                towers.base(key.from).y > towers.base(key.to).y,
                "{} above {}",
                schema.tables[key.from].name,
                schema.tables[key.to].name
            );
        }
        for a in 0..towers.len() {
            for b in a + 1..towers.len() {
                if towers.floor[a] != towers.floor[b] {
                    continue;
                }
                let apart = (towers.base(a) - towers.base(b)).abs();
                let room = (towers.width[a] + towers.width[b]) / 2.0;
                assert!(
                    apart.x >= room || apart.z >= room,
                    "{} and {} stand in each other",
                    schema.tables[a].name,
                    schema.tables[b].name
                );
            }
        }
        // Pointed at by more tables, a tower is wider.
        let width = |name: &str| towers.width[schema.table(name).unwrap()];
        assert!(width("customer") > width("log"));
        // A table stands nearer what it points at than a table it has
        // nothing to do with.
        let across = |a: &str, b: &str| {
            let (a, b) = (schema.table(a).unwrap(), schema.table(b).unwrap());
            let (a, b) = (towers.base(a), towers.base(b));
            Vec2::new(a.x - b.x, a.z - b.z).length()
        };
        assert!(across("orders", "customer") < across("orders", "setting"));
    }

    #[test]
    fn the_tower_pointed_at_is_the_nearest_under_the_pointer() {
        let tower = |x: f32, depth: f32| {
            Some(OnScreen {
                base: Vec2::new(x, 200.0),
                top: Vec2::new(x, 100.0),
                half: 20.0,
                label: Vec2::new(x, 90.0),
                depth,
            })
        };
        let on_screen = [tower(100.0, 50.0), tower(110.0, 30.0), tower(400.0, 10.0)];
        assert_eq!(pick(&on_screen, Vec2::new(105.0, 150.0)), Some(1));
        assert_eq!(pick(&on_screen, Vec2::new(85.0, 150.0)), Some(0));
        assert_eq!(pick(&on_screen, Vec2::new(250.0, 150.0)), None);
        // Past its top, a tower is not under the pointer.
        assert_eq!(pick(&on_screen, Vec2::new(400.0, 40.0)), None);
    }

    #[test]
    fn a_tower_stacks_its_keys_first_and_caps_what_it_leaves_out() {
        let mut columns = String::new();
        for i in 0..20 {
            columns.push_str(&format!("    c{i} = Column(Integer)\n"));
        }
        let text = format!(
            "class Big(Base):\n    __tablename__ = \"big\"\n{columns}    id = Column(Integer, primary_key=True)\n"
        );
        let schema = schema(&text);
        let mut space = DatabaseSpace::default();
        space.take(&Arc::new(schema));
        assert_eq!(space.plates[0].len(), MAX_PLATES);
        let towers = space.towers.as_ref().unwrap();
        assert_eq!(towers.rows[0], MAX_PLATES + 1);
        // The primary key at the base; a column left out on the cap.
        assert_eq!(space.row(0, Some(20)), 0);
        assert_eq!(space.row(0, Some(19)), MAX_PLATES);
        assert!(towers.height(0) < FLOOR);
    }
}
