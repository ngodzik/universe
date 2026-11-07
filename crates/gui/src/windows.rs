//! What opens over everything else: the places of a name to choose from,
//! and the shortcuts. Each button types the key the window says, so that
//! the state decides as it does for the keyboard.

use bevy_egui::egui::{
    self, Align2, Color32, Context, Frame, Margin, RichText, ScrollArea, Stroke, Ui,
};
use universe_ui::input::{KeyCode, KeyEvent, KeyModifiers};
use universe_ui::keymap::SHORTCUTS;
use universe_ui::{App, HELP};

use crate::theme::{ACCENT, DIM, SELECTED, TEXT};

/// Draws whatever window the state has open. Keys its buttons type are
/// added to `keys`.
pub(crate) fn show(ctx: &Context, app: &App, keys: &mut Vec<KeyEvent>) {
    if app.keys_open().is_some() {
        shortcuts(ctx);
    }
    if app.definition_choice().is_some() {
        definitions(ctx, app, keys);
    }
}

/// A window centred a third of the way down, its edge in `colour`.
fn window(ctx: &Context, title: &str, colour: Color32, add: impl FnOnce(&mut Ui)) {
    sized_window(ctx, title, colour, 520.0..=760.0, add);
}

/// A window as wide as `widths` allows.
fn sized_window(
    ctx: &Context,
    title: &str,
    colour: Color32,
    widths: std::ops::RangeInclusive<f32>,
    add: impl FnOnce(&mut Ui),
) {
    egui::Window::new(RichText::new(title).color(colour).strong())
        .id(egui::Id::new(("window", title)))
        .anchor(
            Align2::CENTER_TOP,
            egui::vec2(0.0, ctx.content_rect().height() / 6.0),
        )
        .collapsible(false)
        .resizable(false)
        .min_width(*widths.start())
        .max_width(*widths.end())
        .frame(
            Frame::window(&ctx.global_style())
                // As see-through as the window: its menus stay opaque.
                .fill(ctx.global_style().visuals.faint_bg_color)
                .stroke(Stroke::new(1.5, colour))
                .inner_margin(Margin::same(18)),
        )
        .show(ctx, add);
}

/// The keys a window takes, under it.
fn hint(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).small().color(DIM));
}

/// The places a name is defined (gd, Ctrl-click), to choose one: a click
/// goes there, as the arrows then Enter do.
fn definitions(ctx: &Context, app: &App, keys: &mut Vec<KeyEvent>) {
    let Some(choice) = app.definition_choice() else {
        return;
    };
    let title = format!(
        "Where {} is {} · {} {}",
        choice.name,
        if choice.uses { "used" } else { "defined" },
        choice.items.len(),
        if choice.uses { "uses" } else { "places" }
    );
    window(ctx, &title, ACCENT, |ui| {
        ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for (i, item) in choice.items.iter().enumerate() {
                let shown = row(ui, i == choice.selected, |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("{}:{}", item.path, item.line))
                                .small()
                                .color(DIM),
                        );
                        ui.label(RichText::new(&item.text).monospace().color(TEXT));
                    });
                });
                if i == choice.selected {
                    shown.scroll_to_me(None);
                }
                if shown.interact(egui::Sense::click()).clicked() {
                    // The state moves by keys: down or up to it, then Enter.
                    let (code, steps) = if i >= choice.selected {
                        (KeyCode::Down, i - choice.selected)
                    } else {
                        (KeyCode::Up, choice.selected - i)
                    };
                    for _ in 0..steps {
                        keys.push(KeyEvent::new(code, KeyModifiers::NONE));
                    }
                    keys.push(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
            }
        });
        hint(
            ui,
            &format!(
                "↑ ↓ to move · Enter or a click to go · Esc to close · found by {}",
                choice.by
            ),
        );
    });
}

/// A row of a list, highlighted when selected.
fn row(ui: &mut Ui, selected: bool, add: impl FnOnce(&mut Ui)) -> egui::Response {
    Frame::new()
        .fill(if selected {
            SELECTED
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(6)
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(add);
        })
        .response
}

/// What `:help` says, then every shortcut (Ctrl-S), grouped by where it
/// works.
fn shortcuts(ctx: &Context) {
    window(ctx, "Shortcuts", ACCENT, |ui| {
        ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
            ui.label(RichText::new(HELP).monospace().small().color(TEXT));
            for (group, keys) in SHORTCUTS {
                ui.add_space(6.0);
                ui.label(RichText::new(*group).color(ACCENT).strong());
                egui::Grid::new(group).num_columns(2).show(ui, |ui| {
                    for (keys, what) in *keys {
                        ui.label(RichText::new(*keys).monospace().color(DIM));
                        ui.label(*what);
                        ui.end_row();
                    }
                });
            }
        });
        hint(ui, "Esc to close");
    });
}
