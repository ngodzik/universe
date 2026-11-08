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
use std::f32::consts::{FRAC_PI_2, TAU};
use std::path::PathBuf;
use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, FontId, Rect, Response, Ui};
use universe_codemap::{Column, ForeignKey, Schema, Table};
use universe_ui::App;

use crate::database_view::{self, DETAILS_WIDTH, DatabaseView};
use crate::space::{self, Naming, OnScreen, Rig, Shells, hdr, tilt};
use crate::theme::{CYAN, DIM, TEXT, YELLOW};
use crate::universe::{PALETTE, Space, Universe, UniverseCamera};
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

/// The colours of the keys, as directions in HDR: primary keys gold,
/// foreign keys cyan, the moon of the columns left out grey.
const GOLD: [f32; 3] = [1.0, 0.7, 0.2];
const TEAL: [f32; 3] = [0.2, 0.8, 1.0];
const GREY: [f32; 3] = [0.4, 0.44, 0.55];
/// The colour of the arcs leaving the table pointed at: the 2D view's
/// accent, as a direction in HDR.
const ORANGE: [f32; 3] = [1.0, 0.45, 0.25];

/// Where the tables of `schema` stand: each in the shell of its layer by
/// its foreign keys, larger the more tables point at it, gathered with its
/// file's others.
pub(crate) fn shells(schema: &Schema) -> Shells {
    let count = schema.tables.len();
    let keys = schema.foreign_keys();
    let mut pointers: Vec<Vec<usize>> = vec![Vec::new(); count];
    for key in keys.iter().filter(|k| k.from != k.to) {
        if !pointers[key.to].contains(&key.from) {
            pointers[key.to].push(key.from);
        }
    }
    let size: Vec<f32> = pointers
        .iter()
        .map(|p| (SMALLEST + LARGER * (p.len() as f32).sqrt()).min(LARGEST))
        .collect();
    let reach = size.iter().map(|s| s + OUTER + MOON).collect();
    let mut paths: Vec<&str> = schema.tables.iter().map(|t| t.path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    let file = schema
        .tables
        .iter()
        .map(|t| paths.binary_search(&t.path.as_str()).unwrap_or(0))
        .collect();
    Shells::new(
        database_view::layers(schema),
        size,
        reach,
        file,
        keys.iter().map(|k| (k.from, k.to)),
    )
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
#[derive(Resource, Default)]
pub(crate) struct DatabaseSpace {
    schema: Option<Arc<Schema>>,
    keys: Vec<ForeignKey>,
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
    /// What the search keeps, the table pointed at or chosen, and the one
    /// chosen, for the camera.
    kept: Vec<bool>,
    focus: Option<usize>,
    chosen: Option<usize>,
    pub(crate) rig: Rig,
    /// Where each planet is on screen, and the moons of the one pointed at
    /// with their columns.
    on_screen: Vec<Option<OnScreen>>,
    columns_on_screen: Vec<(Option<usize>, Vec2)>,
}

impl DatabaseSpace {
    /// Takes `schema`, when another was read, and lays it out anew.
    fn take(&mut self, schema: &Arc<Schema>) {
        if self.schema.as_ref().is_some_and(|s| Arc::ptr_eq(s, schema)) {
            return;
        }
        self.moons = schema.tables.iter().map(moons).collect();
        self.shells = Some(shells(schema));
        self.keys = schema.foreign_keys();
        self.schema = Some(Arc::clone(schema));
        self.respawn = true;
        self.kept.clear();
        self.focus = None;
        self.chosen = None;
        self.rig.reframe();
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
        let file = self.shells.as_ref().map_or(0, |s| s.group(piece.table()));
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
    fn moon_offset(&self, shells: &Shells, t: usize, moon: &MoonAt) -> Vec3 {
        let radius = shells.size(t) + if moon.inner { INNER } else { OUTER };
        let local = Vec3::new(moon.angle.cos(), 0.0, moon.angle.sin()) * radius;
        self.ring_rotation(t, moon.inner) * local
    }

    /// Where the moon of `column` of planet `t` is, that of the columns left
    /// out when it is one; on the planet's surface facing `toward` when no
    /// column is said or none has a moon.
    fn moon_at(&self, shells: &Shells, t: usize, column: Option<usize>, toward: Vec3) -> Vec3 {
        let centre = shells.centre(t);
        match column.and_then(|c| moon_of(&self.moons[t], c)) {
            Some(moon) => centre + self.moon_offset(shells, t, moon),
            None => {
                let way = (toward - centre).try_normalize().unwrap_or(Vec3::Y);
                centre + way * shells.size(t)
            }
        }
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
    mut sphere: Local<Option<Handle<Mesh>>>,
) {
    let wanted = universe.space == Some(Space::Database) && space.shells.is_some();
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
        InSchema,
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
/// shades, the keys, the camera, and where the names go.
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
    if universe.space != Some(Space::Database) || !space.spawned {
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
    // its column's name.
    if space.focus.is_none() {
        space.spin += dt;
    }
    for (ring, mut transform) in &mut rings {
        transform.rotation = space.ring_rotation(ring.table, ring.inner);
    }
    if std::mem::take(&mut space.reshade) {
        let count = space.shells.as_ref().map_or(0, Shells::len);
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
    let Some(shells) = space.shells.clone() else {
        return;
    };

    // The rings of the table pointed at, the paths its moons turn on.
    if let Some(f) = space.focus.filter(|f| *f < shells.len()) {
        let around = Isometry3d::new(shells.centre(f), tilt(f) * Quat::from_rotation_x(FRAC_PI_2));
        let own = PALETTE[shells.group(f) % PALETTE.len()];
        for (out, colour) in [(INNER, GOLD), (OUTER, own)] {
            gizmos
                .circle(
                    around,
                    shells.size(f) + out,
                    Color::from(hdr(colour, 1.0, 0.35)),
                )
                .resolution(64);
        }
    }

    // The keys: from the moon of the column that points, bowing away from
    // the heart, in to the moon of the column pointed at. Those of the table
    // pointed at lit, a spark running along each from the table that points
    // to the one pointed at; the others faint.
    for (i, key) in space.keys.iter().enumerate() {
        let lit = |t: usize| space.kept.get(t).copied().unwrap_or(true);
        let (from, to) = (shells.centre(key.from), shells.centre(key.to));
        let a = space.moon_at(&shells, key.from, Some(key.from_column), to);
        let b = space.moon_at(&shells, key.to, key.to_column, from);
        let points = space::arc(a, b, from, shells.size(key.from), key.from == key.to);
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
            (space::bezier(points, t), Color::from(colour))
        }));
        if let Some(colour) = spark {
            let t = (t_now * 0.5 + i as f32 * 0.29).fract() * 0.94;
            gizmos.line(
                space::bezier(points, t),
                space::bezier(points, t + 0.06),
                Color::from(hdr(colour, 6.0, 1.0)),
            );
        }
    }

    // The camera: drifting while nothing is pointed at, turned by a drag,
    // gliding to frame the whole, or the table chosen.
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
    space.columns_on_screen = match space.focus.filter(|f| *f < shells.len()) {
        Some(f) => space.moons[f]
            .iter()
            .filter_map(|m| {
                let at = view.project(shells.centre(f) + space.moon_offset(&shells, f, m))?;
                Some((m.column, at))
            })
            .collect(),
        None => Vec::new(),
    };
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
    space::parts(
        painter,
        at,
        leftwards,
        &[
            (mark, FontId::monospace(size), mark_colour),
            (&column.name, FontId::monospace(size), name),
            (&kind, FontId::monospace(size * 0.9), DIM),
        ],
    );
}

/// The database in space, drawn over its planets in `rect` under the title
/// the view draws: the mouse for the camera, the tables' names, the columns
/// of the table pointed at. Returns whether a model was opened, which
/// leaves the view.
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
    space.rig.follow(ui, response);
    let hovered = response
        .hover_pos()
        .and_then(|at| space::pick(&space.on_screen, Vec2::new(at.x, at.y)));
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
    space.rig.left = if view.chosen.is_some() {
        (DETAILS_WIDTH + 24.0) / rect.width().max(1.0)
    } else {
        0.0
    };

    let painter = ui.painter_at(rect);
    let canvas = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 92.0),
        rect.max,
    ));
    let focus = space.focus;
    let related =
        |t: usize| focus.is_some_and(|f| space.shells.as_ref().is_some_and(|s| s.joined(f, t)));
    space::names(
        &painter,
        &canvas,
        &space.on_screen,
        |t| schema.tables[t].name.as_str(),
        &Naming {
            focus,
            related: &related,
            kept,
            few: schema.tables.len() <= 40,
        },
    );

    // The columns of the table pointed at, on both sides of its rings.
    if let Some(f) = focus
        && let Some(table) = schema.tables.get(f)
        && let Some(planet) = space.on_screen.get(f).copied().flatten()
    {
        let size = 12.0;
        space::beside(
            &canvas,
            planet,
            &space.columns_on_screen,
            size + 3.0,
            // The details cover the left.
            view.chosen.is_some().then_some(false),
            |painter, at, leftwards, column| match column.and_then(|c| table.columns.get(c)) {
                Some(column) => column_label(painter, at, leftwards, column, size),
                None => shadowed(
                    painter,
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
            },
        );
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
        let mut shells = shells(&schema);
        for _ in 0..2_000 {
            if shells.settled() {
                break;
            }
            shells.step();
        }
        assert!(shells.settled());
        let from_middle = |t: usize| shells.centre(t).length();
        for key in schema.foreign_keys() {
            assert!(
                from_middle(key.from) > from_middle(key.to),
                "{} outside {}",
                schema.tables[key.from].name,
                schema.tables[key.to].name
            );
        }
        for a in 0..shells.len() {
            for b in a + 1..shells.len() {
                assert!(
                    shells.centre(a).distance(shells.centre(b))
                        >= shells.reach(a) + shells.reach(b),
                    "{} and {} stand in each other",
                    schema.tables[a].name,
                    schema.tables[b].name
                );
            }
        }
        // Pointed at by more tables, a planet is larger.
        let size = |name: &str| shells.size(schema.table(name).unwrap());
        assert!(size("customer") > size("log"));
        // A table is nearer what it points at than a table it has nothing
        // to do with.
        let apart = |a: &str, b: &str| {
            let (a, b) = (schema.table(a).unwrap(), schema.table(b).unwrap());
            shells.centre(a).distance(shells.centre(b))
        };
        assert!(apart("orders", "customer") < apart("orders", "setting"));
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
}
