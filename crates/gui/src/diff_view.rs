//! A pull request's diff as GitHub shows it: every file in one scrolling
//! stream, grouped by area, each line numbered old and new and coloured by
//! its language, three lines around each change and the runs between
//! folded, a click away from twenty more above, below, or all of them;
//! generated files folded whole. A click on a line number picks it,
//! Shift-click the lines to it, for the next message to be about them.
//!
//! The diffs are read on a thread of their own, once per change set. A
//! code line takes the same height whether in sight or not, and is only
//! drawn in sight, so that a long pull request scrolls freely.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use bevy_egui::egui::{
    self, Color32, CornerRadius, FontId, Frame, Margin, RichText, Sense, Stroke, TextFormat, Ui,
    text::LayoutJob,
};
use universe_codemap::{Area, Review};
use universe_tools::Change;
use universe_ui::changes::ChangeSet;
use universe_ui::diff_stream::{FileDiff, RowKind};
use universe_ui::pull_review::{AreaDiff, Summaries};

use crate::theme::{ACCENT, DIM, EDGE, GREEN, LINK, RAISED, RED, SELECTED, TEXT, YELLOW, rgb};

/// How many rows a click at a fold's edge shows.
const MORE: usize = 20;

/// The size of the code.
const CODE: f32 = 12.5;

/// A fold opened: how many rows show from its top, from its bottom, or all.
#[derive(Debug, Clone, Copy, Default)]
struct Opened {
    above: usize,
    below: usize,
    all: bool,
}

/// The stream's state between frames.
#[derive(Default)]
pub(crate) struct DiffView {
    /// What the diffs were read for.
    key: Option<String>,
    reading: Option<Receiver<Vec<FileDiff>>>,
    diffs: Vec<FileDiff>,
    /// The folds opened, by file and the fold's first row.
    opened: HashMap<(String, usize), Opened>,
    /// Generated files shown whole.
    shown_whole: HashSet<String>,
    /// The file to bring to the top next frame.
    scroll_to: Option<String>,
    /// The file being read: the last whose header went past the top.
    current: Option<String>,
}

/// What a click asked of the stream.
pub(crate) enum Picked {
    /// A line of a file, by its number in the new file (the old one's for a
    /// removed line), and whether Shift was held to pick the lines to it.
    Line(String, usize, bool),
}

impl DiffView {
    /// Reads the diffs of `changes` on a thread when they differ from
    /// those read; forgets them when there are none.
    pub(crate) fn update(&mut self, changes: Option<&ChangeSet>) {
        let Some(changes) = changes else {
            *self = Self::default();
            return;
        };
        let key = format!(
            "{} {} {}",
            changes.root.display(),
            changes.base,
            changes
                .files
                .iter()
                .map(|f| format!("{}+{}-{}", f.path, f.added, f.removed))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if self.key.as_deref() != Some(key.as_str()) {
            *self = Self {
                key: Some(key),
                ..Self::default()
            };
            let (send, receive) = mpsc::channel();
            let (root, base, files) = (
                changes.root.clone(),
                changes.base.clone(),
                changes.files.clone(),
            );
            std::thread::spawn(move || {
                let _ = send.send(universe_ui::diff_stream::read(&root, &base, &files));
            });
            self.reading = Some(receive);
        }
        if let Some(reading) = &self.reading {
            match reading.try_recv() {
                Ok(diffs) => {
                    self.diffs = diffs;
                    self.reading = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.reading = None,
            }
        }
    }

    /// Whether the diffs are still being read.
    pub(crate) fn is_reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Brings `path` to the top of the stream.
    pub(crate) fn go_to(&mut self, path: &str) {
        self.scroll_to = Some(path.to_owned());
    }

    /// The file being read, for the tree to follow.
    pub(crate) fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Whether the diffs are read, for a model to be given.
    pub(crate) fn is_read(&self) -> bool {
        !self.is_reading() && !self.diffs.is_empty()
    }

    /// The diff by area, as a model is given it to summarize.
    pub(crate) fn area_diffs(&self, review: &Review) -> Vec<AreaDiff> {
        review
            .areas
            .iter()
            .map(|(area, paths)| AreaDiff {
                area: area.name().to_owned(),
                files: paths.clone(),
                generated: *area == Area::Generated,
                diff: self
                    .diffs
                    .iter()
                    .filter(|d| paths.contains(&d.path))
                    .map(FileDiff::unified)
                    .collect(),
            })
            .collect()
    }

    /// Draws the stream: `review` gives each file its area, `summaries`
    /// what a model said of the whole, each area and each file, drawn by
    /// `markdown` on a background of their own, and `picked` the lines
    /// picked, lit.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        review: Option<&Review>,
        summaries: Option<&Summaries>,
        markdown: &mut dyn FnMut(&mut Ui, &str),
        picked: Option<(&str, usize, usize)>,
    ) -> Option<Picked> {
        if self.is_reading() && self.diffs.is_empty() {
            ui.label(RichText::new("Reading the diff…").color(DIM));
            ui.ctx().request_repaint();
            return None;
        }
        // The files in their areas, in the areas' order; those the review
        // does not know yet under Other.
        let mut grouped: Vec<(Area, Vec<usize>)> = Vec::new();
        let mut placed: HashSet<usize> = HashSet::new();
        if let Some(review) = review {
            for (area, paths) in &review.areas {
                let indices: Vec<usize> = paths
                    .iter()
                    .filter_map(|p| self.diffs.iter().position(|d| &d.path == p))
                    .collect();
                placed.extend(&indices);
                if !indices.is_empty() {
                    grouped.push((*area, indices));
                }
            }
        }
        let rest: Vec<usize> = (0..self.diffs.len())
            .filter(|i| !placed.contains(i))
            .collect();
        if !rest.is_empty() {
            grouped.push((Area::Other, rest));
        }
        let mut clicked = None;
        let top = ui.clip_rect().top();
        let mut current = None;
        match summaries {
            Some(summaries) => said(ui, &summaries.overall, markdown),
            None => {
                ui.label(RichText::new("◌ a model is reading it…").small().color(DIM));
            }
        }
        for (area, indices) in &grouped {
            ui.add_space(14.0);
            ui.label(RichText::new(area.name()).size(17.0).strong().color(TEXT));
            if let Some(text) = summaries.and_then(|s| s.areas.get(area.name())) {
                said(ui, text, markdown);
            }
            for &i in indices {
                let generated = *area == Area::Generated;
                let words = summaries
                    .and_then(|s| s.files.get(&self.diffs[i].path))
                    .cloned();
                let header = self.file_header(ui, i, generated, words.as_deref());
                if header.top() <= top + 4.0 {
                    current = Some(self.diffs[i].path.clone());
                }
                if self.scroll_to.as_deref() == Some(self.diffs[i].path.as_str()) {
                    ui.scroll_to_rect(header, Some(egui::Align::TOP));
                    self.scroll_to = None;
                }
                let folded_whole = generated && !self.shown_whole.contains(&self.diffs[i].path);
                if !folded_whole && let Some(pick) = self.file_rows(ui, i, picked) {
                    clicked = Some(pick);
                }
            }
        }
        self.current = current.or_else(|| self.diffs.first().map(|d| d.path.clone()));
        clicked
    }

    /// A file's header: its path, how it changed, and for a generated one a
    /// button to show it whole. Returns where it was drawn.
    fn file_header(
        &mut self,
        ui: &mut Ui,
        i: usize,
        generated: bool,
        words: Option<&str>,
    ) -> egui::Rect {
        let diff = &self.diffs[i];
        let (letter, colour) = match &diff.change {
            Change::Added => ("A", GREEN),
            Change::Deleted => ("D", RED),
            Change::Renamed { .. } => ("R", LINK),
            Change::Modified => ("M", YELLOW),
        };
        let path = diff.path.clone();
        let shown_whole = self.shown_whole.contains(&path);
        let mut toggle = false;
        let response = Frame::new()
            .fill(RAISED)
            .stroke(Stroke::new(1.0, EDGE))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(letter).monospace().strong().color(colour));
                    let shown = match &diff.change {
                        Change::Renamed { from } => format!("{from} → {}", diff.path),
                        _ => diff.path.clone(),
                    };
                    ui.label(RichText::new(shown).monospace().color(TEXT));
                    ui.label(
                        RichText::new(format!("+{} −{}", diff.added, diff.removed))
                            .small()
                            .color(DIM),
                    );
                    if let Some(words) = words {
                        ui.label(RichText::new(words).small().italics().color(ACCENT));
                    }
                    if generated
                        && ui
                            .small_button(if shown_whole { "fold" } else { "show" })
                            .on_hover_text("Generated: folded until asked for")
                            .clicked()
                    {
                        toggle = true;
                    }
                });
            })
            .response;
        if toggle && !self.shown_whole.remove(&path) {
            self.shown_whole.insert(path);
        }
        response.rect
    }

    /// A file's rows, its folds closed but where opened.
    fn file_rows(
        &mut self,
        ui: &mut Ui,
        i: usize,
        picked: Option<(&str, usize, usize)>,
    ) -> Option<Picked> {
        let diff = &self.diffs[i];
        if diff.binary {
            ui.label(
                RichText::new("not text: nothing to show")
                    .small()
                    .color(DIM),
            );
            return None;
        }
        let row_height = ui.fonts_mut(|f| f.row_height(&FontId::monospace(CODE))) + 2.0;
        let folds = diff.folds();
        let mut clicked = None;
        let mut opened_now: Option<((String, usize), Opened)> = None;
        let mut row = 0;
        while row < diff.rows.len() {
            if let Some(fold) = folds.iter().find(|f| f.start == row) {
                let key = (diff.path.clone(), fold.start);
                let opened = self.opened.get(&key).copied().unwrap_or_default();
                let len = fold.len();
                if opened.all || opened.above + opened.below >= len {
                    // Wholly opened: its rows as any.
                } else {
                    let above = opened.above.min(len);
                    for r in row..row + above {
                        if let Some(p) = code_row(ui, diff, r, row_height, picked) {
                            clicked = Some(p);
                        }
                    }
                    let hidden = len - above - opened.below.min(len - above);
                    let mut asked = None;
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let label = RichText::new(format!("⋯ {hidden} lines unchanged"))
                            .small()
                            .color(DIM);
                        ui.label(label);
                        if ui.small_button(format!("↑ {MORE}")).clicked() {
                            asked = Some(Opened {
                                above: opened.above + MORE,
                                ..opened
                            });
                        }
                        if ui.small_button("all").clicked() {
                            asked = Some(Opened {
                                all: true,
                                ..opened
                            });
                        }
                        if ui.small_button(format!("↓ {MORE}")).clicked() {
                            asked = Some(Opened {
                                below: opened.below + MORE,
                                ..opened
                            });
                        }
                    });
                    if let Some(asked) = asked {
                        opened_now = Some((key, asked));
                    }
                    let below = opened.below.min(len - above);
                    for r in fold.end - below..fold.end {
                        if let Some(p) = code_row(ui, diff, r, row_height, picked) {
                            clicked = Some(p);
                        }
                    }
                    row = fold.end;
                    continue;
                }
            }
            if let Some(p) = code_row(ui, diff, row, row_height, picked) {
                clicked = Some(p);
            }
            row += 1;
        }
        if let Some((key, opened)) = opened_now {
            self.opened.insert(key, opened);
        }
        clicked
    }
}

/// What a model said, on a background of its own so that it is never taken
/// for code.
fn said(ui: &mut Ui, text: &str, markdown: &mut dyn FnMut(&mut Ui, &str)) {
    Frame::new()
        .fill(ACCENT.gamma_multiply(0.07))
        .stroke(Stroke::new(1.0, ACCENT.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            markdown(ui, text);
        });
    ui.add_space(6.0);
}

/// One line of code: its numbers, its sign and its text, tinted when added
/// or removed, lit when picked; drawn only when in sight.
fn code_row(
    ui: &mut Ui,
    diff: &FileDiff,
    index: usize,
    height: f32,
    picked: Option<(&str, usize, usize)>,
) -> Option<Picked> {
    let row = &diff.rows[index];
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let line = row.new.or(row.old).unwrap_or(0);
    let lit =
        picked.is_some_and(|(path, from, to)| path == diff.path && (from..=to).contains(&line));
    let (tint, sign) = match row.kind {
        RowKind::Added => (GREEN.gamma_multiply(0.14), "+"),
        RowKind::Removed => (RED.gamma_multiply(0.14), "−"),
        RowKind::Same => (Color32::TRANSPARENT, " "),
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, if lit { SELECTED } else { tint });
    let font = FontId::monospace(CODE);
    let number = |n: Option<usize>| n.map_or_else(|| "    ".to_owned(), |n| format!("{n:>4}"));
    let numbers = format!("{} {}", number(row.old), number(row.new));
    let numbers_width = ui.fonts_mut(|f| {
        f.layout_no_wrap(numbers.clone(), font.clone(), DIM)
            .size()
            .x
    });
    painter.text(
        rect.left_center() + egui::vec2(4.0, 0.0),
        egui::Align2::LEFT_CENTER,
        &numbers,
        font.clone(),
        if lit { ACCENT } else { DIM },
    );
    let mut job = LayoutJob::default();
    job.append(
        sign,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color: match row.kind {
                RowKind::Added => GREEN,
                RowKind::Removed => RED,
                RowKind::Same => DIM,
            },
            ..TextFormat::default()
        },
    );
    job.append(" ", 0.0, TextFormat::simple(font.clone(), DIM));
    for (colour, text) in &row.runs {
        job.append(text, 0.0, TextFormat::simple(font.clone(), rgb(*colour)));
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    painter.galley(
        rect.left_center() + egui::vec2(numbers_width + 14.0, -galley.size().y / 2.0),
        galley,
        TEXT,
    );
    // The numbers pick the line, as on GitHub.
    let numbers_rect = egui::Rect::from_min_size(rect.min, egui::vec2(numbers_width + 8.0, height));
    let response = ui
        .interact(
            numbers_rect,
            ui.id().with(("pick", &diff.path, index)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        let shift = ui.input(|i| i.modifiers.shift);
        return Some(Picked::Line(diff.path.clone(), line, shift));
    }
    None
}
