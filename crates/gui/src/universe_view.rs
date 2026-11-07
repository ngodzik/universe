//! The universe drawn over its stars: the names of what is looked at, the
//! galaxies to enter, what it holds and how it is grouped, its legend, and
//! the way to the deployments. The stars themselves are Bevy's, in
//! [`universe`](crate::universe).

use bevy::math::Vec2;
use bevy_egui::egui::{self, Align, Color32, Id, Layout, RichText, Stroke, Ui, UiBuilder};
use universe_codemap::NodeKind;
use universe_ui::App;

use crate::plan;
use crate::theme::{self, DIM, EDGE, RAISED, TEXT};
use crate::universe::Universe;

/// The most files a narrowed universe names all of.
const FEW_NAMED: usize = 40;

/// The universe's own controls: dragging turns it, the wheel comes closer,
/// a click centres a star, a double click opens its file, a click on a
/// galaxy's name enters it. Names show for each galaxy, for the star under
/// the pointer and the one chosen, and for every file once few are looked
/// at. Returns whether the deployments were asked for.
pub(crate) fn show(ui: &mut Ui, universe: &mut Universe, app: &mut App) -> bool {
    universe.search(app.map_search());
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    if response.dragged() {
        let delta = response.drag_delta();
        universe.drag += Vec2::new(delta.x, delta.y);
    }
    if response.hovered() {
        universe.zoom += ui.input(|i| i.smooth_scroll_delta.y);
    }
    let pointer = response.hover_pos();
    // Only the stars of the galaxy entered can be picked: the others are
    // background.
    universe.hovered = pointer.and_then(|at| {
        universe
            .on_screen
            .iter()
            .enumerate()
            .filter(|(node, _)| universe.lit(*node))
            .filter_map(|(node, p)| p.map(|p| (node, egui::pos2(p.x, p.y).distance(at))))
            .filter(|(_, d)| *d < 18.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(node, _)| node)
    });
    if response.double_clicked()
        && let Some(node) = universe.hovered
        && let Some(map) = universe.map()
        && matches!(map.nodes[node].kind, NodeKind::File { .. })
    {
        let path = universe.root().join(&map.nodes[node].path);
        app.open_path(path);
        app.show_map(None);
    } else if response.clicked() {
        // A click frames again: the star chosen, or the whole.
        universe.chosen = universe.hovered;
        universe.zoomed = false;
    }

    let painter = ui.painter_at(rect);
    let Some(map) = universe.map() else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Mapping the codebase…",
            egui::FontId::proportional(18.0),
            DIM,
        );
        return false;
    };
    // The names worth reading: under the pointer, or chosen; and every
    // file's, faint, once few enough are looked at to read them all and no
    // galaxy names them.
    let lit_files = (0..map.nodes.len())
        .filter(|n| universe.lit(*n))
        .filter(|n| matches!(map.nodes[*n].kind, NodeKind::File { .. }))
        .count();
    let few =
        (universe.grouping().groups.is_empty() || universe.searching()) && lit_files <= FEW_NAMED;
    for (node, at) in universe.on_screen.iter().enumerate() {
        let Some(at) = at else {
            continue;
        };
        let picked = universe.hovered == Some(node) || universe.chosen == Some(node);
        let named =
            few && universe.lit(node) && matches!(map.nodes[node].kind, NodeKind::File { .. });
        if !picked && !named {
            continue;
        }
        let entry = &map.nodes[node];
        let [r, g, b] = universe.tint(node);
        let fade = if picked { 1.0 } else { 0.25 };
        let colour = Color32::from_rgb(r, g, b).gamma_multiply(0.4 + 0.6 * fade);
        let text = if picked {
            match entry.kind {
                NodeKind::File { lines, .. } => format!("{}  ·  {lines} lines", entry.path),
                NodeKind::Folder => format!(
                    "{}/",
                    if entry.path.is_empty() {
                        "."
                    } else {
                        &entry.path
                    }
                ),
            }
        } else {
            entry.name().to_owned()
        };
        let at = egui::pos2(at.x + 12.0, at.y - 10.0);
        let font = egui::FontId::proportional(if picked { 15.0 } else { 13.0 });
        shadowed(&painter, at, egui::Align2::LEFT_BOTTOM, &text, font, colour);
    }

    // Each galaxy's name above it, to click to enter it.
    let mut enter = None;
    let groups = universe.grouping().groups.clone();
    for (galaxy, group) in groups.iter().enumerate() {
        let Some(at) = universe.galaxies_on_screen.get(galaxy).copied().flatten() else {
            continue;
        };
        let at = egui::pos2(at.x, at.y);
        if !rect.contains(at) {
            continue;
        }
        // Searched, how many of its files are found: a galaxy with none
        // fades, its name too.
        let found = universe.searching().then(|| {
            (0..map.nodes.len())
                .filter(|&n| {
                    universe.lit(n) && universe.grouping().of(n).map(|g| g.0) == Some(galaxy)
                })
                .filter(|&n| matches!(map.nodes[n].kind, NodeKind::File { .. }))
                .count()
        });
        // A galaxy with nothing found is background: unnamed.
        if found == Some(0) {
            continue;
        }
        let [r, g, b] = universe.galaxy_colour(galaxy);
        let colour = Color32::from_rgb(r, g, b);
        let name = egui::FontId::proportional(16.0);
        let galley = painter.layout_no_wrap(group.label.clone(), name.clone(), colour);
        let area = egui::Rect::from_center_size(
            at + egui::vec2(0.0, -galley.size().y / 2.0),
            galley.size() + egui::vec2(12.0, 6.0),
        );
        let openable = group.key.is_some();
        let label = ui.interact(
            area,
            Id::new(("galaxy", galaxy)),
            if openable {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );
        let lit = openable && label.hovered();
        if lit {
            painter.rect_filled(area, 6.0, Color32::from_black_alpha(160));
            painter.rect_stroke(
                area,
                6.0,
                Stroke::new(1.0, colour.gamma_multiply(0.7)),
                egui::StrokeKind::Inside,
            );
        }
        shadowed(
            &painter,
            at,
            egui::Align2::CENTER_BOTTOM,
            &group.label,
            name,
            if lit {
                colour
            } else {
                colour.gamma_multiply(0.85)
            },
        );
        let files = if let Some(found) = found {
            format!("{found} of {} found", group.files)
        } else if group.files == 1 {
            "1 file".to_owned()
        } else {
            format!("{} files", group.files)
        };
        shadowed(
            &painter,
            at + egui::vec2(0.0, 2.0),
            egui::Align2::CENTER_TOP,
            &files,
            egui::FontId::proportional(11.0),
            DIM,
        );
        let label = label.on_hover_cursor(egui::CursorIcon::PointingHand);
        if label.clicked() {
            enter = Some(galaxy);
        }
    }

    // What the universe is, in the corner: the way in, what it holds, what
    // it is grouped by, and what the colours mean.
    let inside = |node: &usize| universe.grouping().inside(*node);
    let files = (0..map.nodes.len())
        .filter(inside)
        .filter(|n| matches!(map.nodes[*n].kind, NodeKind::File { .. }))
        .count();
    let imports = map
        .edges
        .iter()
        .filter(|e| e.kind == universe_codemap::EdgeKind::Imports)
        .filter(|e| inside(&e.from) && inside(&e.to))
        .count();
    let left_out = map.left_out;
    let corner = rect.left_top() + egui::vec2(24.0, 22.0);
    let path: Vec<String> = universe.path().map(str::to_owned).collect();
    let mut leave = None;
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(corner, egui::vec2(560.0, 34.0)))
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // Inside a galaxy, the way back out is a button, not only the
            // path: one level up.
            if !path.is_empty() {
                let back = egui::Button::new(RichText::new("‹ Back").size(14.0).color(TEXT))
                    .fill(theme::see(RAISED, 0.9))
                    .stroke(Stroke::new(1.0, EDGE))
                    .corner_radius(6.0);
                if ui
                    .add(back)
                    .on_hover_text("Back out one level")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    leave = Some(path.len() - 1);
                }
                ui.add_space(6.0);
            }
            let crumb = |ui: &mut Ui, name: &str, size: f32, last: bool| {
                if last {
                    ui.label(RichText::new(name).size(size).color(TEXT));
                    return false;
                }
                // Lit and underlined under the pointer, to read as a link.
                let id = ui.next_auto_id();
                let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
                let mut text =
                    RichText::new(name)
                        .size(size)
                        .color(if hovered { TEXT } else { DIM });
                if hovered {
                    text = text.underline();
                }
                ui.add(egui::Label::new(text).sense(egui::Sense::click()))
                    .on_hover_text(format!("Back to {name}"))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
            };
            if crumb(ui, "Universe", 26.0, path.is_empty()) {
                leave = Some(0);
            }
            for (depth, name) in path.iter().enumerate() {
                ui.label(RichText::new("›").size(20.0).color(DIM));
                if crumb(ui, name, 20.0, depth + 1 == path.len()) {
                    leave = Some(depth + 1);
                }
            }
        },
    );
    let mut summary = format!("{} · {files} files · {imports} imports", app.project());
    if universe.searching() {
        summary.push_str(&format!(" · {lit_files} found"));
    }
    if left_out > 0 && path.is_empty() {
        summary.push_str(&format!(" · {left_out} more not shown"));
    }
    painter.text(
        corner + egui::vec2(0.0, 38.0),
        egui::Align2::LEFT_TOP,
        summary,
        egui::FontId::proportional(13.0),
        DIM,
    );

    // The criteria, to pick in order: the first makes the galaxies, the
    // second the clusters within them.
    let mut toggle = None;
    let mut deployments = false;
    let chosen = universe.criteria().to_vec();
    let applied = universe.grouping().criteria.clone();
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                corner + egui::vec2(0.0, 60.0),
                egui::vec2(560.0, 26.0),
            ))
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new("Group by").size(12.0).color(DIM));
            for criterion in universe_codemap::Criterion::ALL {
                let place = chosen.iter().position(|c| *c == criterion);
                let text = match place {
                    Some(at) => format!("{} {}", at + 1, criterion.name()),
                    None => criterion.name().to_owned(),
                };
                // Chosen but saying nothing here (one language inside a
                // language): shown, but dim.
                let colour = match place {
                    Some(_) if applied.contains(&criterion) => TEXT,
                    Some(_) => DIM,
                    None => DIM,
                };
                let button = egui::Button::selectable(
                    place.is_some(),
                    RichText::new(text).size(13.0).color(colour),
                );
                let hint = match place {
                    Some(0) => "Makes the galaxies: click to stop grouping by it",
                    Some(_) => "Makes the clusters within each galaxy, and the colours: click to stop grouping by it",
                    None if chosen.is_empty() => "Group the stars into galaxies by it",
                    None => "Group each galaxy's stars into clusters by it",
                };
                if ui.add(button).on_hover_text(hint).clicked() {
                    toggle = Some(criterion);
                }
            }
            ui.add_space(10.0);
            let button = egui::Button::new(RichText::new("Deployments").size(13.0).color(TEXT))
                .fill(theme::see(RAISED, 0.9))
                .stroke(Stroke::new(1.0, EDGE));
            if ui
                .add(button)
                .on_hover_text("Every environment as a galaxy, each service a star")
                .clicked()
            {
                deployments = true;
            }
        },
    );

    // The legend: what the colours mean, each a click away from narrowing
    // to it, unlike the galaxies' names, which turn with the sky.
    let mut at = corner + egui::vec2(0.0, 96.0);
    let legend: Vec<(String, [u8; 3], usize, bool)> = universe
        .legend()
        .map(|(name, colour, files, openable)| (name.to_owned(), colour, files, openable))
        .collect();
    const SHOWN: usize = 12;
    let mut narrow = None;
    for (place, (name, [r, g, b], files, openable)) in legend.iter().enumerate().take(SHOWN) {
        let colour = Color32::from_rgb(*r, *g, *b);
        let galley = painter.layout_no_wrap(
            format!("{name}  {files}"),
            egui::FontId::proportional(12.0),
            DIM,
        );
        let area = egui::Rect::from_min_size(at, galley.size() + egui::vec2(16.0, 2.0));
        let lit = *openable
            && ui
                .interact(area, Id::new(("legend", place)), egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!("Only {name}"))
                .clicked();
        if lit {
            narrow = Some(place);
        }
        let hovered = *openable && ui.rect_contains_pointer(area);
        painter.circle_filled(at + egui::vec2(5.0, 7.0), 4.0, colour);
        painter.text(
            at + egui::vec2(16.0, 0.0),
            egui::Align2::LEFT_TOP,
            format!("{name}  {files}"),
            egui::FontId::proportional(12.0),
            if hovered { TEXT } else { DIM },
        );
        at.y += 18.0;
    }
    if legend.len() > SHOWN {
        painter.text(
            at + egui::vec2(16.0, 0.0),
            egui::Align2::LEFT_TOP,
            format!("and {} more", legend.len() - SHOWN),
            egui::FontId::proportional(12.0),
            DIM,
        );
        at.y += 18.0;
    }
    painter.circle_filled(
        at + egui::vec2(5.0, 7.0),
        4.0,
        Color32::from_rgb(140, 90, 255),
    );
    painter.text(
        at + egui::vec2(16.0, 0.0),
        egui::Align2::LEFT_TOP,
        "folders",
        egui::FontId::proportional(12.0),
        DIM,
    );
    let mut hint = vec!["drag to turn", "scroll to come closer"];
    if !groups.is_empty() {
        hint.push("click a galaxy's name to enter it");
    }
    if !path.is_empty() {
        hint.push("‹ Back to come back out");
    }
    hint.extend([
        "click a star to centre it",
        "double-click to open it",
        "Ctrl-N: the plan",
    ]);
    let hint = hint.join(" · ");
    painter.text(
        rect.left_bottom() + egui::vec2(24.0, -18.0),
        egui::Align2::LEFT_BOTTOM,
        hint,
        egui::FontId::proportional(12.0),
        DIM,
    );
    plan::switch(ui, rect, app);

    if let Some(criterion) = toggle {
        universe.toggle(criterion);
    } else if let Some(depth) = leave {
        universe.leave(depth);
    } else if let Some(galaxy) = enter {
        universe.enter(galaxy);
    } else if let Some(place) = narrow {
        universe.enter_legend(place);
    }
    deployments
}

/// `text` with a dark shadow under it, to read over the stars.
pub(crate) fn shadowed(
    painter: &egui::Painter,
    at: egui::Pos2,
    anchor: egui::Align2,
    text: &str,
    font: egui::FontId,
    colour: Color32,
) {
    painter.text(
        at + egui::vec2(1.0, 1.0),
        anchor,
        text,
        font.clone(),
        Color32::from_black_alpha(200),
    );
    painter.text(at, anchor, text, font, colour);
}
