//! The App view: the project's front end running in a headless browser
//! against a back end simulated from its spec. Its page on the left, live,
//! given the person's pointer, wheel and, once clicked, keys; on the right
//! the pilot, sized by its edge and hidden or shown again with Ctrl-J or
//! its tab: how the front end runs, the states the back end answers in,
//! the workflows to show step by step, the screens, the calls of the page
//! with what serves each and how it may answer, the page's errors.
//!
//! The front ends are those the plan's map reads, kept between sessions so
//! that the workflows list at once, the map read again behind them.

use std::path::{Path, PathBuf};

use bevy_egui::egui::{
    self, Align, Align2, Color32, ColorImage, CornerRadius, CursorIcon, FontId, Id, Layout, Margin,
    PointerButton, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use universe_codemap::{Cache, CodeMap, FrontEnd, Workflow, cache_folder, example_url};
use universe_preview::{Answer, Button, Pointer, Source, Status};
use universe_ui::app_preview::Change;
use universe_ui::{App, Effect};

use crate::plan::Plan;
use crate::theme::{ACCENT, CYAN, DIM, EDGE, GREEN, LINK, PANEL, RAISED, RED, TEXT, YELLOW};

/// The pilot's width when nothing was dragged, and the least it may be.
const PILOT_WIDTH: f32 = 380.0;
const PILOT_LEAST: f32 = 280.0;

/// The header's height, the views' switch and search in it.
const HEADER: f32 = 84.0;

/// The width of the tab that shows a hidden pilot again.
const TAB: f32 = 22.0;

/// What the front ends are kept as between sessions.
const KEPT: &str = "front-ends";
const KEPT_FORMAT: &str = "1";

/// The App view's own state, beside the App's.
pub(crate) struct AppView {
    root: PathBuf,
    base: Option<PathBuf>,
    /// The front ends, as last read.
    fronts: Vec<FrontEnd>,
    /// The map's reading they come from: none for those kept from before.
    from_read: Option<u64>,
    /// The front end chosen, by its place.
    chosen: usize,
    /// The page's texture, and the frame it holds.
    page: Option<(u64, TextureHandle)>,
    /// The photographs of a workflow's steps, as textures.
    shots: Vec<TextureHandle>,
    /// The call open in the pilot, by its place among the calls.
    open_call: Option<u64>,
    /// Where the pointer was last said to be on the page.
    pointer: Option<Pos2>,
    /// The pilot's width while its edge is dragged.
    dragged: Option<f32>,
    /// The room for the page, in its pixels, and the scale it is drawn at.
    room: (u32, u32, f32),
}

impl AppView {
    pub(crate) fn new(root: PathBuf) -> Self {
        let base = crate::infra::base();
        let fronts = base
            .as_ref()
            .and_then(|b| Cache::new(&cache_folder(b, &root)).get(KEPT, KEPT_FORMAT))
            .unwrap_or_default();
        Self {
            root,
            base,
            fronts,
            from_read: None,
            chosen: 0,
            page: None,
            shots: Vec::new(),
            open_call: None,
            pointer: None,
            dragged: None,
            room: (1280, 800, 1.0),
        }
    }

    /// Whether a front end was found, as last read.
    pub(crate) fn found(&self) -> bool {
        !self.fronts.is_empty()
    }

    /// Takes the front ends of the map once read again, and keeps them for
    /// the next session.
    pub(crate) fn update(&mut self, plan: &Plan, app: &mut App) {
        let Some(map) = plan.map() else {
            app.front_end_found(self.found());
            return;
        };
        if self.from_read == Some(plan.reads()) {
            return;
        }
        self.from_read = Some(plan.reads());
        self.fronts = map.front_ends.clone();
        if self.chosen >= self.fronts.len() {
            self.chosen = 0;
        }
        if let Some(base) = &self.base {
            let _ =
                Cache::new(&cache_folder(base, &self.root)).put(KEPT, KEPT_FORMAT, &self.fronts);
        }
        app.front_end_found(self.found());
    }
}

/// Draws the App view.
pub(crate) fn show(
    ui: &mut Ui,
    view: &mut AppView,
    plan: &Plan,
    app: &mut App,
    effects: &mut Vec<Effect>,
) {
    let rect = ui.max_rect();
    let painter = ui.painter_at(rect);
    let name = view
        .fronts
        .get(view.chosen)
        .map_or("no front end", |f| f.name.as_str());
    painter.text(
        rect.left_top() + vec2(24.0, 22.0),
        Align2::LEFT_TOP,
        "App",
        FontId::proportional(26.0),
        TEXT,
    );
    painter.text(
        rect.left_top() + vec2(84.0, 30.0),
        Align2::LEFT_TOP,
        name,
        FontId::proportional(15.0),
        ACCENT,
    );
    painter.text(
        rect.left_top() + vec2(24.0, 60.0),
        Align2::LEFT_TOP,
        status_line(app),
        FontId::proportional(12.5),
        DIM,
    );
    crate::plan::switch(ui, rect, app);

    let body = Rect::from_min_max(rect.min + vec2(0.0, HEADER), rect.max);
    if view.fronts.is_empty() {
        let text = if plan.is_reading() {
            "Reading the project…"
        } else {
            "No front end found: a package whose router serves screens (Next.js, React Router, TanStack Router, a Flutter app)"
        };
        ui.painter().text(
            body.center(),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(16.0),
            DIM,
        );
        if plan.is_reading() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        return;
    }

    let shown = app.app_preview().pilot_shown();
    let wanted = view.dragged.or(app.pilot_width()).unwrap_or(PILOT_WIDTH);
    let most = (body.width() * 0.6).max(PILOT_LEAST);
    let width = wanted.clamp(PILOT_LEAST, most);
    let (page_area, pilot_area) = if shown {
        let split = body.max.x - width;
        (
            Rect::from_min_max(body.min, pos2(split - 1.0, body.max.y)),
            Some(Rect::from_min_max(pos2(split, body.min.y), body.max)),
        )
    } else {
        (
            Rect::from_min_max(body.min, pos2(body.max.x - TAB, body.max.y)),
            None,
        )
    };

    page(ui, view, page_area.shrink2(vec2(12.0, 8.0)), app, effects);

    match pilot_area {
        Some(area) => {
            // The edge, dragged to size it.
            let edge = Rect::from_min_max(
                pos2(area.min.x - 3.0, area.min.y),
                pos2(area.min.x + 3.0, area.max.y),
            );
            let grip = ui.interact(edge, Id::new("pilot-edge"), Sense::drag());
            if grip.hovered() || grip.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if grip.dragged() {
                let delta = grip.drag_delta().x;
                view.dragged = Some((width - delta).clamp(PILOT_LEAST, most));
            } else if let Some(dragged) = view.dragged.take() {
                app.set_pilot_width(dragged);
            }
            ui.painter().line_segment(
                [area.left_top(), area.left_bottom()],
                Stroke::new(1.0, EDGE),
            );
            ui.painter().rect_filled(area, CornerRadius::ZERO, PANEL);
            let inner = area.shrink2(vec2(14.0, 10.0));
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::top_down(Align::Min)),
                |ui| {
                    pilot(ui, view, plan.map(), app, effects);
                },
            );
        }
        None => {
            let tab = Rect::from_min_max(
                pos2(body.max.x - TAB, body.min.y + 40.0),
                pos2(body.max.x, body.min.y + 160.0),
            );
            let response = ui.interact(tab, Id::new("pilot-tab"), Sense::click());
            let fill = if response.hovered() { RAISED } else { PANEL };
            ui.painter().rect_filled(
                tab,
                CornerRadius {
                    nw: 6,
                    sw: 6,
                    ne: 0,
                    se: 0,
                },
                fill,
            );
            ui.painter().text(
                tab.center_top() + vec2(0.0, 14.0),
                Align2::CENTER_CENTER,
                "◀",
                FontId::proportional(12.0),
                ACCENT,
            );
            for (i, c) in "Pilot".chars().enumerate() {
                ui.painter().text(
                    tab.center_top() + vec2(0.0, 36.0 + i as f32 * 15.0),
                    Align2::CENTER_CENTER,
                    c,
                    FontId::proportional(12.5),
                    TEXT,
                );
            }
            if response.on_hover_text("Show the pilot (Ctrl-J)").clicked() {
                app.toggle_pilot();
            }
        }
    }
}

/// What the preview is doing, in a line.
fn status_line(app: &App) -> String {
    let preview = app.app_preview();
    match preview.status() {
        None => {
            "Not started: its front end and a back end simulated from its spec run on this machine"
                .to_owned()
        }
        Some(Status::Reading) => "Reading the spec…".to_owned(),
        Some(Status::Starting { api, line }) => {
            format!("Starting `{line}`, its API simulated on port {api}…")
        }
        Some(Status::Opening) => "Opening it in the browser…".to_owned(),
        Some(Status::Running { .. }) if preview.is_focused() => {
            "The keys go to the page: Esc gives them back".to_owned()
        }
        Some(Status::Running { address }) => {
            format!("Running at {address}, its API simulated: click the page to use it")
        }
        Some(Status::Failed(why)) => format!("Stopped: {}", why.lines().next().unwrap_or_default()),
        Some(Status::Stopped) => "Stopped".to_owned(),
    }
}

/// The page, live: its last frame, given the person's pointer and wheel.
fn page(ui: &mut Ui, view: &mut AppView, area: Rect, app: &mut App, effects: &mut Vec<Effect>) {
    let scale = ui.pixels_per_point();
    // The page is as large as the room for it, in its own pixels.
    view.room = (
        area.width().round() as u32,
        area.height().round() as u32,
        scale,
    );
    if let Some(effect) = app.app_size(view.room.0, view.room.1, scale) {
        effects.push(effect);
    }
    let preview = app.app_preview();
    let painter = ui.painter_at(area);
    painter.rect_filled(area, CornerRadius::same(8), Color32::from_rgb(8, 9, 12));
    let Some(frame) = preview.frame().filter(|_| preview.is_on()) else {
        let (title, said) = match preview.status() {
            Some(Status::Failed(why)) => ("It stopped", why.clone()),
            Some(Status::Stopped) | None => (
                "Not running",
                "Start it in the pilot: its command shows on the button".to_owned(),
            ),
            Some(_) => ("Starting…", status_line(app)),
        };
        painter.text(
            area.center() - vec2(0.0, 18.0),
            Align2::CENTER_CENTER,
            title,
            FontId::proportional(20.0),
            TEXT,
        );
        let galley = painter.layout(
            said,
            FontId::proportional(13.0),
            DIM,
            (area.width() - 80.0).max(200.0),
        );
        painter.galley(
            area.center() + vec2(-galley.size().x / 2.0, 6.0),
            galley,
            DIM,
        );
        if preview.is_on() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        return;
    };
    // The frame's texture, made again only for a new frame.
    let size = [frame.width as usize, frame.height as usize];
    match &mut view.page {
        Some((seq, texture)) if *seq != frame.seq => {
            texture.set(
                ColorImage::from_rgba_unmultiplied(size, &frame.rgba),
                TextureOptions::LINEAR,
            );
            *seq = frame.seq;
        }
        Some(_) => {}
        None => {
            let texture = ui.ctx().load_texture(
                "app-page",
                ColorImage::from_rgba_unmultiplied(size, &frame.rgba),
                TextureOptions::LINEAR,
            );
            view.page = Some((frame.seq, texture));
        }
    }
    // Drawn whole, its sides kept, at its own size when there is room.
    let points = vec2(frame.width as f32, frame.height as f32) / scale;
    let fit = (area.width() / points.x)
        .min(area.height() / points.y)
        .min(1.0);
    let drawn = Rect::from_min_size(area.min, points * fit);
    if let Some((_, texture)) = &view.page {
        painter.image(
            texture.id(),
            drawn,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let focused = preview.is_focused();
    painter.rect_stroke(
        drawn,
        CornerRadius::same(2),
        Stroke::new(
            if focused { 2.0 } else { 1.0 },
            if focused { ACCENT } else { EDGE },
        ),
        StrokeKind::Inside,
    );

    // Where on the page, in its own pixels.
    let on_page = |at: Pos2| -> (f32, f32) {
        let x = (at.x - drawn.min.x) / drawn.width() * points.x;
        let y = (at.y - drawn.min.y) / drawn.height() * points.y;
        (x, y)
    };
    let response = ui.interact(drawn, Id::new("app-page"), Sense::click_and_drag());
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Default);
    }
    let input = ui.input(|i| {
        (
            i.pointer.hover_pos(),
            i.pointer.button_pressed(PointerButton::Primary),
            i.pointer.button_released(PointerButton::Primary),
            i.pointer.button_pressed(PointerButton::Secondary),
            i.pointer.button_released(PointerButton::Secondary),
            i.pointer.button_double_clicked(PointerButton::Primary),
            i.smooth_scroll_delta,
        )
    });
    let (hover, pressed, released, menu_pressed, menu_released, double, scroll) = input;
    let Some(at) = hover.filter(|p| drawn.contains(*p) || response.dragged()) else {
        view.pointer = None;
        return;
    };
    let (x, y) = on_page(at);
    let mut send = |pointer: Pointer| {
        if let Some(effect) = app.app_pointer(pointer) {
            effects.push(effect);
        }
    };
    if view.pointer != Some(at) {
        view.pointer = Some(at);
        send(Pointer::Move { x, y });
    }
    let clicks = if double { 2 } else { 1 };
    if pressed {
        send(Pointer::Press {
            x,
            y,
            button: Button::Left,
            clicks,
        });
    }
    if released {
        send(Pointer::Release {
            x,
            y,
            button: Button::Left,
            clicks,
        });
    }
    if menu_pressed {
        send(Pointer::Press {
            x,
            y,
            button: Button::Right,
            clicks: 1,
        });
    }
    if menu_released {
        send(Pointer::Release {
            x,
            y,
            button: Button::Right,
            clicks: 1,
        });
    }
    if scroll != egui::Vec2::ZERO && drawn.contains(at) {
        send(Pointer::Wheel {
            x,
            y,
            dx: -scroll.x,
            dy: -scroll.y,
        });
    }
}

/// A heading of the pilot.
fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text).size(12.0).color(DIM).strong());
    ui.add_space(2.0);
}

/// The pilot: how the front end runs, its states, workflows, screens and
/// calls.
fn pilot(
    ui: &mut Ui,
    view: &mut AppView,
    map: Option<&CodeMap>,
    app: &mut App,
    effects: &mut Vec<Effect>,
) {
    // Its rows are clicked, not their text selected.
    ui.style_mut().interaction.selectable_labels = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Pilot").size(17.0).color(TEXT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .small_button("▶")
                .on_hover_text("Hide the pilot (Ctrl-J)")
                .clicked()
            {
                app.toggle_pilot();
            }
        });
    });
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            running(ui, view, map, app, effects);
            if app.app_preview().is_running() {
                address(ui, app, effects);
                states(ui, app, effects);
            }
            workflows(ui, view, app, effects);
            screens(ui, view, app, effects);
            if app.app_preview().is_running() {
                calls(ui, view, map, app, effects);
            }
            errors(ui, app);
        });
}

/// How the front end runs: which, the command it is run with, the spec,
/// the button that starts or stops it.
fn running(
    ui: &mut Ui,
    view: &mut AppView,
    map: Option<&CodeMap>,
    app: &mut App,
    effects: &mut Vec<Effect>,
) {
    let settings = crate::infra::base()
        .map(|base| universe_codemap::settings_file(&base, &view.root))
        .map(|file| {
            let home = std::env::var_os("HOME").map_or_else(|| view.root.clone(), PathBuf::from);
            universe_codemap::read_settings(&file, &home).map(|s| s.app)
        });
    let settings = match settings {
        Some(Ok(settings)) => settings,
        Some(Err(e)) => {
            ui.label(RichText::new(e.to_string()).size(12.0).color(RED));
            universe_codemap::AppSettings::default()
        }
        None => universe_codemap::AppSettings::default(),
    };
    // The front end the settings name, else the one chosen.
    if let Some(folder) = &settings.folder
        && let Some(i) = view.fronts.iter().position(|f| &f.folder == folder)
    {
        view.chosen = i;
    }
    if view.fronts.len() > 1 {
        let current = view
            .fronts
            .get(view.chosen)
            .map_or("", |f| f.name.as_str())
            .to_owned();
        egui::ComboBox::from_id_salt("front-end")
            .selected_text(RichText::new(current).size(13.0).color(TEXT))
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                for (i, front) in view.fronts.iter().enumerate() {
                    let label = format!("{}  {}", front.name, front.folder);
                    if ui.selectable_label(view.chosen == i, label).clicked() {
                        view.chosen = i;
                    }
                }
            });
    }
    let Some(front) = view.fronts.get(view.chosen).cloned() else {
        return;
    };
    let config = config(view, &front, &settings, map);
    let on = app.app_preview().is_on();
    ui.add_space(6.0);
    match &config {
        Ok(config) => {
            let line = universe_preview::command_shown(&config.run);
            let folder = if front.folder.is_empty() {
                ".".to_owned()
            } else {
                front.folder.clone()
            };
            ui.label(RichText::new(format!("in {folder}")).size(11.5).color(DIM));
            ui.label(RichText::new(&line).monospace().size(12.0).color(YELLOW));
            ui.label(
                RichText::new(spec_said(&config.spec, &view.root))
                    .size(11.5)
                    .color(DIM),
            );
        }
        Err(why) => {
            ui.label(RichText::new(why).size(12.0).color(RED));
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if on {
            if ui.button(RichText::new("■ Stop").color(RED)).clicked() {
                effects.push(app.stop_app());
            }
        } else if let Ok(config) = config
            && ui
                .button(RichText::new("▶ Start").color(GREEN))
                .on_hover_text("Runs the command above, a back end simulated from the spec, and a headless browser")
                .clicked()
        {
            view.page = None;
            effects.push(app.start_app(config));
        }
    });
}

/// How the spec is had, in a line.
fn spec_said(spec: &universe_preview::SpecSource, root: &Path) -> String {
    match spec {
        universe_preview::SpecSource::File(path) => {
            format!(
                "API simulated from {}",
                path.strip_prefix(root).unwrap_or(path).display()
            )
        }
        universe_preview::SpecSource::Command(command) => {
            format!("API simulated from what `{command}` prints")
        }
        universe_preview::SpecSource::Text(_) => "API simulated from its spec".to_owned(),
    }
}

/// How the chosen front end runs, from what was read of it and the
/// project's settings.
fn config(
    view: &AppView,
    front: &FrontEnd,
    settings: &universe_codemap::AppSettings,
    map: Option<&CodeMap>,
) -> Result<universe_preview::Config, String> {
    let root = &view.root;
    let spec = if let Some(spec) = &settings.spec {
        let path = PathBuf::from(spec);
        universe_preview::SpecSource::File(if path.is_absolute() {
            path
        } else {
            root.join(path)
        })
    } else if let Some(command) = &settings.spec_command {
        universe_preview::SpecSource::Command(command.clone())
    } else {
        let map = map.ok_or("Reading the project…")?;
        let spec = map
            .operations
            .iter()
            .find(|o| !o.from_code)
            .map(|o| map.nodes[o.spec].path.clone())
            .ok_or_else(|| {
                let file = view
                    .base
                    .as_ref()
                    .map(|b| universe_codemap::settings_file(b, root).display().to_string())
                    .unwrap_or_default();
                format!(
                    "No OpenAPI spec in the project to simulate its API from: name one as `spec`, or a command that prints it as `spec_command`, in the [app] table of {file}"
                )
            })?;
        universe_preview::SpecSource::File(root.join(spec))
    };
    let framework = match front.framework {
        universe_codemap::Framework::Next => universe_preview::Framework::Next,
        universe_codemap::Framework::Vite => universe_preview::Framework::Vite,
        universe_codemap::Framework::Flutter => universe_preview::Framework::Flutter,
        universe_codemap::Framework::Other => universe_preview::Framework::Other,
    };
    let flutter = front.framework == universe_codemap::Framework::Flutter;
    if settings.command.is_none() && front.dev.is_none() && !flutter {
        return Err(format!(
            "{} has no `dev` script: name the command that runs it as `command` in the [app] table of the project's settings",
            front.name
        ));
    }
    let cookies: Vec<(String, String)> = if settings.cookies.is_empty() {
        front
            .cookies
            .iter()
            .map(|c| (c.clone(), "simulated".to_owned()))
            .collect()
    } else {
        settings
            .cookies
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    Ok(universe_preview::Config {
        root: root.clone(),
        run: universe_preview::Run {
            folder: if front.folder.is_empty() {
                root.clone()
            } else {
                root.join(&front.folder)
            },
            command: settings.command.clone(),
            runner: front.runner.clone(),
            framework,
            env: settings
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            own_env: front.own_env.clone(),
        },
        api_env: settings.api_env.clone().or_else(|| front.api_env.clone()),
        spec,
        fixtures: view
            .base
            .as_ref()
            .map(|b| cache_folder(b, root).join("app-fixtures")),
        cookies,
        start: example_url(&start_route(front)),
        size: view.room,
        leftovers: view
            .base
            .as_ref()
            .map(|b| universe_preview::leftovers_file(b, root)),
        tests: view
            .base
            .as_ref()
            .map(|b| cache_folder(b, root).join("app-tests")),
    })
}

/// The route a front end opens on: where most of its workflows start, else
/// the screen that links to most.
fn start_route(front: &FrontEnd) -> String {
    let mut counts: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    for flow in &front.workflows {
        if let Some(first) = flow.steps.first() {
            *counts.entry(*first).or_default() += 1;
        }
    }
    let best = counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(screen, _)| screen)
        .or_else(|| {
            (0..front.screens.len()).max_by_key(|&i| {
                (
                    front.screens[i].links.len(),
                    usize::MAX - front.screens[i].route.len(),
                )
            })
        });
    best.and_then(|i| front.screens.get(i))
        .map_or_else(|| "/".to_owned(), |s| s.route.clone())
}

/// The page's address, and the way back, forward and again.
fn address(ui: &mut Ui, app: &mut App, effects: &mut Vec<Effect>) {
    heading(ui, "PAGE");
    let preview = app.app_preview();
    let address = preview.address().to_owned();
    let shown = address
        .split_once("://")
        .map_or(address.as_str(), |(_, rest)| {
            rest.find('/').map_or("/", |i| &rest[i..])
        });
    ui.label(RichText::new(shown).monospace().size(12.0).color(LINK));
    ui.horizontal(|ui| {
        let mut push = |effect: Option<Effect>| {
            if let Some(effect) = effect {
                effects.push(effect);
            }
        };
        if ui.small_button("◀").on_hover_text("Back").clicked() {
            push(app.app_history(true));
        }
        if ui.small_button("▶").on_hover_text("Forward").clicked() {
            push(app.app_history(false));
        }
        if ui
            .small_button("⟳")
            .on_hover_text("Load the page again")
            .clicked()
        {
            push(app.app_reload());
        }
        if ui
            .small_button("📷")
            .on_hover_text("Photograph the page")
            .clicked()
        {
            push(app.app_shoot("page"));
        }
    });
}

/// The states the back end answers in, each a switch.
fn states(ui: &mut Ui, app: &mut App, effects: &mut Vec<Effect>) {
    heading(ui, "STATE OF THE BACK END");
    let state = app.app_preview().state().clone();
    let switches = [
        (
            "Empty lists",
            "every list answers no item; who is there still answers",
            state.tone.empty,
            Change::Empty(!state.tone.empty),
        ),
        (
            "Slow",
            "every call takes 1.5 s: what the page shows meanwhile",
            state.slow,
            Change::Slow(!state.slow),
        ),
        (
            "Failing",
            "every call fails with a 500 but who is there",
            state.failing,
            Change::Failing(!state.failing),
        ),
        (
            "No rights",
            "no permission, the least role",
            state.tone.no_rights,
            Change::NoRights(!state.tone.no_rights),
        ),
        (
            "Flags on",
            "every flag of a feature on",
            state.tone.flags_on,
            Change::FlagsOn(!state.tone.flags_on),
        ),
    ];
    ui.horizontal_wrapped(|ui| {
        for (label, hint, on, change) in switches {
            let text = RichText::new(label)
                .size(12.5)
                .color(if on { ACCENT } else { TEXT });
            if ui
                .add(egui::Button::selectable(on, text))
                .on_hover_text(hint)
                .clicked()
                && let Some(effect) = app.app_change(change)
            {
                effects.push(effect);
                // What the page shows is the old state's until loaded again.
                if let Some(effect) = app.app_reload() {
                    effects.push(effect);
                }
            }
        }
        let changed = state != universe_preview::State::default();
        if changed
            && ui
                .small_button("As the spec")
                .on_hover_text("Every state off, every operation as the spec says")
                .clicked()
        {
            if let Some(effect) = app.app_change(Change::Reset) {
                effects.push(effect);
            }
            if let Some(effect) = app.app_reload() {
                effects.push(effect);
            }
        }
    });
}

/// The workflows, by group: one clicked is shown screen after screen.
fn workflows(ui: &mut Ui, view: &mut AppView, app: &mut App, effects: &mut Vec<Effect>) {
    let Some(front) = view.fronts.get(view.chosen).cloned() else {
        return;
    };
    heading(ui, &format!("WORKFLOWS · {}", front.workflows.len()));
    if let Some(playing) = app.app_preview().playing().cloned() {
        let done = app.app_preview().shots().len();
        egui::Frame::NONE
            .fill(RAISED)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.label(RichText::new(&playing.name).size(13.5).color(TEXT));
                for (i, step) in playing.steps.iter().enumerate() {
                    let mark = if i < done {
                        "✓"
                    } else if i == done {
                        "…"
                    } else {
                        "·"
                    };
                    ui.label(
                        RichText::new(format!("{mark} {step}"))
                            .size(11.5)
                            .color(if i < done { GREEN } else { DIM }),
                    );
                }
                let (said, colour) = if playing.acted {
                    (
                        format!("✓ {} {} called", playing.method, playing.path),
                        GREEN,
                    )
                } else if done >= playing.steps.len() && playing.replay.is_some() {
                    ("Doing again what was done on the page…".to_owned(), DIM)
                } else if done >= playing.steps.len() {
                    (
                        format!(
                            "Now do it on the page: {} {} is looked for",
                            playing.method, playing.path
                        ),
                        YELLOW,
                    )
                } else {
                    ("Showing its screens…".to_owned(), DIM)
                };
                ui.label(RichText::new(said).size(12.0).color(colour));
                if playing.replay.is_some() {
                    compared(ui, app);
                }
                shots(ui, view, app);
                if app.app_preview().shown_whole()
                    && ui
                        .button(RichText::new("Keep as a test").size(12.0))
                        .on_hover_text(
                            "Its screens, the state, and what you did on the page since its last screen showed: shown again, each screen is compared with these and what you did is done again",
                        )
                        .clicked()
                    && let Some(effect) = app.app_keep_test()
                {
                    effects.push(effect);
                }
            });
    }
    kept_tests(ui, app, effects);
    let mut groups: Vec<(&str, Vec<&Workflow>)> = Vec::new();
    for flow in &front.workflows {
        match groups.iter_mut().find(|(g, _)| *g == flow.group) {
            Some((_, flows)) => flows.push(flow),
            None => groups.push((&flow.group, vec![flow])),
        }
    }
    let running = app.app_preview().is_running();
    for (group, flows) in groups {
        egui::CollapsingHeader::new(
            RichText::new(format!("{group} · {}", flows.len()))
                .size(12.5)
                .color(TEXT),
        )
        .id_salt(("workflows", group))
        .default_open(false)
        .show(ui, |ui| {
            for flow in flows {
                let text = RichText::new(format!("{}  ·  {} steps", flow.name, flow.steps.len()))
                    .size(12.0);
                let tip = flow
                    .steps
                    .iter()
                    .filter_map(|&s| front.screens.get(s).map(|s| s.route.as_str()))
                    .collect::<Vec<_>>()
                    .join(" → ");
                let response = ui.add_enabled(running, egui::Button::new(text).frame(false));
                if response
                    .on_hover_text(format!("{tip}\nthen {} {}", flow.method, flow.path))
                    .clicked()
                {
                    let steps: Vec<(String, String)> = flow
                        .steps
                        .iter()
                        .filter_map(|&s| front.screens.get(s))
                        .map(|s| (s.route.clone(), example_url(&s.route)))
                        .collect();
                    view.shots.clear();
                    if let Some(effect) =
                        app.app_play(&flow.name, (&flow.method, &flow.path), steps)
                    {
                        effects.push(effect);
                    }
                }
            }
        });
    }
}

/// How the test shown again compared with what was kept: each screen, and
/// whether its action was called again.
fn compared(ui: &mut Ui, app: &App) {
    let preview = app.app_preview();
    for (label, changed) in preview.compared() {
        let (said, colour) = match changed {
            Some(c) if *c < 0.01 => ("✓ the same".to_owned(), GREEN),
            Some(c) => (format!("≠ {:.0}% changed", c * 100.0), YELLOW),
            None => ("no picture to compare".to_owned(), DIM),
        };
        ui.label(
            RichText::new(format!("{label}: {said}"))
                .size(11.5)
                .color(colour),
        );
    }
    let Some(playing) = preview.playing() else {
        return;
    };
    let expected = playing
        .replay
        .and_then(|i| preview.tests().get(i))
        .is_some_and(|k| k.test.acted);
    let ended = preview.compared().iter().any(|(label, _)| label == "after");
    if expected && ended {
        let (said, colour) = if playing.acted {
            (
                format!("✓ {} {} called again", playing.method, playing.path),
                GREEN,
            )
        } else {
            (
                format!("✗ {} {} not called this time", playing.method, playing.path),
                RED,
            )
        };
        ui.label(RichText::new(said).size(12.0).color(colour));
    }
}

/// The workflows kept as tests, each shown again by a click.
fn kept_tests(ui: &mut Ui, app: &mut App, effects: &mut Vec<Effect>) {
    let tests: Vec<(String, usize, bool)> = app
        .app_preview()
        .tests()
        .iter()
        .map(|k| (k.test.name.clone(), k.test.steps.len(), k.test.acted))
        .collect();
    if tests.is_empty() {
        return;
    }
    heading(ui, &format!("KEPT TESTS · {}", tests.len()));
    let running = app.app_preview().is_running();
    for (i, (name, steps, acted)) in tests.into_iter().enumerate() {
        ui.horizontal(|ui| {
            let shown = ui
                .add_enabled(
                    running,
                    egui::Button::new(RichText::new("▶").size(11.0)).small(),
                )
                .on_hover_text(
                    "Show it again: its screens compared with those kept, what was done done again",
                );
            if shown.clicked()
                && let Some(effect) = app.app_replay(i)
            {
                effects.push(effect);
            }
            let what = if acted { "acts" } else { "screens only" };
            ui.label(
                RichText::new(format!("{name} · {steps} screens · {what}"))
                    .size(12.0)
                    .color(TEXT),
            );
        });
    }
}

/// The photographs of a workflow's steps, small, side by side.
fn shots(ui: &mut Ui, view: &mut AppView, app: &App) {
    let shots = app.app_preview().shots();
    while view.shots.len() < shots.len() {
        let shot = &shots[view.shots.len()];
        let Ok(picture) = image::load_from_memory_with_format(&shot.png, image::ImageFormat::Png)
        else {
            break;
        };
        let rgba = picture.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        view.shots.push(ui.ctx().load_texture(
            format!("app-shot-{}", view.shots.len()),
            ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
            TextureOptions::LINEAR,
        ));
    }
    if view.shots.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (texture, shot) in view.shots.iter().zip(shots) {
            let [w, h] = texture.size();
            let width = 100.0;
            let size = vec2(width, width * h as f32 / w.max(1) as f32);
            ui.add(egui::Image::new((texture.id(), size)).corner_radius(3))
                .on_hover_text(&shot.label);
        }
    });
}

/// The screens, each opened by a click.
fn screens(ui: &mut Ui, view: &mut AppView, app: &mut App, effects: &mut Vec<Effect>) {
    let Some(front) = view.fronts.get(view.chosen) else {
        return;
    };
    let running = app.app_preview().is_running();
    egui::CollapsingHeader::new(
        RichText::new(format!("SCREENS · {}", front.screens.len()))
            .size(12.0)
            .color(DIM)
            .strong(),
    )
    .id_salt("screens")
    .default_open(false)
    .show(ui, |ui| {
        for screen in &front.screens {
            let text = RichText::new(&screen.route)
                .monospace()
                .size(11.5)
                .color(LINK);
            let response = ui.add_enabled(running, egui::Button::new(text).frame(false));
            let tip = format!(
                "{} · {} calls · links to {}",
                screen.file,
                screen.calls.len(),
                screen.links.len()
            );
            if response.on_hover_text(tip).clicked()
                && let Some(effect) = app.app_go(&example_url(&screen.route))
            {
                effects.push(effect);
            }
        }
    });
}

/// The calls of the page shown: each with its operation, what serves it,
/// how it answers, and the way to make it answer otherwise.
fn calls(
    ui: &mut Ui,
    view: &mut AppView,
    map: Option<&CodeMap>,
    app: &mut App,
    effects: &mut Vec<Effect>,
) {
    let calls: Vec<universe_preview::Call> = app.app_preview().page_calls().cloned().collect();
    heading(ui, &format!("CALLS OF THE PAGE · {}", calls.len()));
    let operations = app.app_preview().operations().to_vec();
    let answers = app.app_preview().state().answers.clone();
    for call in calls.iter().rev().take(60) {
        let open = view.open_call == Some(call.seq);
        let status = if call.status >= 400 { RED } else { GREEN };
        let source = match call.source {
            Source::Schema => ("schema", DIM),
            Source::Fixture => ("fixture", CYAN),
            Source::Changed => ("changed", YELLOW),
            Source::Missing => ("not in the spec", RED),
        };
        let path = call.path.split('?').next().unwrap_or(&call.path);
        let response = ui
            .horizontal(|ui| {
                ui.label(
                    RichText::new(&call.method)
                        .monospace()
                        .size(11.0)
                        .color(crate::api_view::method_colour(&call.method)),
                );
                ui.label(
                    RichText::new(call.status.to_string())
                        .monospace()
                        .size(11.0)
                        .color(status),
                );
                ui.label(RichText::new(source.0).size(10.5).color(source.1));
                // A long path cut short, whole on hovering.
                ui.add(
                    egui::Label::new(RichText::new(path).monospace().size(11.0).color(TEXT))
                        .truncate(),
                )
                .on_hover_text(&call.path);
            })
            .response
            .interact(Sense::click());
        if response.clicked() {
            view.open_call = if open { None } else { Some(call.seq) };
        }
        if !open {
            continue;
        }
        let Some(index) = call.operation else {
            ui.label(RichText::new("No operation of the spec has this path: the page calls what the API does not say it serves").size(11.5).color(DIM));
            continue;
        };
        let Some(operation) = operations.get(index) else {
            continue;
        };
        egui::Frame::NONE
            .fill(RAISED)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                if !operation.summary.is_empty() {
                    ui.label(RichText::new(&operation.summary).size(12.5).color(TEXT));
                }
                ui.label(RichText::new(format!("{} {} · {} ms", operation.method, operation.path, call.took.as_millis())).monospace().size(11.0).color(DIM));
                // What serves it, as the map read it.
                let served = map.and_then(|m| {
                    m.operations
                        .iter()
                        .find(|o| o.method == operation.method && o.path == operation.path)
                        .and_then(|o| o.handler)
                        .map(|(file, line)| (m.nodes[file].path.clone(), line))
                });
                if let Some((file, line)) = served
                    && ui
                        .add(egui::Button::new(RichText::new(format!("served by {file}:{line}")).size(11.5).color(LINK)).frame(false))
                        .on_hover_text("Open it")
                        .clicked()
                {
                    app.open_from_app(view.root.join(&file), line);
                }
                ui.label(RichText::new("answer with").size(11.0).color(DIM));
                ui.horizontal_wrapped(|ui| {
                    let chosen = answers.get(&index);
                    let mut choose = |ui: &mut Ui, label: String, answer: Option<Answer>| {
                        let on = chosen == answer.as_ref();
                        let text = RichText::new(label).size(11.5).color(if on { ACCENT } else { TEXT });
                        if ui.add(egui::Button::selectable(on, text)).clicked()
                            && let Some(effect) = app.app_change(Change::Answer(index, answer))
                        {
                            effects.push(effect);
                        }
                    };
                    choose(ui, "as the state".to_owned(), None);
                    for status in &operation.statuses {
                        if let Ok(code) = status.parse::<u16>()
                            && !status.starts_with('2')
                        {
                            choose(ui, status.clone(), Some(Answer::Status(code)));
                        }
                    }
                    if !operation.statuses.iter().any(|s| s == "500") {
                        choose(ui, "500".to_owned(), Some(Answer::Status(500)));
                    }
                    choose(ui, "empty".to_owned(), Some(Answer::Empty));
                    choose(ui, "slow".to_owned(), Some(Answer::Slow));
                });
                if ui
                    .button(RichText::new("Keep as a fixture").size(11.5))
                    .on_hover_text("Its last answer, kept in a file opened here: change it, and it answers so from then on")
                    .clicked()
                    && let Some(effect) = app.app_keep(index)
                {
                    effects.push(effect);
                }
            });
    }
}

/// The page's errors, and the front end's last lines.
fn errors(ui: &mut Ui, app: &App) {
    let preview = app.app_preview();
    let errors: Vec<&String> = preview.errors().collect();
    if !errors.is_empty() {
        heading(ui, &format!("ERRORS OF THE PAGE · {}", errors.len()));
        // The same error again is counted, not written again.
        let mut counted: Vec<(&String, usize)> = Vec::new();
        for error in errors.iter().rev() {
            match counted.iter_mut().find(|(e, _)| e == error) {
                Some((_, n)) => *n += 1,
                None => counted.push((error, 1)),
            }
        }
        for (error, times) in counted.into_iter().take(20) {
            let error = if times > 1 {
                format!("×{times}  {error}")
            } else {
                error.clone()
            };
            ui.add(
                egui::Label::new(
                    RichText::new(error.as_str())
                        .monospace()
                        .size(11.0)
                        .color(RED),
                )
                .wrap(),
            );
        }
    }
    let said: Vec<&String> = preview.said().collect();
    if !said.is_empty() {
        egui::CollapsingHeader::new(
            RichText::new(format!("FRONT END'S OUTPUT · {}", said.len()))
                .size(12.0)
                .color(DIM)
                .strong(),
        )
        .id_salt("said")
        .default_open(matches!(preview.status(), Some(Status::Failed(_))))
        .show(ui, |ui| {
            for line in said.iter().rev().take(40).rev() {
                ui.add(
                    egui::Label::new(
                        RichText::new(line.as_str())
                            .monospace()
                            .size(10.5)
                            .color(DIM),
                    )
                    .wrap(),
                );
            }
        });
    }
}
