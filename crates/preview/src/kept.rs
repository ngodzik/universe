//! Workflows kept as tests: the screens shown, the state the back end
//! answered in, what the person did on the last screen and whether its
//! action was called, with a photograph of each screen and of the end. A
//! test shown again does it all again and compares each photograph with
//! the one kept, so that what a change does to the pages shows, and
//! whether the action is still reached.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::browser::Recorded;
use crate::error::PreviewError;
use crate::mock::Answer;

/// A workflow kept as a test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeptTest {
    /// What it does: `Create Product`.
    pub name: String,
    /// The action it ends with: its method.
    pub method: String,
    /// And its path in the spec.
    pub path: String,
    /// Its screens: each a label and a path of the front end's.
    pub steps: Vec<(String, String)>,
    /// The state the back end answered in.
    pub state: KeptState,
    /// What the person did on the last screen.
    pub inputs: Vec<Recorded>,
    /// Whether its action was called when it was kept.
    pub acted: bool,
    /// The page's size, in its pixels, and the scale it was drawn at.
    pub size: (u32, u32, f32),
}

/// The state the back end answered in, its operations by method and path,
/// which do not move as the spec changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptState {
    /// Every list empty.
    pub empty: bool,
    /// Every call slow.
    pub slow: bool,
    /// Every call failing, but those that tell who is there.
    pub failing: bool,
    /// No right granted.
    pub no_rights: bool,
    /// Every flag on.
    pub flags_on: bool,
    /// How some operations answered: their method, path and answer.
    pub answers: Vec<(String, String, Answer)>,
}

/// A test kept in a folder: its file, and what it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    /// Its file.
    pub file: PathBuf,
    /// The test.
    pub test: KeptTest,
}

impl Kept {
    /// The photograph of step `index` kept with it.
    #[must_use]
    pub fn step_picture(&self, index: usize) -> PathBuf {
        self.pictures().join(format!("{}.png", index + 1))
    }

    /// The photograph of its end kept with it.
    #[must_use]
    pub fn after_picture(&self) -> PathBuf {
        self.pictures().join("after.png")
    }

    fn pictures(&self) -> PathBuf {
        self.file.with_extension("")
    }
}

/// Keeps `test` in `folder`, with the photographs of its steps and, when
/// had, of its end.
///
/// # Errors
///
/// [`PreviewError::Write`] when a file cannot be written.
pub fn keep_test(
    folder: &Path,
    test: &KeptTest,
    steps: &[Vec<u8>],
    after: Option<&[u8]>,
) -> Result<Kept, PreviewError> {
    let file = folder.join(format!("{}.json", slug(&test.name)));
    let kept = Kept {
        file: file.clone(),
        test: test.clone(),
    };
    let write = |path: &Path, bytes: &[u8]| {
        std::fs::create_dir_all(path.parent().unwrap_or(folder))
            .and_then(|()| std::fs::write(path, bytes))
            .map_err(|source| PreviewError::Write {
                path: path.to_owned(),
                source,
            })
    };
    let text = serde_json::to_string_pretty(test).unwrap_or_default();
    write(&file, text.as_bytes())?;
    for (i, png) in steps.iter().enumerate() {
        write(&kept.step_picture(i), png)?;
    }
    if let Some(png) = after {
        write(&kept.after_picture(), png)?;
    }
    Ok(kept)
}

/// The tests kept in `folder`, by name.
#[must_use]
pub fn kept_tests(folder: &Path) -> Vec<Kept> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut kept: Vec<Kept> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|file| {
            let text = std::fs::read_to_string(&file).ok()?;
            let test = serde_json::from_str(&text).ok()?;
            Some(Kept { file, test })
        })
        .collect();
    kept.sort_by(|a, b| a.test.name.cmp(&b.test.name));
    kept
}

/// The share of the pixels two photographs differ by, those a little
/// different (a font's edge drawn otherwise) aside: 0 for the same, 1
/// when their sizes differ. `None` when one is no picture.
#[must_use]
pub fn difference(kept: &[u8], now: &[u8]) -> Option<f32> {
    let a = image::load_from_memory(kept).ok()?.to_rgba8();
    let b = image::load_from_memory(now).ok()?.to_rgba8();
    if a.dimensions() != b.dimensions() {
        return Some(1.0);
    }
    let total = (a.width() as usize * a.height() as usize).max(1);
    let differing = a
        .pixels()
        .zip(b.pixels())
        .filter(|(p, q)| p.0.iter().zip(q.0.iter()).any(|(x, y)| x.abs_diff(*y) > 32))
        .count();
    Some(differing as f32 / total as f32)
}

/// A frame of the page as a PNG picture.
#[must_use]
pub fn png_of(frame: &crate::Frame) -> Option<Vec<u8>> {
    let picture = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba.clone())?;
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .ok()?;
    Some(bytes)
}

/// A name as a file's: `create-product`.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() {
        "test".to_owned()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{Button, Input, Pointer};

    fn png(width: u32, height: u32, red: u8) -> Vec<u8> {
        let picture = image::RgbaImage::from_pixel(width, height, image::Rgba([red, 10, 10, 255]));
        let mut bytes = Vec::new();
        picture
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn a_test_is_kept_listed_and_its_pictures_compared() {
        let dir = tempfile::tempdir().unwrap();
        let test = KeptTest {
            name: "Create Product!".to_owned(),
            method: "POST".to_owned(),
            path: "/v1/products/".to_owned(),
            steps: vec![("/products".to_owned(), "/products".to_owned())],
            state: KeptState {
                empty: true,
                answers: vec![("GET".to_owned(), "/v1/me".to_owned(), Answer::Status(403))],
                ..KeptState::default()
            },
            inputs: vec![Recorded {
                after_ms: 300,
                input: Input::Pointer(Pointer::Press {
                    x: 10.0,
                    y: 20.0,
                    button: Button::Left,
                    clicks: 1,
                }),
            }],
            acted: true,
            size: (800, 600, 1.0),
        };
        let kept = keep_test(dir.path(), &test, &[png(4, 4, 200)], Some(&png(4, 4, 10))).unwrap();
        assert!(kept.file.ends_with("create-product.json"));
        assert!(kept.step_picture(0).ends_with("create-product/1.png"));
        let listed = kept_tests(dir.path());
        assert_eq!(listed, std::slice::from_ref(&kept));

        let step = std::fs::read(kept.step_picture(0)).unwrap();
        assert_eq!(difference(&step, &png(4, 4, 200)), Some(0.0));
        assert_eq!(difference(&step, &png(4, 4, 210)), Some(0.0));
        assert_eq!(difference(&step, &png(4, 4, 20)), Some(1.0));
        assert_eq!(difference(&step, &png(8, 4, 200)), Some(1.0));
        assert_eq!(difference(&step, b"no picture"), None);
    }
}
