//! How the window looks, as it was left, kept in `~/.universe/config.toml`
//! (or `$UNIVERSE_HOME/config.toml`).
//!
//! ```toml
//! text_size = 1.25
//! see_through = true
//! opacity = 0.7
//! background = "stars"
//! background_motion = "slow"
//! ```
//!
//! What else the file holds, written by another version, is kept as it is.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::backdrop::{Motion, Pattern};
use serde::{Deserialize, Serialize};

/// How the window looks when universe opens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Defaults {
    /// Whether the window lets what is behind it show through, as a
    /// terminal can, as Ctrl-M left it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub see_through: bool,
    /// How opaque the window is while see-through, from 0.2 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    /// How large the window's text is, as Cmd + and Cmd − left it; unsaid,
    /// 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_size: Option<f32>,
    /// The pattern behind the window's panels: stars, waves, grid or
    /// rain; unsaid, none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Pattern>,
    /// Whether it moves: still (unsaid) or slow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_motion: Option<Motion>,
    /// What else the file holds, kept as it is when the rest is saved.
    #[serde(flatten)]
    pub other: toml::Table,
}

impl Defaults {
    /// universe's own folder: `UNIVERSE_HOME`, else `~/.universe`; `None`
    /// without a home directory.
    pub fn home() -> Option<PathBuf> {
        Some(match std::env::var_os("UNIVERSE_HOME") {
            Some(home) => PathBuf::from(home),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".universe"),
        })
    }

    /// Where the defaults live, or `None` without a home directory.
    pub fn path() -> Option<PathBuf> {
        Self::home().map(|home| home.join("config.toml"))
    }

    /// Makes universe's own folder, created if missing, and all it holds
    /// readable by their owner only: its settings and the caches of the
    /// projects' maps are nobody else's. The folder shut, what is written in it later
    /// is out of reach too, and loses the others' rights at the next call.
    ///
    /// # Errors
    ///
    /// Any error creating the folder or reading it.
    pub fn keep_home_private() -> io::Result<()> {
        let person = std::env::var_os("HOME").map(PathBuf::from);
        match Self::home() {
            Some(home) => keep_private(&home, person.as_deref()),
            None => Ok(()),
        }
    }

    /// Reads the defaults at `path`; none when the file does not exist.
    ///
    /// # Errors
    ///
    /// A sentence for the person when the file cannot be read or is not
    /// valid: starting with other choices than theirs would be worse.
    pub fn load(path: &Path) -> Result<Self, String> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes the defaults to `path`, through a temporary file so that a
    /// crash never leaves half of one.
    ///
    /// # Errors
    ///
    /// Any error writing the file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        // What another version kept there may be private: its owner's only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(tmp, path)
    }
}

/// Makes `dir`, created if missing, and all it holds readable by their
/// owner only: group and others lose every right, the owner keeps theirs,
/// an executable stays one. Links are not followed. Never the person's own
/// folder, `person`, nor one above it, should `UNIVERSE_HOME` name one.
fn keep_private(dir: &Path, person: Option<&Path>) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let dir = dir.canonicalize()?;
        if person.is_some_and(|person| resolved(person).starts_with(&dir)) {
            return Ok(());
        }
        let mut pending = vec![dir];
        while let Some(path) = pending.pop() {
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            let mode = meta.permissions().mode() & 0o7777;
            if mode & 0o077 != 0 {
                // Another's file, which only they may change, is left.
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o7700));
            }
            if meta.is_dir() {
                pending.extend(fs::read_dir(&path)?.flatten().map(|entry| entry.path()));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (dir, person);
    Ok(())
}

/// `path` with its links resolved as far as it exists, the rest as written:
/// compared with a folder resolved, it is compared on the same ground.
#[cfg(unix)]
fn resolved(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return path.to_owned(),
        }
    }
    let mut out = existing
        .canonicalize()
        .unwrap_or_else(|_| existing.to_owned());
    out.extend(rest.into_iter().rev());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn universes_folder_and_all_it_holds_are_its_owners_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777;
        let set = |path: &Path, mode: u32| {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        };
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".universe");
        let sessions = home.join("sessions/project");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("a.json"), "{}").unwrap();
        fs::write(home.join("hook.sh"), "true").unwrap();
        fs::write(dir.path().join("outside"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("outside"), home.join("link")).unwrap();
        for (path, wide) in [
            (&home, 0o755),
            (&sessions, 0o775),
            (&sessions.join("a.json"), 0o644),
            (&home.join("hook.sh"), 0o755),
            (&dir.path().join("outside"), 0o644),
        ] {
            set(path, wide);
        }

        keep_private(&home, Some(dir.path())).unwrap();
        assert_eq!(mode(&home), 0o700);
        assert_eq!(mode(&sessions), 0o700);
        assert_eq!(mode(&home.join("sessions")), 0o700);
        assert_eq!(mode(&sessions.join("a.json")), 0o600);
        // An executable stays one, for its owner.
        assert_eq!(mode(&home.join("hook.sh")), 0o700);
        // What a link leads to is not universe's.
        assert_eq!(mode(&dir.path().join("outside")), 0o644);

        // Missing, it is made shut.
        let new = dir.path().join("new");
        keep_private(&new, None).unwrap();
        assert_eq!(mode(&new), 0o700);

        // Never the person's own folder, nor one above it, even reached
        // through a link, as macOS's temporary folders are.
        let above = dir.path().join("above");
        fs::create_dir(&above).unwrap();
        set(&above, 0o755);
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&above, &link).unwrap();
        keep_private(&above, Some(&above.join("me"))).unwrap();
        keep_private(&link, Some(&link.join("me"))).unwrap();
        assert_eq!(mode(&above), 0o755);
    }

    #[test]
    fn defaults_survive_a_save_and_a_load_and_the_rest_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join("config.toml");
        assert_eq!(Defaults::load(&path), Ok(Defaults::default()));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "model = \"a/cheap\"\n[other]\nkept = 1\n").unwrap();
        let mut defaults = Defaults::load(&path).unwrap();
        defaults.see_through = true;
        defaults.opacity = Some(0.7);
        defaults.text_size = Some(1.25);
        defaults.background = Some(Pattern::Rain);
        defaults.background_motion = Some(Motion::Slow);
        defaults.save(&path).unwrap();
        let read = Defaults::load(&path).unwrap();
        assert_eq!(read, defaults);
        assert_eq!(read.other["model"].as_str(), Some("a/cheap"));
        assert_eq!(read.other["other"]["kept"].as_integer(), Some(1));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "its owner's only");
        }
    }

    #[test]
    fn a_mistake_in_the_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "text_size = \"big\"").unwrap();
        assert!(Defaults::load(&path).unwrap_err().contains("text_size"));
    }
}
