//! Mermaid diagrams in the window: each drawn once on a thread, by the
//! interface's renderer, into its cache; the PNG decoded and shown as a
//! texture. Its code shows while it draws, and stays if it fails. A
//! picture a reply cited is read the same way.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy_egui::egui::{self, ColorImage, TextureHandle, TextureOptions, Ui};
use universe_ui::diagrams::Renderer;

/// Where a diagram is.
enum State {
    Drawing,
    Drawn(TextureHandle),
    Failed(String),
}

/// A diagram drawn on a thread: its key, and its pixels or why it failed.
type Drawn = (u64, Result<ColorImage, String>);

/// What an image is, for what is said while it is made or when it fails.
#[derive(Clone, Copy)]
enum Kind {
    Diagram,
    Picture,
}

impl Kind {
    fn waiting(self) -> &'static str {
        match self {
            Self::Diagram => "◌ drawing the diagram…",
            Self::Picture => "◌ reading the picture…",
        }
    }

    fn failed(self) -> &'static str {
        match self {
            Self::Diagram => "The diagram could not be drawn",
            Self::Picture => "The picture could not be read",
        }
    }
}

/// The diagrams and pictures of the conversation, shared with the threads
/// that make them.
#[derive(Clone)]
pub(crate) struct Diagrams {
    renderer: Option<Renderer>,
    cache: Option<PathBuf>,
    states: Arc<Mutex<HashMap<u64, State>>>,
    /// Images drawn, waiting to become textures on the main thread.
    drawn: Arc<Mutex<Vec<Drawn>>>,
}

impl Diagrams {
    /// Diagrams drawn by the renderer `configured` in the settings, else the
    /// first installed.
    pub(crate) fn new(configured: Option<&[String]>) -> Self {
        Self {
            renderer: Renderer::find(configured, Renderer::installed),
            cache: universe_ui::diagrams::cache(),
            states: Arc::default(),
            drawn: Arc::default(),
        }
    }

    /// Draws the diagram of `source` in `ui`, once drawn; `false` when it is
    /// not, and its code should show: being drawn, failed, or no renderer.
    pub(crate) fn show(&self, ui: &mut Ui, source: &str) -> bool {
        let (Some(renderer), Some(cache)) = (&self.renderer, &self.cache) else {
            return false;
        };
        // A model's colours, made readable on the dark theme, are what is
        // drawn and kept.
        let source = universe_ui::diagrams::readable(source);
        let key = renderer.key(&source);
        let (renderer, cache) = (renderer.clone(), cache.clone());
        self.image(ui, key, Kind::Diagram, move || {
            let done = cache.join(format!("{key:016x}.png"));
            let path = if done.is_file() {
                Ok(done)
            } else {
                renderer.draw(&source, &cache)
            };
            path.and_then(|p| decode(&p))
        })
    }

    /// Draws the PNG file at `path`, read once on a thread; `false` while it
    /// is read or when it cannot be.
    pub(crate) fn picture(&self, ui: &mut Ui, path: &Path) -> bool {
        let mut hasher = DefaultHasher::new();
        ("picture", path).hash(&mut hasher);
        let path = path.to_owned();
        self.image(ui, hasher.finish(), Kind::Picture, move || decode(&path))
    }

    /// Draws the image kept under `key`, once `read` on a thread made it.
    fn image(
        &self,
        ui: &mut Ui,
        key: u64,
        kind: Kind,
        read: impl FnOnce() -> Result<ColorImage, String> + Send + 'static,
    ) -> bool {
        self.take_drawn(ui.ctx());
        let Ok(mut states) = self.states.lock() else {
            return false;
        };
        match states.get(&key) {
            Some(State::Drawn(texture)) => {
                let response = ui
                    .add(
                        egui::Image::new((texture.id(), shown_size(texture.size_vec2(), ui)))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::ZoomIn);
                if response.clicked() {
                    let texture = texture.clone();
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new(crate::viewer::OPEN), Some(texture))
                    });
                }
                true
            }
            Some(State::Failed(why)) => {
                ui.label(
                    egui::RichText::new(format!("{}: {why}", kind.failed()))
                        .small()
                        .color(crate::theme::DIM),
                );
                false
            }
            Some(State::Drawing) => {
                ui.label(
                    egui::RichText::new(kind.waiting())
                        .small()
                        .color(crate::theme::DIM),
                );
                false
            }
            None => {
                states.insert(key, State::Drawing);
                let (drawn, ctx) = (Arc::clone(&self.drawn), ui.ctx().clone());
                std::thread::spawn(move || {
                    let image = read();
                    if let Ok(mut drawn) = drawn.lock() {
                        drawn.push((key, image));
                    }
                    ctx.request_repaint();
                });
                false
            }
        }
    }

    /// Turns the images drawn into textures.
    fn take_drawn(&self, ctx: &egui::Context) {
        let ready: Vec<_> = self
            .drawn
            .lock()
            .map(|mut d| std::mem::take(&mut *d))
            .unwrap_or_default();
        let Ok(mut states) = self.states.lock() else {
            return;
        };
        for (key, image) in ready {
            let state = match image {
                Ok(image) => State::Drawn(ctx.load_texture(
                    format!("diagram-{key:016x}"),
                    image,
                    TextureOptions::LINEAR,
                )),
                Err(why) => State::Failed(why),
            };
            states.insert(key, state);
        }
    }
}

/// The most a tall image takes of the conversation, in points: a click
/// shows it whole.
const TALLEST: f32 = 420.0;

/// How large an image `size` pixels shows in `ui`: at its own size, one of
/// its pixels to one of the screen's, narrowed to the room there is and
/// shortened to [`TALLEST`].
fn shown_size(size: egui::Vec2, ui: &Ui) -> egui::Vec2 {
    fit_within(size / ui.pixels_per_point(), ui.available_width())
}

fn fit_within(own: egui::Vec2, width: f32) -> egui::Vec2 {
    let mut shown = own;
    if shown.x > width {
        shown *= width / shown.x;
    }
    if shown.y > TALLEST {
        shown *= TALLEST / shown.y;
    }
    shown
}

/// The PNG at `path` as pixels.
fn decode(path: &std::path::Path) -> Result<ColorImage, String> {
    let image = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drawn_png_is_read_as_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let pixels = decode(&path).unwrap();
        assert_eq!(pixels.size, [3, 2]);
        assert!(decode(&dir.path().join("none.png")).is_err());
    }

    #[test]
    fn an_image_keeps_its_own_size_within_the_width_and_a_tall_one_is_capped() {
        let at = |x: f32, y: f32| fit_within(egui::vec2(x, y), 600.0);
        assert_eq!(at(300.0, 200.0), egui::vec2(300.0, 200.0));
        assert_eq!(at(1200.0, 400.0), egui::vec2(600.0, 200.0));
        assert_eq!(at(300.0, 840.0), egui::vec2(150.0, 420.0));
    }
}
