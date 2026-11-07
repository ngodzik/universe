//! The universe window: the interface's state drawn on the GPU.
//!
//! The state, the keys and what each does live in `universe-ui`, and the
//! effects are carried out by its `Host`. This crate only:
//! - reads the window's keys and hands them to the state (`keys`);
//! - drives the `Host` once a frame, without ever waiting on it;
//! - draws the state with egui on Bevy: the project's views (`universe`,
//!   `plan`, `services_view`, `api_view`, `database_view` and
//!   `database_space`, `fleet_view`), its files and the file open.
//!
//! Bevy owns the window and the frame loop, so that the views draw in 2D
//! and 3D under the same panels.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod api_view;
mod arch_view;
mod backdrop;
mod database_space;
mod database_view;
mod error;
mod fleet_view;
mod infra;
mod keys;
mod plan;
mod service_board;
mod services_view;
mod theme;
mod universe;
mod universe_view;
mod windows;

use std::time::Duration;

use bevy::app::AppExit;
use bevy::camera::CameraOutputMode;
use bevy::input::ButtonState;
use bevy::input::keyboard::{KeyCode as PhysicalKey, KeyboardInput};
use bevy::prelude::*;
use bevy::render::render_resource::BlendState;
use bevy::window::{CompositeAlphaMode, PrimaryWindow};
use bevy::winit::{UpdateMode, WinitSettings};
use bevy_egui::egui::{
    self, Align, Color32, Frame, Id, LayerId, Layout, Margin, RichText, Ui, UiBuilder,
};
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiInput, EguiInputSet, EguiOutput, EguiPlugin,
    EguiPostUpdateSet, EguiPreUpdateSet, EguiPrimaryContextPass, PrimaryEguiContext,
};
use universe_codemap::System;
use universe_ui::backdrop::Motion;
use universe_ui::input::{KeyCode, KeyEvent, KeyModifiers};
use universe_ui::keymap::{Focus, Mode};
use universe_ui::reader::ReaderMode;
use universe_ui::{App, Effect, Host, MapView, clipboard};

pub use error::GuiError;

use crate::keys::{Held, Pressed};
use crate::plan::Plan;
use crate::theme::{ACCENT, DIM, PANEL, RAISED, SELECTED, TEXT, YELLOW};
use crate::universe::Universe;

/// How the window's pixels blend with what is behind it, or why they cannot.
/// Metal offers only `Opaque` and `PostMultiplied`; a Wayland compositor
/// takes premultiplied pixels. Under X11 transparency hangs on the driver
/// and a compositor, and some (NVIDIA's Vulkan) offer only `Opaque`: asked
/// for another, the surface cannot be made and the window stops at its first
/// frame. Bevy does not fall back, so the window is opaque there.
fn composite_alpha() -> Result<CompositeAlphaMode, &'static str> {
    if cfg!(target_os = "macos") {
        Ok(CompositeAlphaMode::PostMultiplied)
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty()) {
        Ok(CompositeAlphaMode::PreMultiplied)
    } else {
        Err(
            "under X11, a window's transparency depends on the driver and is not asked for; it can be under Wayland or macOS",
        )
    }
}

/// How long the window may sleep while a lookup runs: its places show
/// within it.
const BUSY_WAIT: Duration = Duration::from_millis(100);

/// How long it may sleep otherwise. Nothing changes then but a key, which
/// wakes it at once.
const IDLE_WAIT: Duration = Duration::from_secs(1);

/// How long the text cursor stays lit, then dark, while a command is typed:
/// the window wakes at this pace to blink it, and no faster.
const BLINK: Duration = Duration::from_millis(530);

/// How long it may sleep while a background moves: about twelve frames a
/// second, enough for a slow pattern.
const BACKDROP_WAIT: Duration = Duration::from_millis(80);

/// The height of macOS's title bar, in points.
const TITLE_BAR: f32 = 28.0;

/// Opens the window on `system`, a project alone or the repositories of a
/// system, whose state is `app`, and runs it until it is closed.
///
/// # Errors
///
/// When Bevy stops with an error: the window could not be opened, or a
/// system failed.
pub fn run(mut app: App, system: System) -> Result<(), GuiError> {
    if let Err(why) = composite_alpha() {
        app.opaque_only(why);
    }
    let title = format!("{} · universe", app.project());
    let universe = Universe::new(system.repos.clone(), system.name.clone());
    let shell = Shell {
        host: Host::new(),
        plan: Plan::new(system.repos.clone()),
        api: api_view::ApiView::default(),
        database: database_view::DatabaseView::default(),
        services: services_view::ServicesView::default(),
        infra: infra::Infra::new(system),
        app,
        keys: Vec::new(),
        wheel_rest: 0.0,
        typing: false,
    };

    let mut window = bevy::app::App::new();
    window
        // Behind the panels, nothing: what shows through a see-through
        // window is the desktop, through the panels' own alpha.
        .insert_resource(ClearColor(Color::NONE))
        .insert_resource(WinitSettings {
            focused_mode: UpdateMode::reactive(IDLE_WAIT),
            unfocused_mode: UpdateMode::reactive_low_power(IDLE_WAIT),
        })
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(main_window(title)),
                    // The window the system takes away, as an unplugged
                    // screen does, is opened again rather than quit; the
                    // close button and Ctrl-C still quit, below.
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    close_when_requested: false,
                    ..default()
                })
                .set(bevy::log::LogPlugin {
                    level: bevy::log::Level::WARN,
                    ..default()
                })
                // Pipelined, a frame reaches the screen only when the next
                // one runs; the window sleeps between keys, so a key
                // pressed showed only at the next wake.
                .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(),
        )
        .add_plugins(EguiPlugin::default())
        // egui draws with the panels' camera, named rather than guessed:
        // the universe has a camera of its own.
        .insert_resource(EguiGlobalSettings {
            auto_create_primary_context: false,
            ..default()
        })
        .insert_non_send(shell)
        .insert_resource(universe)
        .insert_resource(fleet_view::FleetView::default())
        .insert_resource(database_space::DatabaseSpace::default())
        .add_systems(Startup, (panels_camera, universe::setup))
        .add_systems(
            PreUpdate,
            quiet_modifiers
                .after(EguiInputSet::WriteEguiEvents)
                .before(EguiPreUpdateSet::BeginPass),
        )
        .add_systems(
            Update,
            (
                read_keys,
                drive,
                universe::show_or_hide,
                fleet_view::spawn,
                universe::animate,
                fleet_view::animate,
                database_space::spawn,
                database_space::animate,
            )
                .chain(),
        )
        .add_systems(EguiPrimaryContextPass, (draw, note_typing.after(draw)))
        .add_systems(Last, close_or_reopen)
        .add_systems(
            PreUpdate,
            follow_new_window.before(EguiPreUpdateSet::InitContexts),
        )
        .add_systems(
            PostUpdate,
            copy_selection.after(EguiPostUpdateSet::ProcessOutput),
        );
    match window.run() {
        AppExit::Success => Ok(()),
        AppExit::Error(code) => Err(GuiError::Exited(code.get())),
    }
}

/// The state, the host that carries out its effects, and what the window
/// remembers between frames. Not `Send`: the state is the main thread's.
struct Shell {
    app: App,
    host: Host,
    /// Keys typed by a button, for the state on the next frame.
    keys: Vec<KeyEvent>,
    /// The codebase's plan, drawn by egui alone; the other views read its
    /// map.
    plan: Plan,
    /// The codebase's API, from the plan's map.
    api: api_view::ApiView,
    /// The tables the project's models define.
    database: database_view::DatabaseView,
    /// The services the project runs, and who reaches whom.
    services: services_view::ServicesView,
    /// Where the project runs, read on threads of their own.
    infra: infra::Infra,
    /// What the wheel or the touchpad moved short of a whole line, kept for
    /// the next move: a touchpad sends many small ones.
    wheel_rest: f32,
    /// Whether a field of egui's has the keys, as last drawn: what is typed
    /// there is not universe's.
    typing: bool,
}

impl Shell {
    /// Carries out `effect`, without waiting.
    fn carry_out(&mut self, effect: Effect) {
        let Self { app, host, .. } = self;
        host.carry_out(app, effect);
    }

    fn key(&mut self, key: KeyEvent) {
        if let Some(effect) = self.app.on_key(key) {
            self.carry_out(effect);
        }
    }
}

/// The panels' camera: drawn over the universe's, blending into what it
/// drew, or over nothing while the universe is hidden.
fn panels_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            output_mode: CameraOutputMode::Write {
                blend_state: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                clear_color: ClearColorConfig::Custom(Color::NONE),
            },
            ..default()
        },
        PrimaryEguiContext,
    ));
}

/// Shows the panels over the universe while it is shown, over a cleared
/// window otherwise: the theme's background when the window is opaque, so
/// that a frame egui misses (it skips one while its font texture is made
/// anew) shows the background rather than the desktop behind the window.
fn keep_panels_over(universe: &Universe, opaque: bool, camera: &mut Mut<Camera>) {
    let wanted = if universe.in_space() {
        None
    } else if opaque {
        let [r, g, b, _] = theme::BACKGROUND.to_array();
        Some(Color::srgb_u8(r, g, b))
    } else {
        Some(Color::NONE)
    };
    let CameraOutputMode::Write { clear_color, .. } = &camera.output_mode else {
        return;
    };
    let current = match clear_color {
        ClearColorConfig::Custom(colour) => Some(*colour),
        _ => None,
    };
    // Checked before writing: a write would mark the camera changed.
    if current == wanted {
        return;
    }
    if let CameraOutputMode::Write { clear_color, .. } = &mut camera.output_mode {
        *clear_color = wanted.map_or(ClearColorConfig::None, ClearColorConfig::Custom);
    }
}

/// Drops the modifiers bevy_egui reports every frame when they did not
/// change. egui takes any event as a reason to draw again at once, so that
/// one, sent each frame, kept the window from ever sleeping: two cores busy
/// with nothing on screen moving.
///
/// It also asks egui to copy on Cmd-C (Ctrl-Shift-C off macOS, Ctrl-C
/// being the state's): bevy_egui, built without its clipboard, never does.
fn quiet_modifiers(mut inputs: Query<&mut EguiInput>, mut last: Local<Option<egui::Modifiers>>) {
    for mut input in &mut inputs {
        input.events.retain(|event| match event {
            egui::Event::ModifiersChanged(now) if *last == Some(*now) => false,
            egui::Event::ModifiersChanged(now) => {
                *last = Some(*now);
                true
            }
            _ => true,
        });
        let copy = input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::C,
                    pressed: true,
                    modifiers,
                    ..
                } if keys::copies(*modifiers)
            )
        });
        if copy {
            input.events.push(egui::Event::Copy);
        }
    }
}

/// Hands each key pressed to the state, and pastes when asked.
///
/// The modifiers are followed through the events in their order rather
/// than read once a frame: Ctrl pressed and released within one frame, as
/// a quick Ctrl-N is, would otherwise be missed.
fn read_keys(
    mut pressed: MessageReader<KeyboardInput>,
    mut held: Local<Held>,
    mut shell: NonSendMut<Shell>,
) {
    for event in pressed.read() {
        let down = event.state == ButtonState::Pressed;
        match event.key_code {
            PhysicalKey::ShiftLeft | PhysicalKey::ShiftRight => held.shift = down,
            PhysicalKey::ControlLeft | PhysicalKey::ControlRight => held.control = down,
            PhysicalKey::AltLeft => held.alt = down,
            PhysicalKey::SuperLeft | PhysicalKey::SuperRight => held.command = down,
            _ => {}
        }
        // A field of egui's has the keys once focused, but universe's own
        // that move between views.
        if !down || (shell.typing && !(held.control && event.key_code == PhysicalKey::KeyN)) {
            continue;
        }
        match keys::translate(&event.logical_key, *held) {
            Some(Pressed::Key(key)) => shell.key(key),
            Some(Pressed::Paste) => match clipboard::paste() {
                Ok(text) => shell.app.on_paste(&text),
                Err(e) => shell.app.report_error(e),
            },
            Some(Pressed::TextSize(step)) => shell.app.step_text_size(step),
            None => {}
        }
    }
}

/// Once a frame: what the buttons asked for, what the lookups found, what
/// the state queued; then the window's title, how long it may sleep, and
/// whether to quit.
fn drive(
    mut shell: NonSendMut<Shell>,
    mut exit: MessageWriter<AppExit>,
    mut winit: ResMut<WinitSettings>,
    mut universe: ResMut<Universe>,
    fleet: Res<fleet_view::FleetView>,
    mut panels: Query<&mut Camera, With<Camera2d>>,
) {
    for key in std::mem::take(&mut shell.keys) {
        shell.key(key);
    }
    {
        let Shell { app, host, .. } = &mut *shell;
        host.receive(app);
    }
    while let Some(effect) = shell.app.take_queued() {
        shell.carry_out(effect);
    }

    let wait = if shell.app.is_looking() {
        BUSY_WAIT
    } else if shell.app.mode() != Mode::Normal {
        BLINK
    } else {
        IDLE_WAIT
    };
    let shown = shell.app.map_view() == Some(MapView::Universe);
    if universe.shown != shown {
        universe.shown = shown;
    }
    let schema = shell.app.map_view() == Some(MapView::Database) && !shell.database.flat();
    if universe.schema != schema {
        universe.schema = schema;
    }
    if let Ok(mut camera) = panels.single_mut() {
        keep_panels_over(&universe, shell.app.opacity() >= 1.0, &mut camera);
    }
    // Written only when it changes: a write wakes the loop, and a write
    // every frame would keep it from ever sleeping. The universe moves all
    // the time, and only it and the database in space draw without rest; a
    // service's board is flat, and the space behind it rests.
    let continuous = (shown && (!fleet.active || fleet.moving())) || schema;
    // A moving background wakes the window about twelve times a second
    // while it has the focus; behind other windows it sleeps as before.
    let backdrop = shell.app.backdrop();
    let moving = backdrop.pattern.is_some() && backdrop.motion == Motion::Slow;
    let focused = if continuous {
        UpdateMode::Continuous
    } else if moving {
        UpdateMode::reactive(wait.min(BACKDROP_WAIT))
    } else {
        UpdateMode::reactive(wait)
    };
    if winit.focused_mode != focused {
        winit.focused_mode = focused;
        winit.unfocused_mode = if continuous {
            UpdateMode::Continuous
        } else {
            UpdateMode::reactive_low_power(wait)
        };
    }
    if shell.app.should_quit() {
        exit.write(AppExit::Success);
    }
}

/// Draws the state. Changes nothing in it but what only the drawing knows:
/// what was clicked, and how far the panes scroll.
#[allow(clippy::too_many_arguments)]
fn draw(
    mut contexts: EguiContexts,
    mut shell: NonSendMut<Shell>,
    mut universe: ResMut<Universe>,
    mut fleet: ResMut<fleet_view::FleetView>,
    mut space: ResMut<database_space::DatabaseSpace>,
    mut styled: Local<bool>,
    mut followed: Local<f32>,
    window: Query<Entity, With<PrimaryWindow>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    if !*styled {
        ctx.set_theme(egui::Theme::Dark);
        theme::apply(&ctx);
        // The state keeps the text's size: egui's own keys would change it
        // behind its back.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        *styled = true;
        // The fonts set take effect on the next pass: text drawn in a
        // family they add, bold, would find none now.
        ctx.request_repaint();
        return Ok(());
    }
    let size = shell.app.text_size();
    if (ctx.zoom_factor() - size).abs() > 0.001 {
        ctx.set_zoom_factor(size);
    }
    let shell = &mut *shell;
    let mut root = Ui::new(
        ctx.clone(),
        Id::new("universe"),
        UiBuilder::new()
            .layer_id(LayerId::background())
            .max_rect(ctx.content_rect()),
    );
    // Ctrl-M fades the background in or out rather than switching it.
    let opacity = ctx.animate_value_with_time(Id::new("opacity"), shell.app.opacity(), 0.25);
    if (*followed - opacity).abs() > 0.001 {
        theme::follow(&ctx, opacity);
        *followed = opacity;
    }
    let panel = theme::see(PANEL, opacity);
    let background = theme::see(theme::BACKGROUND, opacity);
    let pane = Frame::new().fill(panel).inner_margin(Margin::same(14));

    // On macOS the content goes under the title bar: a band of its height
    // keeps the system's buttons and title on something opaque, except in
    // full screen, which winit reports however it was entered.
    if cfg!(target_os = "macos") {
        let full = window.single().ok().is_some_and(|entity| {
            bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
                windows
                    .get_window(entity)
                    .is_some_and(|w| w.fullscreen().is_some())
            })
        });
        if !full {
            egui::Panel::top("title-bar")
                .exact_size(TITLE_BAR / ctx.zoom_factor())
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::new().fill(theme::BACKGROUND))
                .show(&mut root, |_| {});
        }
    }
    egui::Panel::bottom("status")
        .frame(
            Frame::new()
                .fill(background)
                .inner_margin(Margin::symmetric(14, 4)),
        )
        .show_separator_line(false)
        .show(&mut root, |ui| status(ui, &shell.app));

    let now = ctx.input(|i| i.time);
    let view = shell.app.map_view();
    // The plan's map is read while a view needs it: the deployments need
    // the services to render environments.
    shell.plan.showing(
        matches!(
            view,
            Some(MapView::Plan | MapView::Services | MapView::Api | MapView::Database)
        ) || (view == Some(MapView::Universe) && fleet.active),
        now,
    );
    shell.plan.take_read(now);
    if view == Some(MapView::Database) {
        shell.database.update(&shell.plan);
    }
    if let Some(view) = view {
        let window = ctx.content_rect().width().max(1.0);
        let free = root.available_rect_before_wrap();
        universe.covered = (ctx.content_rect().right() - free.right()).max(0.0) / window;
        fleet.covered = universe.covered;
        space.covered = universe.covered;
        let in_space = universe.in_space();
        egui::CentralPanel::default()
            .frame(if in_space {
                Frame::NONE
            } else {
                Frame::NONE.fill(background)
            })
            .show(&mut root, |ui| match view {
                MapView::Universe if fleet.active => {
                    if fleet_view::show(
                        ui,
                        &mut fleet,
                        &mut shell.infra,
                        &shell.plan,
                        &mut shell.app,
                    ) {
                        fleet.active = false;
                    }
                }
                MapView::Universe => {
                    if universe_view::show(ui, &mut universe, &mut shell.app) {
                        fleet.active = true;
                    }
                }
                MapView::Plan => plan::show(ui, &mut shell.plan, &mut shell.app),
                MapView::Api => api_view::show(ui, &mut shell.api, &shell.plan, &mut shell.app),
                MapView::Services => services_view::show(
                    ui,
                    &mut shell.services,
                    &shell.plan,
                    &mut shell.infra,
                    &mut shell.api,
                    &mut shell.app,
                ),
                MapView::Database => {
                    database_view::show(ui, &mut shell.database, &mut space, &mut shell.app);
                }
            });
    } else {
        if shell.app.tree().is_some() {
            egui::Panel::left("tree")
                .default_size(280.0)
                .frame(pane)
                .show(&mut root, |ui| {
                    tree(ui, &mut shell.app, &mut shell.wheel_rest, &mut shell.keys);
                });
        }
        egui::CentralPanel::default()
            .frame(pane.fill(background))
            .show(&mut root, |ui| {
                if shell.app.file().is_some() {
                    file(ui, &mut shell.app, &mut shell.wheel_rest, &mut shell.keys);
                } else {
                    backdrop::paint(ui, &shell.app);
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new(
                                "Choose a file in the tree. Ctrl-N goes back to the views",
                            )
                            .color(DIM),
                        );
                    });
                }
            });
    }

    if universe.deployments != fleet.active {
        universe.deployments = fleet.active;
    }
    windows::show(&ctx, &shell.app, &mut shell.keys);
    if !shell.keys.is_empty() {
        ctx.request_repaint();
    }
    Ok(())
}

/// The window, as first opened and as opened again.
fn main_window(title: String) -> Window {
    Window {
        title,
        resolution: (1400, 900).into(),
        // Made see-through from the start where it can be: a window cannot
        // become so later. Opaque, its panels hide it all.
        transparent: composite_alpha().is_ok(),
        composite_alpha_mode: composite_alpha().unwrap_or(CompositeAlphaMode::Opaque),
        // A see-through window leaves its title bar see-through too: on
        // macOS the content goes under it, and a band of the bar's height
        // is drawn opaque there.
        fullsize_content_view: cfg!(target_os = "macos"),
        titlebar_transparent: cfg!(target_os = "macos"),
        ..default()
    }
}

/// Quits when the window's close button asks; opens the window again when
/// the system took it away unasked, as unplugging a screen does. Each step
/// is logged.
fn close_or_reopen(
    mut commands: Commands,
    mut requested: MessageReader<bevy::window::WindowCloseRequested>,
    mut closed: MessageReader<bevy::window::WindowClosed>,
    mut destroyed: MessageReader<bevy::window::WindowDestroyed>,
    windows: Query<(), With<PrimaryWindow>>,
    mut shell: NonSendMut<Shell>,
) {
    if requested.read().next().is_some() {
        warn!("the window's close button was pressed: quitting");
        shell.app.quit();
    }
    let gone = closed.read().count() + destroyed.read().count() > 0;
    if gone && !shell.app.should_quit() && windows.is_empty() {
        warn!("the system took the window away unasked: opening it again");
        let title = format!("{} · universe", shell.app.project());
        commands.spawn((main_window(title), PrimaryWindow));
    }
}

/// Points egui's contexts at the primary window when it is made again, as
/// macOS does after sleep or when a screen is unplugged. bevy_egui maps a
/// window to its contexts only when a context is made: the new window's
/// clicks and scrolling would reach nothing, while keys, which universe
/// reads itself, still arrive. Harmless where windows are never remade.
fn follow_new_window(
    added: Query<Entity, Added<PrimaryWindow>>,
    contexts: Query<(Entity, &bevy::camera::RenderTarget), With<bevy_egui::EguiContext>>,
    mut map: ResMut<bevy_egui::input::WindowToEguiContextMap>,
) {
    let Ok(window) = added.single() else {
        return;
    };
    for (context, target) in &contexts {
        if !matches!(
            target,
            bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Primary)
        ) {
            continue;
        }
        if let Some(old) = map.context_to_window.insert(context, window)
            && old != window
            && let Some(theirs) = map.window_to_contexts.get_mut(&old)
        {
            theirs.remove(&context);
        }
        map.window_to_contexts
            .entry(window)
            .or_default()
            .insert(context);
    }
}

/// What egui copied, a selection on Cmd-C (Ctrl-Shift-C elsewhere) or a
/// copy button, to the system's clipboard: bevy_egui is built without its
/// own. egui copies a label's selection only at the end of its pass, so it
/// is read once the output is processed, never while drawing.
fn copy_selection(outputs: Query<&EguiOutput, Changed<EguiOutput>>, mut shell: NonSendMut<Shell>) {
    for output in &outputs {
        for command in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command
                && let Err(e) = clipboard::copy(text)
            {
                shell.app.report_error(e);
            }
        }
    }
}

/// Notes whether a field of egui's has the keys, for the next frame's keys.
fn note_typing(mut contexts: EguiContexts, mut shell: NonSendMut<Shell>) -> Result {
    let typing = contexts.ctx_mut()?.egui_wants_keyboard_input();
    if shell.typing != typing {
        shell.typing = typing;
    }
    Ok(())
}

/// The status line: the mode, the `:` command being typed, then the note
/// of the moment and the project.
fn status(ui: &mut Ui, app: &App) {
    ui.horizontal(|ui| {
        let mode = match app.mode() {
            Mode::Normal => "NORMAL",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
        };
        Frame::new()
            .fill(if app.mode() == Mode::Normal {
                RAISED
            } else {
                ACCENT.gamma_multiply(0.3)
            })
            .corner_radius(4)
            .inner_margin(Margin::symmetric(6, 1))
            .show(ui, |ui| {
                ui.label(RichText::new(mode).small().strong().color(TEXT));
            });
        if app.mode() == Mode::Command {
            let line = app.command_line();
            let text: Vec<char> = line.text().chars().collect();
            let cursor = line.cursor().min(text.len());
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(RichText::new(":").monospace().color(ACCENT));
            ui.label(
                RichText::new(text[..cursor].iter().collect::<String>())
                    .monospace()
                    .color(TEXT),
            );
            caret(ui);
            ui.label(
                RichText::new(text[cursor..].iter().collect::<String>())
                    .monospace()
                    .color(TEXT),
            );
            ui.spacing_mut().item_spacing.x = 8.0;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(app.project()).small().color(DIM));
            // The note in what is left between: cut short rather than over
            // the rest, whole on hover.
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                if let Some((text, bad)) = app.notice() {
                    let colour = if bad { theme::RED } else { YELLOW };
                    ui.add(egui::Label::new(RichText::new(text).small().color(colour)).truncate())
                        .on_hover_text(text);
                }
            });
        });
    });
}

/// The text cursor, lit or dark as the time says.
fn caret(ui: &mut Ui) {
    let lit = ((ui.input(|i| i.time) / BLINK.as_secs_f64()) as u64).is_multiple_of(2);
    let height = ui.text_style_height(&egui::TextStyle::Monospace);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(2.0, height), egui::Sense::hover());
    if lit {
        ui.painter().rect_filled(rect, 1, ACCENT);
    }
}

/// The project's files, each marked with git's letter when it has one.
fn tree(ui: &mut Ui, app: &mut App, rest: &mut f32, keys: &mut Vec<KeyEvent>) {
    backdrop::paint(ui, app);
    let Some(tree) = app.tree() else {
        return;
    };
    let focused = app.focus() == Focus::Tree;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Files")
                .strong()
                .color(if focused { ACCENT } else { DIM }),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .small_button("hide")
                .on_hover_text("Ctrl-B shows or hides the files")
                .clicked()
            {
                keys.push(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
            }
        });
    });
    ui.add_space(4.0);
    let row = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let visible = ((ui.available_height() / row) as usize).max(1);
    let area = ui.available_rect_before_wrap();
    let selected = tree.selected();
    let mut offset = tree.offset();
    if selected < offset {
        offset = selected;
    } else if selected >= offset + visible {
        offset = selected + 1 - visible;
    }
    tree.set_offset(offset);
    let mut clicked = None;
    for (i, entry) in tree.rows().iter().enumerate().skip(offset).take(visible) {
        let icon = if entry.is_dir {
            if tree.is_expanded(&entry.path) {
                "▾ "
            } else {
                "▸ "
            }
        } else {
            "  "
        };
        let letter = tree.git_status(&entry.path, entry.is_dir);
        let letter_colour = |letter: char| match letter {
            'A' | '?' => theme::GREEN,
            'D' => theme::RED,
            'R' => theme::CYAN,
            _ => YELLOW,
        };
        let text = RichText::new(format!("{}{icon}{}", "   ".repeat(entry.depth), entry.name))
            .color(if let Some(letter) = letter.filter(|_| !entry.is_dir) {
                letter_colour(letter)
            } else if entry.is_dir {
                TEXT
            } else {
                DIM
            });
        let text = if letter == Some('D') {
            text.strikethrough()
        } else {
            text
        };
        let shown = Frame::new()
            .fill(if i == selected && focused {
                SELECTED
            } else {
                Color32::TRANSPARENT
            })
            .corner_radius(4)
            .inner_margin(Margin::symmetric(4, 1))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(text).selectable(false));
                    if let Some(letter) = letter.filter(|_| !entry.is_dir) {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(letter.to_string())
                                    .monospace()
                                    .small()
                                    .color(letter_colour(letter)),
                            );
                        });
                    }
                });
            });
        let response = shown.response.interact(egui::Sense::click());
        if response.hovered() {
            ui.painter()
                .rect_filled(response.rect, 4, Color32::from_white_alpha(6));
        }
        if response.clicked() {
            clicked = Some(i);
        }
    }
    // The wheel moves the choice, as the arrows do, so that what is chosen
    // stays in sight.
    let lines = wheel_lines(ui, area, rest, row);
    if lines != 0 {
        app.wheel(Focus::Tree, lines);
    }
    if let Some(index) = clicked {
        app.click_tree(index);
    }
}

/// The wheel's or the touchpad's move over `area` since last frame, in
/// whole rows of `row` points, down when positive; what is left over waits
/// in `rest`.
fn wheel_lines(ui: &Ui, area: egui::Rect, rest: &mut f32, row: f32) -> i32 {
    if !ui.rect_contains_pointer(area) {
        return 0;
    }
    let delta = ui.input(|i| i.smooth_scroll_delta.y);
    if delta == 0.0 {
        return 0;
    }
    *rest -= delta / row.max(1.0);
    let lines = rest.trunc();
    *rest -= lines;
    lines as i32
}

/// The open file, its lines numbered and coloured by its language, the
/// cursor's line lit; the reader's mode and message under it.
fn file(ui: &mut Ui, app: &mut App, rest: &mut f32, keys: &mut Vec<KeyEvent>) {
    backdrop::paint(ui, app);
    let Some(reader) = app.file() else {
        return;
    };
    let focused = app.focus() == Focus::File;
    let mut close = false;
    let mut definition = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(reader.path().display().to_string())
                .strong()
                .color(if focused { ACCENT } else { DIM }),
        );
        ui.label(RichText::new("read only").small().color(DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("close").on_hover_text("q").clicked() {
                close = true;
            }
            // Back to where gd left from, and forward again.
            if ui
                .small_button("›")
                .on_hover_text("forward again (Alt-→)")
                .clicked()
            {
                keys.push(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
            }
            if ui
                .small_button("‹")
                .on_hover_text("back to where gd or a Ctrl-click left from (Ctrl-O, Alt-←)")
                .clicked()
            {
                keys.push(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
            }
            if app.tree().is_none()
                && ui
                    .small_button("files")
                    .on_hover_text("the file tree (Ctrl-A)")
                    .clicked()
            {
                keys.push(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
            }
        });
    });
    ui.add_space(6.0);
    let row = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y;
    let glyph =
        ui.fonts_mut(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), 'm'));
    let rows = ((ui.available_height() - 40.0) / row).max(1.0) as usize;
    let columns = ((ui.available_width() - 60.0) / glyph.max(1.0)).max(1.0) as usize;
    reader.set_viewport(rows, columns);
    let (cursor_row, cursor_column) = reader.cursor();
    let selection = reader.selection();
    let styled = reader.styled();
    let text_area = ui.available_rect_before_wrap();
    let mut clicked = None;
    // Long lines shift left together to keep the cursor in sight, as in
    // Vim with `nowrap`.
    let left = reader.left_offset();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    for (on_screen, i) in reader.in_view(rows).enumerate() {
        let line = &reader.lines()[i];
        let current = i == cursor_row;
        // One piece of text per line, its runs coloured: a widget per run
        // made every key cost a frame of hundreds of them.
        let mut job = egui::text::LayoutJob::default();
        let format = |colour: Color32| egui::TextFormat::simple(font.clone(), colour);
        job.append(
            &format!("{:>5}  ", i + 1),
            0.0,
            format(if current { ACCENT } else { DIM }),
        );
        let mut skip = left;
        let mut runs = |text: &str, colour: Color32| {
            let count = text.chars().count();
            if skip >= count {
                skip -= count;
                return;
            }
            let from = text.char_indices().nth(skip).map_or(text.len(), |(b, _)| b);
            skip = 0;
            job.append(&text[from..], 0.0, format(colour));
        };
        match styled.as_deref().and_then(|s| s.get(i)) {
            Some(styled) => {
                for (colour, text) in styled {
                    runs(text, theme::rgb(*colour));
                }
            }
            None => runs(line, TEXT),
        }
        job.wrap.max_rows = 1;
        job.wrap.max_width = f32::INFINITY;
        let background = if current {
            SELECTED
        } else {
            Color32::TRANSPARENT
        };
        let shown = Frame::new().fill(background).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(job).selectable(false).extend());
        });
        let rect = shown.response.rect;
        // What visual mode selects, over the text.
        if let Some((from, to)) = selection.and_then(|s| s.columns(i, line.chars().count())) {
            let x = |column: usize| rect.left() + (7 + column.saturating_sub(left)) as f32 * glyph;
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x(from), rect.top()),
                    egui::pos2(x(to), rect.bottom()),
                ),
                1,
                ACCENT.gamma_multiply(0.25),
            );
        }
        if current && focused {
            // The cursor: a block, as in Vim's normal mode.
            let x = rect.left() + (7 + cursor_column.saturating_sub(left)) as f32 * glyph;
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(x, rect.top()),
                    egui::vec2(glyph, rect.height()),
                ),
                1,
                theme::see(ACCENT, 0.45),
            );
        }
        let response = shown.response.interact(egui::Sense::click());
        if response.clicked()
            && let Some(at) = response.interact_pointer_pos()
        {
            // Past the line numbers, in characters on screen: the reader
            // adds the shift.
            let gutter = 7.0 * glyph;
            let column = ((at.x - rect.left() - gutter) / glyph.max(1.0)).max(0.0) as usize;
            if ui.input(|i| i.modifiers.command) {
                // Ctrl-click: to where the name clicked is defined.
                definition = Some((on_screen, column));
            } else {
                clicked = Some((on_screen, column));
            }
        }
    }
    // The colours are borrowed from the reader, which the clicks change.
    drop(styled);
    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        let mode = match reader.mode() {
            ReaderMode::Normal => "READ".to_owned(),
            ReaderMode::Visual { line: true } => "VISUAL LINE".to_owned(),
            ReaderMode::Visual { line: false } => "VISUAL".to_owned(),
            ReaderMode::Command => format!(":{}", reader.prompt()),
            ReaderMode::Search => format!("/{}", reader.prompt()),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(mode).small().color(DIM));
            if let Some((message, error)) = reader.message() {
                ui.label(RichText::new(message).small().color(if error {
                    theme::RED
                } else {
                    DIM
                }));
            }
        });
    });
    let lines = wheel_lines(ui, text_area, rest, row);
    if lines != 0 {
        app.wheel(Focus::File, lines);
    }
    if let Some((row, column)) = clicked {
        app.click_file(row, column);
    }
    if close {
        app.close_file();
    }
    if let Some((row, column)) = definition {
        app.definition_at(row, column);
    }
}

#[cfg(test)]
mod tests {
    use bevy::camera::RenderTarget;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::window::WindowRef;
    use bevy_egui::input::WindowToEguiContextMap;

    use super::*;

    #[test]
    fn a_window_made_again_gets_eguis_contexts() {
        let mut world = World::new();
        world.init_resource::<WindowToEguiContextMap>();
        let context = world
            .spawn((
                RenderTarget::Window(WindowRef::Primary),
                bevy_egui::EguiContext::default(),
            ))
            .id();
        // Mapped to the window that is gone.
        let gone = world.spawn_empty().id();
        {
            let mut map = world.resource_mut::<WindowToEguiContextMap>();
            map.context_to_window.insert(context, gone);
            map.window_to_contexts
                .entry(gone)
                .or_default()
                .insert(context);
        }
        let window = world.spawn(PrimaryWindow).id();
        world.run_system_once(follow_new_window).unwrap();
        let map = world.resource::<WindowToEguiContextMap>();
        assert_eq!(map.context_to_window[&context], window);
        assert!(map.window_to_contexts[&window].contains(&context));
        assert!(!map.window_to_contexts[&gone].contains(&context));
    }
}
