//! An image over the whole window, a diagram or a picture a reply showed:
//! zoomed by a pinch or the platform's zoom modifier with the wheel, toward
//! the pointer; moved by dragging or scrolling; fitted again with F; closed
//! with Esc or a click beside it. It sits on a dark card, so that a diagram
//! drawn on a transparent background reads at any window opacity.

use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Order, Rect, Sense, TextureHandle,
    Vec2, pos2,
};

use crate::theme::{DIM, EDGE, RAISED};

/// Where a click on an image leaves it, in egui's memory, for the window
/// to open the viewer once the drawing is done: the transcript only reads
/// the state.
pub(crate) const OPEN: &str = "open-image";

/// The most an image is enlarged, in screen pixels for one of its own.
const LARGEST: f32 = 16.0;

/// The least, as a share of the size that fits.
const SMALLEST: f32 = 0.2;

/// How much a key zooms.
pub(crate) const STEP: f32 = 1.25;

/// An image shown whole.
pub(crate) struct Viewer {
    texture: TextureHandle,
    /// Points for one of its pixels; unset, it fits the window.
    scale: Option<f32>,
    /// Where its centre is from the window's.
    offset: Vec2,
    /// A key's zoom asked for before it was first drawn.
    pending_zoom: f32,
}

impl Viewer {
    /// `texture`, fitted to the window.
    pub(crate) fn new(texture: TextureHandle) -> Self {
        Self {
            texture,
            scale: None,
            offset: Vec2::ZERO,
            pending_zoom: 1.0,
        }
    }

    /// Back to fitting the window.
    pub(crate) fn fit(&mut self) {
        self.scale = None;
        self.offset = Vec2::ZERO;
    }

    /// Zooms by `factor` about the window's centre, as a key does.
    pub(crate) fn zoom(&mut self, factor: f32) {
        if let Some(scale) = &mut self.scale {
            *scale *= factor;
            self.offset *= factor;
        } else {
            // Unset, it is fitted: the next frame zooms from there.
            self.pending_zoom *= factor;
        }
    }

    /// Draws it over the window; `false` once the person closed it.
    pub(crate) fn show(&mut self, ctx: &egui::Context) -> bool {
        let area = ctx.content_rect();
        let mut open = true;
        egui::Area::new(Id::new("image-viewer"))
            .order(Order::Foreground)
            .fixed_pos(area.min)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(area.size(), Sense::click_and_drag());
                let painter = ui.painter();
                // Opaque: blended in linear light, even a nearly opaque black lets a
                // fifth of what is behind show.
                painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(6, 7, 10));
                let size = self.texture.size_vec2();
                let fit = fitted(size, rect.size(), ui.pixels_per_point());
                let mut scale = self.scale.unwrap_or(fit) * std::mem::replace(&mut self.pending_zoom, 1.0);
                let (zoom, scroll, pointer) = ui.input(|i| {
                    (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos())
                });
                let centre = rect.center();
                if zoom != 1.0 {
                    let next = (scale * zoom).clamp(fit * SMALLEST, LARGEST / ui.pixels_per_point());
                    // The point under the pointer stays under it.
                    let at = pointer.unwrap_or(centre) - centre;
                    self.offset = at - (at - self.offset) * (next / scale);
                    scale = next;
                } else {
                    self.offset += scroll;
                }
                if response.dragged() {
                    self.offset += response.drag_delta();
                }
                if response.hovered() {
                    ui.ctx().set_cursor_icon(if response.dragged() {
                        CursorIcon::Grabbing
                    } else {
                        CursorIcon::Grab
                    });
                }
                self.scale = Some(scale);
                let shown = Rect::from_center_size(centre + self.offset, size * scale);
                painter.rect_filled(shown.expand(10.0), CornerRadius::same(8), RAISED);
                painter.image(
                    self.texture.id(),
                    shown,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                if response.clicked() && !pointer.is_some_and(|p| shown.expand(10.0).contains(p)) {
                    open = false;
                }
                let percent = (scale * ui.pixels_per_point() * 100.0).round();
                let hint = painter.layout_no_wrap(
                    format!(
                        "{percent}%  ·  pinch or {} + wheel: zoom  ·  drag: move  ·  F: fit  ·  Esc: close",
                        zoom_modifier()
                    ),
                    FontId::proportional(13.0),
                    DIM,
                );
                let pill = Rect::from_center_size(
                    rect.center_top() + Vec2::new(0.0, 26.0),
                    hint.size() + Vec2::new(24.0, 10.0),
                );
                painter.rect_filled(pill, CornerRadius::same(12), RAISED);
                painter.galley(pill.center() - hint.size() / 2.0, hint, DIM);
                let close = Rect::from_center_size(rect.right_top() + Vec2::new(-28.0, 26.0), Vec2::splat(28.0));
                let button = ui.interact(close, Id::new("image-viewer-close"), Sense::click());
                painter.circle_filled(
                    close.center(),
                    13.0,
                    if button.hovered() { EDGE } else { RAISED },
                );
                painter.text(
                    close.center(),
                    Align2::CENTER_CENTER,
                    "✕",
                    FontId::proportional(14.0),
                    Color32::WHITE,
                );
                if button.clicked() {
                    open = false;
                }
            });
        open
    }
}

/// The modifier that zooms with the wheel, as the hint names it: egui's,
/// Cmd on macOS and Ctrl elsewhere.
pub(crate) fn zoom_modifier() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl"
    }
}

/// The scale, in points for one of its pixels, at which an image `size`
/// pixels fits `room` points with a margin; a small one is enlarged at
/// most twice, pixel for pixel, `ppp` screen pixels to a point.
fn fitted(size: Vec2, room: Vec2, ppp: f32) -> f32 {
    let room = room - Vec2::new(80.0, 120.0);
    (room.x / size.x.max(1.0))
        .min(room.y / size.y.max(1.0))
        .min(2.0 / ppp)
        .max(0.01)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_fits_the_window_and_a_small_one_is_enlarged_at_most_twice() {
        let room = Vec2::new(1080.0, 820.0);
        // Wide: its width decides.
        let wide = fitted(Vec2::new(4000.0, 1000.0), room, 1.0);
        assert!((wide - 0.25).abs() < 1e-6);
        // Tall: its height.
        let tall = fitted(Vec2::new(500.0, 3500.0), room, 1.0);
        assert!((tall - 0.2).abs() < 1e-6);
        // Small: twice, pixel for pixel, on a screen of two pixels a point too.
        assert!((fitted(Vec2::new(100.0, 50.0), room, 1.0) - 2.0).abs() < 1e-6);
        assert!((fitted(Vec2::new(100.0, 50.0), room, 2.0) - 1.0).abs() < 1e-6);
    }
}
