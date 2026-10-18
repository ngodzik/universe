//! The last frame of each page, kept between sessions: shown at once as
//! the page loads again, until it shows itself. Each is kept again every
//! time its page shows whole, so that it follows what changes it.

use std::path::Path;

use crate::Frame;

/// The index of the pages kept: each page's key, and its picture's file.
const INDEX: &str = "pages.json";

/// The most pages kept: the latest.
const KEPT: usize = 60;

/// Keeps `frame` as the last of page `key` in `folder`. A failure to write
/// is the cache's, not the preview's.
pub fn keep_page(folder: &Path, key: &str, frame: &Frame) {
    let Some(png) = crate::kept::png_of(frame) else {
        return;
    };
    let file = format!("{:016x}.png", fnv(key));
    let mut index = read_index(folder);
    index.retain(|(k, _)| k != key);
    index.push((key.to_owned(), file.clone()));
    while index.len() > KEPT {
        let (_, gone) = index.remove(0);
        let _ = std::fs::remove_file(folder.join(gone));
    }
    let text = serde_json::to_string(&index).unwrap_or_default();
    let _ = std::fs::create_dir_all(folder)
        .and_then(|()| std::fs::write(folder.join(&file), png))
        .and_then(|()| std::fs::write(folder.join(INDEX), text));
}

/// The latest `most` pages kept in `folder`, the latest last: each its key
/// and its last frame.
#[must_use]
pub fn kept_pages(folder: &Path, most: usize) -> Vec<(String, Frame)> {
    let mut index = read_index(folder);
    let from = index.len().saturating_sub(most);
    index
        .drain(from..)
        .enumerate()
        .filter_map(|(i, (key, file))| {
            let png = std::fs::read(folder.join(file)).ok()?;
            let picture = image::load_from_memory(&png).ok()?.to_rgba8();
            Some((
                key,
                Frame {
                    width: picture.width(),
                    height: picture.height(),
                    rgba: picture.into_raw(),
                    // Apart from the frames the browser numbers.
                    seq: u64::MAX - i as u64,
                },
            ))
        })
        .collect()
}

fn read_index(folder: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(folder.join(INDEX))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// FNV-1a: a key's file, the same everywhere.
fn fnv(key: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_s_last_frame_is_kept_and_kept_again() {
        let dir = tempfile::tempdir().unwrap();
        let frame = |shade: u8| Frame {
            width: 2,
            height: 2,
            rgba: vec![shade; 16],
            seq: 7,
        };
        keep_page(dir.path(), "/products|", &frame(10));
        keep_page(dir.path(), "/orders|", &frame(20));
        keep_page(dir.path(), "/products|", &frame(30));
        let kept = kept_pages(dir.path(), 5);
        let keys: Vec<&str> = kept.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["/orders|", "/products|"]);
        assert_eq!(kept[1].1.rgba[0], 30);
        assert_ne!(kept[0].1.seq, kept[1].1.seq);
        // Only the latest, when fewer are asked for.
        let latest = kept_pages(dir.path(), 1);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].0, "/products|");
    }
}
