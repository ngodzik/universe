//! The codebase as a universe: each file a star sized by its length, each
//! folder a dimmer one, the links between them what holds what, what
//! imports what and what calls what through the API. The stars spread out
//! from the centre as the layout finds their places.
//!
//! The stars gather into galaxies by what the person chose: by module at
//! first, or by language, layer or role, and by a second criterion into
//! clusters within each galaxy, which then gives the colours. A galaxy can
//! be entered: the others fade where they are, and its own stars spread
//! out, gathered again within it.
//!
//! The scene is drawn by a camera of its own, in HDR with bloom, under the
//! panels. It is only drawn while shown: hidden, the camera sleeps and the
//! window goes back to drawing only when something changes.

use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use universe_codemap::{
    CodeMap, Criterion, EdgeKind, Grouping, Key, Language, Layout, MAX_CRITERIA, NodeKind, Repos,
    Role,
};

/// The most files mapped: beyond, the layout would take too long to settle
/// and the sky too busy to read.
const MAX_FILES: usize = 1_200;

/// Layout steps run each frame while it settles: fast enough to watch the
/// stars find their places, slow enough to see them do it.
const STEPS_PER_FRAME: usize = 2;

/// How bright a star is at rest, in HDR units: above 1, it blooms.
const REST: f32 = 3.6;
/// How bright a star outside the galaxy entered is: below 1, it does not
/// bloom, and the galaxy stands out.
const FADED: f32 = 0.16;
/// How much smaller a star outside the galaxy entered is.
const FADED_SIZE: f32 = 0.55;

/// The camera's nearest to what it looks at: closer, a star would pass
/// through it and its glow fill the window.
const NEAREST: f32 = 3.5;

/// The far stars of the background, and how far: beyond the widest
/// universe the camera can back away from.
const SKY_STARS: usize = 1_400;
const SKY_NEAR: f32 = 900.0;
const SKY_DEPTH: f32 = 500.0;

/// The farthest the camera frames the whole from.
const FARTHEST: f32 = 320.0;

/// The colours of groups that have none of their own, as directions in
/// HDR: modules, or what a criterion leaves out.
const PALETTE: [[f32; 3]; 10] = [
    [1.0, 0.42, 0.18],
    [0.25, 0.6, 1.0],
    [0.3, 1.0, 0.45],
    [1.0, 0.3, 0.7],
    [1.0, 0.85, 0.25],
    [0.2, 0.95, 0.9],
    [0.65, 0.4, 1.0],
    [1.0, 0.25, 0.25],
    [0.7, 1.0, 0.2],
    [0.55, 0.8, 1.0],
];
const LEFT_OUT: LinearRgba = LinearRgba::rgb(0.5, 0.55, 0.65);
const FOLDER: LinearRgba = LinearRgba::rgb(0.55, 0.35, 1.0);

/// The layer the links are drawn on: seen by the universe's camera only, so
/// that the panels' camera does not draw them flattened.
const LINKS_LAYER: usize = 1;

/// How the universe is shown, read and written by the panels and by the
/// systems that draw it.
#[derive(Resource)]
pub(crate) struct Universe {
    /// Whether it shows, as the state says.
    pub(crate) shown: bool,
    /// The repositories read, and the name of the project or the system.
    repos: Repos,
    name: String,
    map: Option<CodeMap>,
    layout: Option<Layout>,
    /// The star of each node, in the map's order.
    stars: Vec<Entity>,
    /// The camera: around which point, from which angles, how far.
    focus: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// How far the camera is heading: it glides there.
    target_distance: f32,
    /// Whether the wheel set the distance since the view last framed
    /// something: it is then left alone.
    pub(crate) zoomed: bool,
    /// What the stars are grouped by, in order, as the person chose.
    criteria: Vec<Criterion>,
    /// The galaxies entered, outermost first, each with its name.
    entered: Vec<(Key, String)>,
    /// The stars gathered as the criteria and the focus say.
    grouping: Grouping,
    /// The colour of each galaxy, and of each cluster within it.
    galaxy_colours: Vec<LinearRgba>,
    cluster_colours: Vec<Vec<LinearRgba>>,
    /// What the colours mean: a name, a colour and how many files.
    legend: Legend,
    /// The stars must take their colours again.
    recolour: bool,
    /// What the stars are searched for, and whether the search finds each:
    /// a file by its path, a folder when it holds a file found. Empty when
    /// nothing is searched for.
    query: String,
    found: Vec<bool>,
    /// Where each galaxy's name goes on screen, in logical pixels.
    pub(crate) galaxies_on_screen: Vec<Option<Vec2>>,
    /// What the panels report of the mouse, for the next frame.
    pub(crate) drag: Vec2,
    pub(crate) zoom: f32,
    /// The star under the pointer, and the one chosen with a click.
    pub(crate) hovered: Option<usize>,
    pub(crate) chosen: Option<usize>,
    /// Where each node was on screen last frame, in logical pixels.
    pub(crate) on_screen: Vec<Option<Vec2>>,
    /// How much of the window's width the panels cover on the right: the
    /// view moves so that the universe is centred in what is left.
    pub(crate) covered: f32,
    /// Whether the window shows the deployments rather than the code: the
    /// stars rest, and the camera is the fleet's.
    pub(crate) deployments: bool,
}

impl Universe {
    pub(crate) fn new(repos: Repos, name: String) -> Self {
        Self {
            shown: false,
            repos,
            name,
            map: None,
            layout: None,
            stars: Vec::new(),
            focus: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.35,
            distance: 90.0,
            target_distance: 90.0,
            zoomed: false,
            drag: Vec2::ZERO,
            zoom: 0.0,
            criteria: vec![Criterion::Module],
            entered: Vec::new(),
            grouping: Grouping::default(),
            galaxy_colours: Vec::new(),
            cluster_colours: Vec::new(),
            legend: Vec::new(),
            recolour: false,
            query: String::new(),
            found: Vec::new(),
            galaxies_on_screen: Vec::new(),
            hovered: None,
            chosen: None,
            on_screen: Vec::new(),
            covered: 0.0,
            deployments: false,
        }
    }

    /// What the stars are grouped by, in order.
    pub(crate) fn criteria(&self) -> &[Criterion] {
        &self.criteria
    }

    /// Groups by `criterion` too, after those chosen, or no longer when it
    /// was: past the most criteria, it takes the last one's place.
    pub(crate) fn toggle(&mut self, criterion: Criterion) {
        if let Some(at) = self.criteria.iter().position(|c| *c == criterion) {
            self.criteria.remove(at);
        } else {
            self.criteria.truncate(MAX_CRITERIA - 1);
            self.criteria.push(criterion);
        }
        self.regroup();
    }

    /// The galaxies entered, outermost first, by name.
    pub(crate) fn path(&self) -> impl Iterator<Item = &str> {
        self.entered.iter().map(|(_, name)| name.as_str())
    }

    /// Enters the grouping's galaxy `group`, unless it is what a criterion
    /// leaves out, which has nothing to narrow to.
    pub(crate) fn enter(&mut self, group: usize) {
        if let Some(galaxy) = self.grouping.groups.get(group)
            && let Some(key) = galaxy.key.clone()
        {
            self.entered.push((key, galaxy.label.clone()));
            self.regroup();
        }
    }

    /// Narrows to the legend's entry `at`, in every galaxy: to the Rust
    /// files of each module, say.
    pub(crate) fn enter_legend(&mut self, at: usize) {
        if let Some(meaning) = self.legend.get(at)
            && let Some(key) = meaning.key.clone()
        {
            self.entered.push((key, meaning.name.clone()));
            self.regroup();
        }
    }

    /// Leaves the galaxies entered past the first `depth`.
    pub(crate) fn leave(&mut self, depth: usize) {
        if depth < self.entered.len() {
            self.entered.truncate(depth);
            self.regroup();
        }
    }

    /// The stars as they are gathered.
    pub(crate) fn grouping(&self) -> &Grouping {
        &self.grouping
    }

    /// Searches the stars for `query`, unless they already are.
    pub(crate) fn search(&mut self, query: &str) {
        if query == self.query {
            return;
        }
        query.clone_into(&mut self.query);
        self.found = match &self.map {
            Some(map) if !query.is_empty() => found(map, query),
            _ => Vec::new(),
        };
        self.recolour = true;
        self.chosen = None;
    }

    /// Whether the stars are searched for something.
    pub(crate) fn searching(&self) -> bool {
        !self.query.is_empty()
    }

    /// Whether a node's star is lit: inside the galaxy entered, and found
    /// by the search, if any.
    pub(crate) fn lit(&self, node: usize) -> bool {
        self.grouping.inside(node) && self.found.get(node).copied().unwrap_or(true)
    }

    /// The colour of galaxy `group`, for its name.
    pub(crate) fn galaxy_colour(&self, group: usize) -> [u8; 3] {
        srgb(self.galaxy_colours.get(group).copied().unwrap_or(LEFT_OUT))
    }

    /// The colour of a node's star, for its name.
    pub(crate) fn tint(&self, node: usize) -> [u8; 3] {
        srgb(self.colour(node))
    }

    /// What the colours mean: a name, a colour, how many files, and
    /// whether it can be narrowed to.
    pub(crate) fn legend(&self) -> impl Iterator<Item = (&str, [u8; 3], usize, bool)> {
        self.legend
            .iter()
            .map(|m| (m.name.as_str(), srgb(m.colour), m.files, m.key.is_some()))
    }

    /// The colour of a node, as a direction in HDR: brightness comes
    /// after. Folders keep theirs, to read as the frame of what they hold.
    fn colour(&self, node: usize) -> LinearRgba {
        let Some(map) = &self.map else {
            return LEFT_OUT;
        };
        match (map.nodes[node].kind, self.grouping.of(node)) {
            (NodeKind::Folder, _) => FOLDER,
            (NodeKind::File { language, .. }, None) if self.grouping.criteria.is_empty() => {
                language_colour(language)
            }
            (_, None) => LEFT_OUT,
            (_, Some((galaxy, None))) => self.galaxy_colours[galaxy],
            (_, Some((galaxy, Some(cluster)))) => self.cluster_colours[galaxy][cluster],
        }
    }

    /// How bright a node's star is at rest: faded when outside the galaxy
    /// entered.
    fn rest(&self, node: usize) -> f32 {
        if self.lit(node) { REST } else { FADED }
    }

    /// How large a node's star is at rest: smaller when outside the galaxy
    /// entered.
    fn scale(&self, node: usize) -> f32 {
        let Some(map) = &self.map else {
            return 0.0;
        };
        let size = size(&map.nodes[node]);
        if self.lit(node) {
            size
        } else {
            size * FADED_SIZE
        }
    }

    /// Gathers the stars again, as the criteria and the focus now say, and
    /// sends them to their new places.
    fn regroup(&mut self) {
        let Some(map) = &self.map else {
            return;
        };
        let keys: Vec<Key> = self.entered.iter().map(|(key, _)| key.clone()).collect();
        self.grouping = universe_codemap::group(map, &keys, &self.criteria);
        // The project's own package has no name of its own: it takes the
        // project's.
        let project = self.name.clone();
        for group in self
            .grouping
            .groups
            .iter_mut()
            .chain(self.grouping.subgroups.iter_mut().flatten())
            .filter(|g| g.label.is_empty())
        {
            group.label.clone_from(&project);
        }
        if let Some(layout) = &mut self.layout {
            layout.regroup(&self.grouping);
        }
        let (galaxies, clusters, legend) = colours(map, &self.grouping);
        self.galaxy_colours = galaxies;
        self.cluster_colours = clusters;
        self.legend = legend;
        self.galaxies_on_screen = vec![None; self.grouping.groups.len()];
        self.recolour = true;
        self.chosen = None;
        self.hovered = None;
        self.zoomed = false;
    }

    /// The map, once built.
    pub(crate) fn map(&self) -> Option<&CodeMap> {
        self.map.as_ref()
    }
}

/// Marks what belongs to the universe, to show or hide it all at once.
#[derive(Component)]
pub(crate) struct InUniverse;

/// A star of the code, to hide while the deployments show.
#[derive(Component)]
pub(crate) struct CodeStar;

/// The universe's camera.
#[derive(Component)]
pub(crate) struct UniverseCamera;

/// Shared meshes.
#[derive(Resource)]
pub(crate) struct Assets3d {
    pub(crate) sphere: Handle<Mesh>,
}

/// Sets up the camera, the links' look and the sky, all hidden until shown.
pub(crate) fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gizmos: ResMut<GizmoConfigStore>,
) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: 0,
            is_active: false,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.008, 0.01, 0.02)),
            ..default()
        },
        bevy::camera::Hdr,
        Tonemapping::AcesFitted,
        // Stronger than the natural look: stars should shine.
        Bloom {
            intensity: 0.34,
            ..Bloom::NATURAL
        },
        RenderLayers::from_layers(&[0, LINKS_LAYER]),
        // Far enough to see the sky from the farthest the camera goes.
        Projection::Perspective(PerspectiveProjection {
            far: (SKY_NEAR + SKY_DEPTH + FARTHEST * 1.5) * 1.2,
            ..default()
        }),
        Transform::from_xyz(0.0, 0.0, 90.0).looking_at(Vec3::ZERO, Vec3::Y),
        UniverseCamera,
    ));
    let (config, _) = gizmos.config_mut::<DefaultGizmoConfigGroup>();
    config.render_layers = RenderLayers::layer(LINKS_LAYER);
    config.line.width = 1.4;

    let sphere = meshes.add(Sphere::new(1.0).mesh().uv(24, 16));
    // The sky: faint stars far off, placed the same each time.
    let mut seed = 0x2545_f491_u32;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let dim = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        emissive: LinearRgba::rgb(0.9, 0.95, 1.2),
        ..default()
    });
    let bright = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        emissive: LinearRgba::rgb(3.0, 3.2, 4.0),
        ..default()
    });
    for i in 0..SKY_STARS {
        let theta = random() * std::f32::consts::TAU;
        let y = random() * 2.0 - 1.0;
        let r = (1.0 - y * y).sqrt();
        let far = SKY_NEAR + random() * SKY_DEPTH;
        let at = Vec3::new(theta.cos() * r, y, theta.sin() * r) * far;
        // As large, seen from where they are, as they always were.
        let size = (0.08 + random() * random() * 0.35) * far / 220.0;
        commands.spawn((
            Mesh3d(sphere.clone()),
            MeshMaterial3d(if i % 9 == 0 {
                bright.clone()
            } else {
                dim.clone()
            }),
            Transform::from_translation(at).with_scale(Vec3::splat(size)),
            Visibility::Hidden,
            InUniverse,
        ));
    }
    commands.insert_resource(Assets3d { sphere });
}

/// `colour` made `brightness` times brighter, in HDR where above 1 blooms,
/// with an alpha of its own: scaling a colour scales its alpha too.
fn hdr(colour: LinearRgba, brightness: f32, alpha: f32) -> LinearRgba {
    LinearRgba::new(
        colour.red * brightness,
        colour.green * brightness,
        colour.blue * brightness,
        alpha,
    )
}

/// Which nodes of `map` a search for `query` finds: the files whose path
/// it matches, and the folders on the way to them.
fn found(map: &CodeMap, query: &str) -> Vec<bool> {
    let mut found: Vec<bool> = map
        .nodes
        .iter()
        .map(|n| {
            matches!(n.kind, NodeKind::File { .. })
                && universe_ui::search_matches(query, [n.path.as_str()])
        })
        .collect();
    let mut ways: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (node, _) in found.iter().enumerate().filter(|(_, f)| **f) {
        let path = map.nodes[node].path.as_str();
        ways.insert("");
        ways.extend(path.match_indices('/').map(|(at, _)| &path[..at]));
    }
    for (node, f) in found.iter_mut().enumerate() {
        if matches!(map.nodes[node].kind, NodeKind::Folder) {
            *f = ways.contains(map.nodes[node].path.as_str());
        }
    }
    found
}

/// The colour of a language, as a direction in HDR.
fn language_colour(language: Language) -> LinearRgba {
    match language {
        Language::Rust => LinearRgba::rgb(1.0, 0.42, 0.18),
        Language::Python => LinearRgba::rgb(0.25, 0.65, 1.0),
        Language::TypeScript => LinearRgba::rgb(0.2, 0.5, 1.0),
        Language::JavaScript => LinearRgba::rgb(1.0, 0.85, 0.25),
        Language::Dart => LinearRgba::rgb(0.1, 0.8, 0.95),
        Language::Go => LinearRgba::rgb(0.4, 0.95, 0.3),
        Language::Markdown => LinearRgba::rgb(0.85, 0.9, 1.0),
        Language::Config => LinearRgba::rgb(0.2, 0.95, 0.75),
        Language::Other => LEFT_OUT,
    }
}

/// The colour of a role, as a direction in HDR.
fn role_colour(role: Role) -> LinearRgba {
    match role {
        Role::Code => LinearRgba::rgb(1.0, 0.5, 0.2),
        Role::Tests => LinearRgba::rgb(0.3, 1.0, 0.45),
        Role::Documents => LinearRgba::rgb(0.85, 0.9, 1.0),
        Role::Settings => LinearRgba::rgb(0.2, 0.95, 0.75),
        Role::Other => LEFT_OUT,
    }
}

/// What a colour means.
struct Meaning {
    /// The group it stands for; `None` for what a criterion leaves out.
    key: Option<Key>,
    name: String,
    colour: LinearRgba,
    /// How many files are in it.
    files: usize,
}

/// What the colours mean, the most files first.
type Legend = Vec<Meaning>;

/// The colour of each galaxy and of each cluster, and the legend of the
/// one that colours the stars: the clusters when there are, as the same
/// group in two galaxies (Rust here and there) shares its colour.
fn colours(map: &CodeMap, grouping: &Grouping) -> (Vec<LinearRgba>, Vec<Vec<LinearRgba>>, Legend) {
    // Every distinct group, galaxies first, in their order: a module's
    // colour is its place among them, the same wherever it shows.
    let mut seen: Vec<Option<Key>> = Vec::new();
    let all = grouping
        .groups
        .iter()
        .chain(grouping.subgroups.iter().flatten());
    for group in all.clone() {
        if !seen.contains(&group.key) {
            seen.push(group.key.clone());
        }
    }
    let modules: Vec<&Key> = seen
        .iter()
        .flatten()
        .filter(|k| matches!(k, Key::Module(_)))
        .collect();
    let layers = all
        .filter_map(|g| match g.key {
            Some(Key::Layer { layer, .. }) => Some(layer),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let colour = |key: &Option<Key>| match key {
        None => LEFT_OUT,
        Some(Key::Language(language)) => language_colour(*language),
        Some(Key::Role(role)) => role_colour(*role),
        Some(Key::Layer { layer, .. }) => {
            // Foundations cool, the top warm, as in the plan.
            let t = if layers > 0 {
                *layer as f32 / layers as f32
            } else {
                0.0
            };
            LinearRgba::rgb(0.2, 0.8, 1.0).mix(&LinearRgba::rgb(1.0, 0.42, 0.2), t)
        }
        Some(module) => {
            let place = modules.iter().position(|m| *m == module).unwrap_or(0);
            let [r, g, b] = PALETTE[place % PALETTE.len()];
            LinearRgba::rgb(r, g, b)
        }
    };
    let galaxies: Vec<LinearRgba> = grouping.groups.iter().map(|g| colour(&g.key)).collect();
    let clusters: Vec<Vec<LinearRgba>> = grouping
        .subgroups
        .iter()
        .map(|groups| groups.iter().map(|g| colour(&g.key)).collect())
        .collect();
    let mut legend: Vec<(Option<Key>, String, usize)> = Vec::new();
    let colouring = if grouping.subgroups.is_empty() {
        grouping.groups.iter().collect::<Vec<_>>()
    } else {
        grouping.subgroups.iter().flatten().collect()
    };
    for group in colouring {
        match legend.iter_mut().find(|(key, ..)| *key == group.key) {
            Some(entry) => entry.2 += group.files,
            None => legend.push((group.key.clone(), group.label.clone(), group.files)),
        }
    }
    legend.sort_by(|a, b| {
        a.0.is_none()
            .cmp(&b.0.is_none())
            .then(b.2.cmp(&a.2))
            .then(a.1.cmp(&b.1))
    });
    let mut legend: Legend = legend
        .into_iter()
        .map(|(key, name, files)| Meaning {
            colour: colour(&key),
            key,
            name,
            files,
        })
        .collect();
    // Ungrouped, the stars keep their languages' colours.
    if grouping.criteria.is_empty() {
        let mut languages: Vec<(Language, usize)> = Vec::new();
        for (at, node) in map.nodes.iter().enumerate() {
            if let NodeKind::File { language, .. } = node.kind
                && grouping.inside(at)
            {
                match languages.iter_mut().find(|(l, _)| *l == language) {
                    Some(entry) => entry.1 += 1,
                    None => languages.push((language, 1)),
                }
            }
        }
        languages.sort_by_key(|(_, files)| std::cmp::Reverse(*files));
        legend = languages
            .into_iter()
            .map(|(language, files)| Meaning {
                key: Some(Key::Language(language)),
                name: language.name().to_owned(),
                colour: language_colour(language),
                files,
            })
            .collect();
    }
    (galaxies, clusters, legend)
}

/// A colour, as the panels draw it.
fn srgb(colour: LinearRgba) -> [u8; 3] {
    let c = Color::from(colour).to_srgba();
    [
        (c.red * 255.0) as u8,
        (c.green * 255.0) as u8,
        (c.blue * 255.0) as u8,
    ]
}

/// How big a node's star is: files by their length, the root the largest.
fn size(node: &universe_codemap::Node) -> f32 {
    match node.kind {
        NodeKind::Folder if node.depth == 0 => 0.5,
        NodeKind::Folder => 0.2,
        NodeKind::File { lines, .. } => 0.11 + 0.07 * (1.0 + lines as f32 / 25.0).ln(),
    }
}

/// Builds the map and the stars the first time the universe shows, and
/// shows or hides the scene with it.
pub(crate) fn show_or_hide(
    mut commands: Commands,
    mut universe: ResMut<Universe>,
    assets: Option<Res<Assets3d>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut camera: Query<&mut Camera, With<UniverseCamera>>,
    mut parts: Query<&mut Visibility, With<InUniverse>>,
) {
    let shown = universe.shown;
    if let Ok(mut camera) = camera.single_mut()
        && camera.is_active != shown
    {
        camera.is_active = shown;
        for mut visibility in &mut parts {
            *visibility = if shown {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
        if shown {
            // Each time it opens, the camera glides in from afar.
            universe.distance = 90.0;
            universe.zoomed = false;
        }
    }
    let Some(assets) = assets else {
        return;
    };
    if !shown || universe.map.is_some() {
        return;
    }
    let map = universe_codemap::map_system(&universe.repos, MAX_FILES);
    let layout = Layout::new(&map);
    let stars = map
        .nodes
        .iter()
        .zip(layout.positions())
        .map(|(node, at)| {
            // Coloured once gathered, below.
            let material = materials.add(StandardMaterial {
                base_color: Color::BLACK,
                emissive: hdr(FOLDER, REST, 1.0),
                ..default()
            });
            commands
                .spawn((
                    Mesh3d(assets.sphere.clone()),
                    MeshMaterial3d(material),
                    Transform::from_translation(Vec3::from_array(*at))
                        .with_scale(Vec3::splat(size(node))),
                    Visibility::Visible,
                    InUniverse,
                    CodeStar,
                ))
                .id()
        })
        .collect();
    universe.on_screen = vec![None; map.nodes.len()];
    universe.stars = stars;
    universe.map = Some(map);
    universe.layout = Some(layout);
    // What was searched for is found again in the new map.
    universe.query.clear();
    universe.regroup();
}

/// What the camera's query reads and moves.
type CameraParts = (
    &'static mut Transform,
    &'static Camera,
    &'static GlobalTransform,
);

/// The universe's camera alone: no star.
type CameraOnly = (
    With<UniverseCamera>,
    Without<MeshMaterial3d<StandardMaterial>>,
);

/// Each frame while shown: the layout's next steps, the stars where it
/// put them, the links, the camera.
pub(crate) fn animate(
    mut universe: ResMut<Universe>,
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut stars: Query<(&mut Transform, &MeshMaterial3d<StandardMaterial>), Without<UniverseCamera>>,
    mut camera: Query<CameraParts, CameraOnly>,
    mut gizmos: Gizmos,
) {
    if !universe.shown || universe.deployments {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    if universe.map.is_none() {
        return;
    }
    let tints: Vec<(LinearRgba, f32, f32)> = (0..universe.stars.len())
        .map(|node| {
            (
                universe.colour(node),
                universe.rest(node),
                universe.scale(node),
            )
        })
        .collect();
    let lit: Vec<bool> = (0..universe.stars.len())
        .map(|node| universe.lit(node))
        .collect();
    let recolour = std::mem::take(&mut universe.recolour);
    let Universe {
        map,
        layout,
        stars: entities,
        grouping,
        ..
    } = &mut *universe;
    let (Some(map), Some(layout)) = (map.as_ref(), layout.as_mut()) else {
        return;
    };
    if !layout.settled() {
        for _ in 0..STEPS_PER_FRAME {
            layout.step(map);
        }
    }
    let positions: Vec<Vec3> = layout
        .positions()
        .iter()
        .map(|p| Vec3::from_array(*p))
        .collect();
    let (middle, extent) = layout.extent();

    for (node, entity) in entities.iter().enumerate() {
        let Ok((mut transform, material)) = stars.get_mut(*entity) else {
            continue;
        };
        transform.translation = positions[node];
        let (colour, rest, scale) = tints[node];
        if recolour {
            transform.scale = Vec3::splat(scale);
            if let Some(mut material) = materials.get_mut(&material.0) {
                material.emissive = hdr(colour, rest, 1.0);
            }
        }
    }

    // The links: what holds what faint and violet, imports in the colour of
    // the file that imports. Only within the galaxy entered: the rest is
    // background.
    for edge in &map.edges {
        if !lit[edge.from] || !lit[edge.to] {
            continue;
        }
        let (a, b) = (positions[edge.from], positions[edge.to]);
        let colour = match edge.kind {
            EdgeKind::Contains => hdr(LinearRgba::rgb(0.35, 0.25, 0.9), 0.9, 0.25),
            EdgeKind::Imports => hdr(tints[edge.from].0, 1.1, 0.32),
            // Through the API, from the front end to the back: magenta.
            EdgeKind::Calls => hdr(LinearRgba::rgb(0.85, 0.35, 0.95), 1.4, 0.45),
        };
        // Between galaxies, fainter: the galaxies stand out, and what ties
        // them still shows.
        let galaxy = |node: usize| grouping.of(node).map(|(g, _)| g);
        let between = galaxy(edge.from) != galaxy(edge.to);
        let colour = if between {
            hdr(colour, 0.45, colour.alpha * 0.45)
        } else {
            colour
        };
        gizmos.line(a, b, Color::from(colour));
    }

    // The camera: turned by dragging, drifting slowly otherwise, gliding in
    // to see the whole universe, or the star chosen.
    let drag = std::mem::take(&mut universe.drag);
    let zoom = std::mem::take(&mut universe.zoom);
    if drag == Vec2::ZERO {
        universe.yaw += 0.06 * dt;
    }
    universe.yaw -= drag.x * 0.006;
    universe.pitch = (universe.pitch + drag.y * 0.006).clamp(-1.4, 1.4);
    let whole = (extent * 2.4).clamp(6.0, FARTHEST);
    let goal = match universe.chosen {
        Some(node) => (positions.get(node).copied().unwrap_or(Vec3::ZERO), 7.0),
        None => (Vec3::from_array(middle), whole),
    };
    // Until the wheel moves it, the camera frames the whole universe, or
    // the star chosen; after, it stays where the wheel left it. The wheel
    // scales the distance, so that a touchpad's burst cannot fly the camera
    // through the stars, where their glow would fill the window.
    if zoom != 0.0 {
        universe.zoomed = true;
        universe.target_distance *= (-zoom * 0.0015).clamp(-1.5, 1.5).exp();
    } else if !universe.zoomed {
        universe.target_distance = goal.1;
    }
    universe.target_distance = universe.target_distance.clamp(NEAREST, whole * 1.4);
    let ease = 1.0 - (-3.0 * dt).exp();
    universe.focus = universe.focus.lerp(goal.0, ease);
    universe.distance += (universe.target_distance - universe.distance) * ease;
    let Ok((mut transform, camera, global)) = camera.single_mut() else {
        return;
    };
    let offset = Vec3::new(
        universe.yaw.cos() * universe.pitch.cos(),
        universe.pitch.sin(),
        universe.yaw.sin() * universe.pitch.cos(),
    ) * universe.distance;
    let looking =
        Transform::from_translation(universe.focus + offset).looking_at(universe.focus, Vec3::Y);
    // Moved sideways, not turned: the universe stays upright and centred
    // in the part of the window the panels leave free.
    let aspect = camera
        .logical_viewport_size()
        .map_or(1.6, |size| size.x / size.y.max(1.0));
    let half_width = universe.distance * std::f32::consts::FRAC_PI_8.tan() * aspect;
    let shift = looking.right() * (universe.covered * half_width);
    *transform = looking.with_translation(looking.translation + shift);
    // Where each star is on screen, for the panels to label and pick.
    universe.on_screen = positions
        .iter()
        .map(|p| camera.world_to_viewport(global, *p).ok())
        .collect();
    // And where each galaxy's name goes: just above its highest star, as
    // the camera sees it.
    let up = looking.up();
    let galaxies = universe.grouping.groups.len();
    let mut sums = vec![(Vec3::ZERO, 0_usize); galaxies];
    for (node, at) in positions.iter().enumerate() {
        if let Some((galaxy, _)) = universe.grouping.of(node) {
            sums[galaxy].0 += *at;
            sums[galaxy].1 += 1;
        }
    }
    let middles: Vec<Vec3> = sums
        .iter()
        .map(|(sum, n)| *sum / (*n).max(1) as f32)
        .collect();
    let mut tops = vec![0.0_f32; galaxies];
    for (node, at) in positions.iter().enumerate() {
        if let Some((galaxy, _)) = universe.grouping.of(node) {
            tops[galaxy] = tops[galaxy].max((*at - middles[galaxy]).dot(*up));
        }
    }
    universe.galaxies_on_screen = middles
        .iter()
        .zip(&tops)
        .map(|(middle, top)| {
            camera
                .world_to_viewport(global, *middle + *up * (top + 1.0))
                .ok()
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use universe_codemap::Node;

    use super::*;

    fn universe() -> Universe {
        let node = |path: &str, kind| Node {
            path: path.into(),
            kind,
            depth: path.matches('/').count() + 1,
        };
        let file = NodeKind::File {
            language: Language::Rust,
            lines: 10,
        };
        let map = CodeMap {
            nodes: vec![
                node("", NodeKind::Folder),
                node("src", NodeKind::Folder),
                node("src/a.rs", file),
                node("src/b.rs", file),
            ],
            edges: Vec::new(),
            left_out: 0,
            ..CodeMap::default()
        };
        let mut universe = Universe::new(Repos::new(std::path::Path::new("/p"), &[]), "p".into());
        universe.layout = Some(Layout::new(&map));
        universe.map = Some(map);
        universe.regroup();
        universe
    }

    #[test]
    fn a_search_lights_only_the_files_it_finds() {
        let mut universe = universe();
        let lit = |universe: &Universe| (2..4).map(|n| universe.lit(n)).collect::<Vec<_>>();
        assert_eq!(lit(&universe), [true, true]);
        universe.search("a.rs");
        assert!(universe.searching());
        assert_eq!(lit(&universe), [true, false]);
        universe.search("");
        assert!(!universe.searching());
        assert_eq!(lit(&universe), [true, true]);
    }
}
