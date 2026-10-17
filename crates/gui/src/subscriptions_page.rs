//! The usage pane's second page (Ctrl-O twice): the subscriptions of Claude
//! Code and Codex, each window over its current period and when it starts
//! again, then what each request took and the context each session's calls
//! read. The charts take no drag or scroll, so that the pane scrolls.

use bevy_egui::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Ui};
use egui_plot::{Line, Plot, PlotPoints, Points};
use universe_core::TokenCount;
use universe_ui::App;
use universe_ui::sessions;
use universe_ui::subscriptions::{
    RequestView, SessionView, WindowView, back_at, local_time, percent, short, span,
};

use crate::theme::{ACCENT, CYAN, DIM, EDGE, RAISED, RED, TEXT, YELLOW};

/// How many requests are listed.
const REQUESTS: usize = 24;

/// How many sessions' contexts are drawn.
const SESSIONS: usize = 6;

/// Draws the page.
pub(crate) fn show(ui: &mut Ui, app: &App) {
    let now = sessions::now();
    let subscriptions = app.subscriptions();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Subscriptions")
                .size(20.0)
                .strong()
                .color(TEXT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("Ctrl-O closes").small().color(DIM));
        });
    });
    ui.label(
        RichText::new(
            "Shares are whole percents as the agents report them; a finer one, after ~, is \
             estimated from each call's tokens.",
        )
        .small()
        .color(DIM),
    );
    let windows = subscriptions.windows(now);
    if windows.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(
                    "Nothing heard yet: a subscription's windows show once Claude Code or Codex \
                     answers under it.",
                )
                .color(DIM),
            );
        });
        return;
    }
    ui.add_space(8.0);
    for window in &windows {
        window_card(ui, window, now);
        ui.add_space(6.0);
    }
    ui.add_space(10.0);
    heading(ui, "Requests", "the latest first, and what each took");
    let requests = subscriptions.requests(REQUESTS);
    if requests.is_empty() {
        ui.label(
            RichText::new("Their calls are being read…")
                .small()
                .color(DIM),
        );
    }
    for request in &requests {
        request_row(ui, request);
    }
    ui.add_space(10.0);
    heading(ui, "Sessions", "the context each call read");
    for session in subscriptions.sessions(SESSIONS) {
        session_plot(ui, &session);
    }
}

fn heading(ui: &mut Ui, title: &str, what: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().color(TEXT));
        ui.label(RichText::new(what).small().color(DIM));
    });
    ui.add_space(2.0);
}

/// How full a share is, as a colour.
fn fill(share: f64) -> Color32 {
    if share >= 90.0 {
        RED
    } else if share >= 70.0 {
        YELLOW
    } else {
        CYAN
    }
}

/// One window: its share in large, a bar, when it starts again, and its
/// reports over the period.
fn window_card(ui: &mut Ui, window: &WindowView, now: u64) {
    let share = window.estimate.unwrap_or(window.reported);
    Frame::new()
        .fill(RAISED)
        .stroke(egui::Stroke::new(1.0, EDGE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        short(window.agent),
                        span(window.minutes)
                    ))
                    .strong()
                    .color(TEXT),
                );
                let shown = match window.estimate {
                    Some(estimate) => format!("~{}", percent(estimate)),
                    None => percent(window.reported),
                };
                ui.label(RichText::new(shown).size(18.0).strong().color(fill(share)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let back = match window.resets_at {
                        Some(at) => format!("starts again {}", back_at(at, window.minutes)),
                        None => "ended: starts again at the next use".to_owned(),
                    };
                    ui.label(RichText::new(back).small().color(DIM));
                });
            });
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, EDGE);
            let mut used = rect;
            used.set_width(rect.width() * (share / 100.0).clamp(0.0, 1.0) as f32);
            ui.painter().rect_filled(used, 3.0, fill(share));
            if let Some(end) = window.resets_at
                && window.reports.len() > 1
            {
                period_plot(ui, window, end, now);
            }
        });
}

/// The shares reported over the period, in hours from its start, and the
/// estimate now.
fn period_plot(ui: &mut Ui, window: &WindowView, end: u64, now: u64) {
    let start = end.saturating_sub(window.minutes * 60);
    let hours = |at: u64| at.saturating_sub(start) as f64 / 3600.0;
    let reports: Vec<[f64; 2]> = window
        .reports
        .iter()
        .map(|(at, used)| [hours(*at), *used])
        .collect();
    let top = reports
        .iter()
        .map(|p| p[1])
        .fold(window.estimate.unwrap_or(0.0), f64::max)
        .max(5.0)
        * 1.2;
    Plot::new(format!(
        "subscription-{}-{}",
        short(window.agent),
        window.name
    ))
    .height(80.0)
    .show_background(false)
    .allow_drag(false)
    .allow_zoom(false)
    .allow_scroll(false)
    .allow_boxed_zoom(false)
    .allow_double_click_reset(false)
    .include_x(0.0)
    .include_x(window.minutes as f64 / 60.0)
    .include_y(0.0)
    .include_y(top)
    .y_axis_formatter(|mark, _| format!("{:.0}%", mark.value))
    .x_axis_formatter(|mark, _| format!("{:.0}h", mark.value))
    .show(ui, |plot| {
        plot.line(
            Line::new("reported", PlotPoints::from(reports.clone()))
                .color(CYAN)
                .width(1.5),
        );
        if let Some(estimate) = window.estimate {
            plot.points(
                Points::new(
                    "now, estimated",
                    PlotPoints::from(vec![[hours(now), estimate]]),
                )
                .color(ACCENT)
                .radius(4.0),
            );
        }
    });
}

/// One request: when, by whom and where, its tokens, its share of each
/// window, and whether it found the cache gone.
fn request_row(ui: &mut Ui, request: &RequestView) {
    let folder = request.folder.rsplit('/').next().unwrap_or(&request.folder);
    let who = if request.by_universe {
        "universe"
    } else {
        "by you"
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(local_time(request.at))
                .small()
                .monospace()
                .color(DIM),
        );
        ui.label(
            RichText::new(short(request.agent))
                .small()
                .strong()
                .color(TEXT),
        );
        ui.label(RichText::new(request.models.join(", ")).small().color(TEXT));
        ui.label(
            RichText::new(format!("{folder} · {who}"))
                .small()
                .color(DIM),
        );
        let shares: Vec<String> = request
            .shares
            .iter()
            .map(|(_, minutes, share)| {
                let share = share.map_or_else(|| "?".to_owned(), |s| format!("{s:.2}%"));
                format!("{} {share}", span(*minutes))
            })
            .collect();
        if !shares.is_empty() {
            ui.label(RichText::new(shares.join(" · ")).small().color(CYAN))
                .on_hover_text(
                    "Its share of each window, from the rates learnt; ? until its plan has \
                     added a whole percent",
                );
        }
        ui.label(
            RichText::new(format!(
                "{} call{} · {} in · {} cached · {} out",
                request.calls,
                if request.calls == 1 { "" } else { "s" },
                TokenCount(request.input),
                TokenCount(request.cache_read),
                TokenCount(request.output)
            ))
            .small()
            .color(DIM),
        );
        if let Some(tokens) = request.cache_gone {
            ui.label(
                RichText::new(format!("cache gone: {} read again", TokenCount(tokens)))
                    .small()
                    .color(YELLOW),
            )
            .on_hover_text(
                "Its session had been idle longer than the cache lives: about twenty minutes for \
                 Codex, an hour for Claude Code",
            );
        }
    });
}

/// One session's calls: the context each read, over time.
fn session_plot(ui: &mut Ui, session: &SessionView) {
    let folder = session.folder.rsplit('/').next().unwrap_or(&session.folder);
    ui.label(
        RichText::new(format!(
            "{} · {} · {folder} · {} calls",
            short(session.agent),
            session.model,
            session.context.len()
        ))
        .small()
        .color(TEXT),
    );
    let Some(first) = session.context.first().map(|c| c.0) else {
        return;
    };
    let points: Vec<[f64; 2]> = session
        .context
        .iter()
        .map(|(at, context)| [at.saturating_sub(first) as f64 / 60.0, *context as f64])
        .collect();
    Plot::new(format!("session-{}-{first}", short(session.agent)))
        .height(50.0)
        .show_background(false)
        .show_axes([false, true])
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .include_y(0.0)
        .y_axis_formatter(|mark, _| TokenCount(mark.value.max(0.0) as u64).to_string())
        .show(ui, |plot| {
            plot.line(
                Line::new("context", PlotPoints::from(points))
                    .color(ACCENT)
                    .width(1.2),
            );
        });
}
