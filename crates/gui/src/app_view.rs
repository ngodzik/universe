//! The App view: the project's front end running in the system's Chrome
//! against a back end simulated from its spec. Its page on the left, in
//! Chrome's own window held over this one's, used there as in any
//! browser; the wait drawn until its first page has loaded. On the right
//! the pilot, sized by its edge and hidden or shown again with Ctrl-J or
//! its tab: how the front end runs, the states the back end answers in,
//! the workflows to show step by step, the screens, the calls of the page
//! with what serves each and how it may answer, the page's errors.
//!
//! The front ends are those the plan's map reads, kept between sessions so
//! that the workflows list at once, the map read again behind them.

use std::path::{Path, PathBuf};

use bevy_egui::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Layout, Margin, Rect,
    RichText, ScrollArea, Sense, Stroke, Ui, UiBuilder, pos2, vec2,
};
use universe_codemap::{Cache, CodeMap, FrontEnd, Workflow, cache_folder, example_url};
use universe_preview::{Answer, Source, Status};
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
    /// The call open in the pilot, by its place among the calls.
    open_call: Option<u64>,
    /// The pilot's width while its edge is dragged.
    dragged: Option<f32>,
    /// How the chosen front end runs: its folder, its command, its spec.
    how: Option<(String, String, String)>,
    /// Where universe's window is on the screen, as the window last said.
    pub(crate) screen: Option<Screen>,
}

/// Where universe's window is on the screen, for Chrome's to be placed
/// over its page area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Screen {
    /// The top left of what it draws, in the screen's pixels.
    pub(crate) left: f64,
    pub(crate) top: f64,
    /// The screen's pixels to a device-independent one.
    pub(crate) scale: f64,
    /// Whether it is minimized: Chrome's window then hides too.
    pub(crate) minimized: bool,
    /// Its window on X11, which Chrome's is held over.
    pub(crate) parent: Option<u64>,
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
            open_call: None,
            dragged: None,
            how: None,
            screen: None,
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
        Some(Status::Opening) => "Opening it in Chrome…".to_owned(),
        Some(Status::Running { .. }) if !preview.page_shown() => {
            format!("Building {}…", preview.loading_page())
        }
        Some(Status::Running { address }) => {
            format!("Running at {address}, its API simulated: use the page as in any browser")
        }
        Some(Status::Failed(why)) => format!("Stopped: {}", why.lines().next().unwrap_or_default()),
        Some(Status::Stopped) => "Stopped".to_owned(),
    }
}

/// The page: Chrome's window over this area once its first page has
/// loaded, the wait drawn until then.
fn page(ui: &mut Ui, view: &AppView, area: Rect, app: &mut App, effects: &mut Vec<Effect>) {
    let preview = app.app_preview();
    let painter = ui.painter_at(area);
    painter.rect_filled(area, CornerRadius::same(8), Color32::from_rgb(8, 9, 12));
    if !preview.is_on() {
        let (title, said) = match preview.status() {
            Some(Status::Failed(why)) => ("It stopped", why.clone()),
            _ => (
                "Not running",
                "Start it in the pilot, on the right: the page opens here, in Chrome".to_owned(),
            ),
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
        return;
    }
    if !preview.page_shown() {
        let deep = app.bullets() != universe_ui::markdown::Bullets::Plain;
        waiting(ui, &painter, area, app, deep);
        ui.ctx().request_repaint();
        return;
    }
    // Under Chrome's window, seen only while it catches up with a move.
    painter.text(
        area.center(),
        Align2::CENTER_CENTER,
        "The page shows here, in Chrome",
        FontId::proportional(14.0),
        DIM,
    );
    let place = view
        .screen
        .filter(|screen| !screen.minimized)
        .map(|screen| place(area, ui.pixels_per_point(), screen));
    if let Some(effect) = app.app_place(place) {
        effects.push(effect);
    }
}

/// Where Chrome's page goes for this area to show it: from the window's
/// points to the screen's pixels, then to its device-independent ones.
fn place(area: Rect, pixels_per_point: f32, screen: Screen) -> universe_preview::Place {
    let pixels = |points: f32| f64::from(points) * f64::from(pixels_per_point);
    let scale = screen.scale.max(0.1);
    let dip = |pixels: f64| (pixels / scale).round();
    universe_preview::Place {
        left: dip(screen.left + pixels(area.min.x)) as i32,
        top: dip(screen.top + pixels(area.min.y)) as i32,
        width: dip(pixels(area.width())).max(1.0) as u32,
        height: dip(pixels(area.height())).max(1.0) as u32,
    }
}

/// The App view is not shown: Chrome's window hides with it.
pub(crate) fn hidden(app: &mut App, effects: &mut Vec<Effect>) {
    if let Some(effect) = app.app_place(None) {
        effects.push(effect);
    }
}

/// The wait while a page is built and nothing shows yet: stars that come
/// near, the calls the back end answers flying out among them; flat when
/// the window draws nothing in depth (`/bullets plain`).
fn waiting(ui: &Ui, painter: &egui::Painter, area: Rect, app: &App, deep: bool) {
    let time = ui.input(|i| i.time);
    let centre = area.center();
    let preview = app.app_preview();
    // A number from a place, the same each time.
    let noise = |i: u32, salt: u32| {
        let mut h = i.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        (h % 10_000) as f32 / 10_000.0
    };
    if deep {
        let half = area.size() * 0.5;
        for i in 0..220u32 {
            let (x, y) = (noise(i, 1) * 2.0 - 1.0, noise(i, 2) * 2.0 - 1.0);
            let speed = 0.12 + noise(i, 3) * 0.1;
            let z = 1.0 - ((time as f32 * speed + noise(i, 4)) % 1.0);
            let project = |z: f32| centre + vec2(x * half.x, y * half.y) * (0.18 / (z + 0.08));
            let (near, far) = (project(z), project((z + 0.04).min(1.0)));
            if !area.contains(near) {
                continue;
            }
            let light = ((1.0 - z) * 1.4).min(1.0);
            let colour = if i % 7 == 0 { CYAN } else { TEXT };
            painter.line_segment(
                [far, near],
                Stroke::new(0.6 + light * 1.6, colour.gamma_multiply(light)),
            );
        }
    } else {
        for ring in 0..3 {
            let phase = ((time as f32 * 0.6) + ring as f32 / 3.0) % 1.0;
            painter.circle_stroke(
                centre,
                30.0 + phase * 120.0,
                Stroke::new(1.5, ACCENT.gamma_multiply(1.0 - phase)),
            );
        }
    }
    // The calls answered, the newest flying out from the middle.
    let calls: Vec<&universe_ui::app_preview::Activity> = preview
        .activity()
        .filter(|a| a.kind == universe_ui::app_preview::ActivityKind::Call)
        .collect();
    let now = preview
        .activity()
        .last()
        .map_or(std::time::Duration::ZERO, |a| a.at);
    for (n, call) in calls.iter().rev().take(10).enumerate() {
        let age = now.saturating_sub(call.at).as_secs_f32() + (time as f32 % 1.0) * 0.0;
        let angle = noise(n as u32 + call.at.as_millis() as u32, 9) * std::f32::consts::TAU;
        let out = if deep { 60.0 + n as f32 * 26.0 } else { 0.0 };
        let at = if deep {
            centre + vec2(angle.cos(), angle.sin()) * out
        } else {
            centre + vec2(0.0, 70.0 + n as f32 * 18.0)
        };
        let fade = (1.0 - n as f32 / 10.0) * if age > 30.0 { 0.4 } else { 1.0 };
        painter.text(
            at,
            Align2::CENTER_CENTER,
            &call.text,
            FontId::monospace(11.0),
            LINK.gamma_multiply(fade),
        );
    }
    let title = if calls.is_empty() {
        "Building the page"
    } else {
        "Building the page, its data simulated"
    };
    let detail = format!(
        "{} · {:.0} s · {} calls answered",
        preview.loading_page(),
        preview.loading_for().map_or(0.0, |d| d.as_secs_f32()),
        calls.len()
    );
    // On a backdrop of their own, read over what moves behind them.
    let title = painter.layout_no_wrap(title.to_owned(), FontId::proportional(19.0), TEXT);
    let detail = painter.layout_no_wrap(detail, FontId::proportional(12.5), DIM);
    let gap = 6.0;
    let block = vec2(
        title.size().x.max(detail.size().x),
        title.size().y + gap + detail.size().y,
    );
    let backdrop = Rect::from_center_size(centre, block + vec2(36.0, 22.0));
    painter.rect_filled(
        backdrop,
        CornerRadius::same(10),
        Color32::from_rgba_unmultiplied(8, 9, 12, 210),
    );
    let top = backdrop.top() + 11.0;
    painter.galley(pos2(centre.x - title.size().x / 2.0, top), title, TEXT);
    painter.galley(
        pos2(
            centre.x - detail.size().x / 2.0,
            top + block.y - detail.size().y,
        ),
        detail,
        DIM,
    );
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
    running(ui, view, map, app, effects);
    banner(ui, app, effects);
    // The log keeps the bottom; the rest scrolls above it.
    let room = (ui.available_height() - LOG_HEIGHT - 10.0).max(80.0);
    ScrollArea::vertical()
        .id_salt("pilot")
        .max_height(room)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if app.app_preview().is_running() {
                address(ui, app, effects);
                states(ui, app, effects);
            }
            workflows(ui, view, app, effects);
            screens(ui, view, app, effects);
            if app.app_preview().is_running() {
                calls(ui, view, map, app, effects);
            }
            errors(ui, app, effects);
            how_it_runs(ui, view, app);
        });
    log(ui, app);
}

/// The log's height at the pilot's bottom.
const LOG_HEIGHT: f32 = 150.0;

/// What happens, a line each, the latest at the bottom: the front end
/// starting, pages loading and built, each call answered, what failed.
fn log(ui: &mut Ui, app: &App) {
    ui.add_space(6.0);
    ui.label(RichText::new("LOG").size(11.0).color(DIM).strong());
    egui::Frame::NONE
        .fill(Color32::from_rgb(10, 12, 16))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.set_min_height(LOG_HEIGHT - 30.0);
            ScrollArea::vertical()
                .id_salt("log")
                .max_height(LOG_HEIGHT - 30.0)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    // Its lines are copied, as text is.
                    ui.style_mut().interaction.selectable_labels = true;
                    for line in app.app_preview().activity() {
                        let colour = match line.kind {
                            universe_ui::app_preview::ActivityKind::Step => TEXT,
                            universe_ui::app_preview::ActivityKind::Call => LINK,
                            universe_ui::app_preview::ActivityKind::Done => GREEN,
                            universe_ui::app_preview::ActivityKind::Built => YELLOW,
                            universe_ui::app_preview::ActivityKind::Failed => RED,
                        };
                        // Its time apart, so that a long line wraps beside it.
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(
                                RichText::new(format!("{:>5.1}", line.at.as_secs_f32()))
                                    .monospace()
                                    .size(10.5)
                                    .color(DIM),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&line.text)
                                        .monospace()
                                        .size(10.5)
                                        .color(colour),
                                )
                                .wrap(),
                            );
                        });
                    }
                });
        });
}

/// What the page met, said at the top while it shows: the error, and the
/// way to copy it.
fn banner(ui: &mut Ui, app: &App, effects: &mut Vec<Effect>) {
    let errors: Vec<String> = app.app_preview().page_errors().cloned().collect();
    let Some(first) = errors.first() else {
        return;
    };
    ui.add_space(6.0);
    egui::Frame::NONE
        .fill(Color32::from_rgb(52, 20, 24))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.style_mut().interaction.selectable_labels = true;
            let said = if errors.len() > 1 {
                format!("The page met {} errors", errors.len())
            } else {
                "The page met an error".to_owned()
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(said).size(12.5).color(RED).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .small_button("Copy")
                        .on_hover_text("Every error of the page, to paste")
                        .clicked()
                    {
                        effects.push(Effect::Copy(errors.join("\n\n")));
                    }
                });
            });
            let line = first.lines().next().unwrap_or_default();
            ui.add(egui::Label::new(RichText::new(line).monospace().size(11.0).color(TEXT)).wrap());
        });
}

/// How the front end runs, folded at the bottom: its folder, its command,
/// the spec, and what its server printed.
fn how_it_runs(ui: &mut Ui, view: &AppView, app: &App) {
    ui.add_space(8.0);
    egui::CollapsingHeader::new(RichText::new("HOW IT RUNS").size(12.0).color(DIM).strong())
        .id_salt("how-it-runs")
        .default_open(matches!(
            app.app_preview().status(),
            Some(Status::Failed(_))
        ))
        .show(ui, |ui| {
            ui.style_mut().interaction.selectable_labels = true;
            if let Some((folder, line, spec)) = &view.how {
                ui.label(RichText::new(format!("in {folder}")).size(11.5).color(DIM));
                ui.add(
                    egui::Label::new(RichText::new(line).monospace().size(11.5).color(YELLOW))
                        .wrap(),
                );
                ui.label(RichText::new(spec).size(11.5).color(DIM));
            }
            let said: Vec<&String> = app.app_preview().said().collect();
            if !said.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Its server's last lines")
                        .size(11.0)
                        .color(DIM),
                );
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
            }
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
    view.how = config.as_ref().ok().map(|config| {
        let folder = if front.folder.is_empty() {
            "the project's root".to_owned()
        } else {
            front.folder.clone()
        };
        (
            folder,
            universe_preview::command_shown(&config.run),
            spec_said(&config.spec, &view.root),
        )
    });
    if let Err(why) = &config {
        ui.label(RichText::new(why).size(12.0).color(RED));
    }
    ui.horizontal(|ui| {
        if on {
            if ui.button(RichText::new("■ Stop").color(RED)).clicked() {
                effects.push(app.stop_app());
            }
        } else if let Ok(config) = config
            && ui
                .button(RichText::new("▶ Start").color(GREEN))
                .on_hover_text(format!(
                    "Runs its front end ({}), a back end simulated from its spec, and Chrome over this view: How it runs, at the bottom, says how",
                    view.how.as_ref().map_or("", |(_, line, _)| line.as_str())
                ))
                .clicked()
        {
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
        leftovers: view
            .base
            .as_ref()
            .map(|b| universe_preview::leftovers_file(b, root)),
        shown: true,
        parent: view.screen.and_then(|screen| screen.parent),
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
        let done = playing.shown;
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
            });
    }
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

/// The page's errors, each to copy, the same one counted.
fn errors(ui: &mut Ui, app: &App, effects: &mut Vec<Effect>) {
    let errors: Vec<&String> = app.app_preview().errors().collect();
    if errors.is_empty() {
        return;
    }
    heading(ui, &format!("ERRORS SEEN · {}", errors.len()));
    let mut counted: Vec<(&String, usize)> = Vec::new();
    for error in errors.iter().rev() {
        match counted.iter_mut().find(|(e, _)| e == error) {
            Some((_, n)) => *n += 1,
            None => counted.push((error, 1)),
        }
    }
    ui.scope(|ui| {
        ui.style_mut().interaction.selectable_labels = true;
        for (error, times) in counted.into_iter().take(20) {
            ui.horizontal(|ui| {
                if ui.small_button("Copy").clicked() {
                    effects.push(Effect::Copy(error.clone()));
                }
                if times > 1 {
                    ui.label(RichText::new(format!("×{times}")).size(11.0).color(DIM));
                }
            });
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
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_s_page_goes_over_the_area_whatever_the_scales() {
        let area = Rect::from_min_size(pos2(12.0, 92.0), vec2(1000.0, 700.0));
        let screen = Screen {
            left: 50.0,
            top: 40.0,
            scale: 1.0,
            minimized: false,
            parent: None,
        };
        assert_eq!(
            place(area, 1.0, screen),
            universe_preview::Place {
                left: 62,
                top: 132,
                width: 1000,
                height: 700
            }
        );
        // Text drawn larger: more of the screen's pixels for each point.
        let larger = place(area, 1.25, screen);
        assert_eq!((larger.left, larger.width), (65, 1250));
        // A screen of two pixels to the point, which Chrome counts as one.
        let dense = Screen {
            left: 100.0,
            top: 80.0,
            scale: 2.0,
            ..screen
        };
        assert_eq!(
            place(area, 2.0, dense),
            universe_preview::Place {
                left: 62,
                top: 132,
                width: 1000,
                height: 700
            }
        );
    }
}
