//! What is particular to one system, kept on this machine and never in a
//! repository: the repositories read together as one (its own, read as
//! code, and those linked to it, read for where it runs), the service
//! drawn in the middle, and the environments to list first. The code reads
//! only the tools' conventions; a system's own names live here.
//!
//! A system is declared by a file of `<base>/systems/`, the base being
//! universe's own folder, `~/.universe`:
//!
//! ```toml
//! repos = ["~/code/shop-api", "~/code/shop-web"]
//! linked = ["~/code/shop-deploy"]
//! centre = "api"
//! environments = [{ name = "prod", path = "clusters/prod/app" }]
//! ```
//!
//! A folder that no system holds is a system of its own, with the settings
//! an older version kept for it in `<base>/projects/<folder>.toml`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::repos::Repos;

/// The settings file of a project: `<base>/projects/<folder>.toml`, the
/// base being universe's own folder, `~/.universe`.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// let file = universe_codemap::settings_file(Path::new("/home/me/.universe"), Path::new("/code/shop"));
/// assert_eq!(file, Path::new("/home/me/.universe/projects/shop.toml"));
/// ```
#[must_use]
pub fn settings_file(base: &Path, root: &Path) -> PathBuf {
    base.join("projects")
        .join(format!("{}.toml", folder_name(root)))
}

/// Where what is read of a system is cached: `<base>/cache/<name>`.
#[must_use]
pub fn cache_folder(base: &Path, name: &str) -> PathBuf {
    base.join("cache").join(name.replace('/', "-"))
}

/// The folder of the files that declare systems: `<base>/systems`.
#[must_use]
pub fn systems_folder(base: &Path) -> PathBuf {
    base.join("systems")
}

fn folder_name(root: &Path) -> String {
    root.file_name().map_or_else(
        || "project".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// An environment named in the settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedEnvironment {
    /// Its name, as shown.
    pub name: String,
    /// Its folder, from the root of the repository that holds it.
    pub path: String,
}

/// A system's own settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// The repositories read as code besides the folder opened, `~`
    /// expanded.
    pub repos: Vec<PathBuf>,
    /// The repositories read with it for where it runs, `~` expanded.
    pub linked: Vec<PathBuf>,
    /// The service drawn in the middle.
    pub centre: Option<String>,
    /// The environments listed first, in this order.
    pub environments: Vec<NamedEnvironment>,
    /// What the settings say that could not be used, such as a linked
    /// folder that is not there.
    pub notes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    repos: Vec<String>,
    #[serde(default)]
    linked: Vec<String>,
    centre: Option<String>,
    #[serde(default)]
    environments: Vec<NamedEnvironment>,
    /// The `[app]` table an older version read, left aside.
    #[serde(default, rename = "app")]
    _app: Option<toml::Table>,
}

/// Why the settings could not be read.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// The file is there but could not be read.
    #[error("cannot read {path}")]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid, or names a setting that does not exist.
    #[error("{path} is not valid")]
    Parse {
        /// The file.
        path: PathBuf,
        /// What is wrong, and where.
        #[source]
        source: toml::de::Error,
    },
}

/// Reads the settings in `file`, `~` standing for `home`. No file is no
/// settings.
///
/// # Errors
///
/// When the file cannot be read, is not TOML, or names a setting that
/// does not exist (a typo is said rather than ignored).
pub fn read_settings(file: &Path, home: &Path) -> Result<Settings, SettingsError> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(source) => {
            return Err(SettingsError::Read {
                path: file.to_owned(),
                source,
            });
        }
    };
    let parsed: File = toml::from_str(&text).map_err(|source| SettingsError::Parse {
        path: file.to_owned(),
        source,
    })?;
    let mut notes = Vec::new();
    let mut folders = |written: &[String], what: &str| -> Vec<PathBuf> {
        written
            .iter()
            .map(|l| expand(l, home))
            .filter(|path| {
                let there = path.is_dir();
                if !there {
                    notes.push(format!(
                        "The {what} folder {} is not there, so it is not read",
                        path.display()
                    ));
                }
                there
            })
            .collect()
    };
    let repos = folders(&parsed.repos, "system's");
    let linked = folders(&parsed.linked, "linked");
    Ok(Settings {
        repos,
        linked,
        centre: parsed.centre,
        environments: parsed.environments,
        notes,
    })
}

/// `written` with `~` standing for `home`.
fn expand(written: &str, home: &Path) -> PathBuf {
    match written.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if written == "~" => home.to_owned(),
        None => PathBuf::from(written),
    }
}

/// The repositories read together as one: those read as code, its own,
/// and those linked to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct System {
    /// Its name, as the window shows it: its file's, else its folder's.
    pub name: String,
    /// The file that declares it or keeps its settings, if one does.
    pub file: Option<PathBuf>,
    /// Its repositories, its own first.
    pub repos: Repos,
    /// Its settings.
    pub settings: Settings,
}

impl System {
    /// The folder `root` alone, with no settings.
    #[must_use]
    pub fn alone(root: &Path) -> Self {
        Self {
            name: folder_name(root),
            file: None,
            repos: Repos::new(root, &[]),
            settings: Settings::default(),
        }
    }

    /// Whether `folder` is one of its repositories or inside one.
    fn holds(&self, folder: &Path) -> bool {
        let folder = folder.canonicalize().unwrap_or_else(|_| folder.to_owned());
        self.repos.repos.iter().any(|r| {
            let root = r.root.canonicalize().unwrap_or_else(|_| r.root.clone());
            folder.starts_with(root)
        })
    }
}

/// The system to open on `folders`: the folders together when there are
/// several; for one, the system a file of `<base>/systems` declares that
/// holds it, else the folder alone with the settings an older version kept
/// for it. `~` in the files stands for `home`.
///
/// # Errors
///
/// When the settings an older version kept for the folder cannot be read
/// or are not valid; a file of `<base>/systems` that cannot be read is only
/// said, in the notes, and the system it declares is not known.
pub fn open_system(
    base: Option<&Path>,
    folders: &[PathBuf],
    home: &Path,
) -> Result<System, SettingsError> {
    let [folder] = folders else {
        let names: Vec<String> = folders.iter().map(|f| folder_name(f)).collect();
        return Ok(System {
            name: names.join(" + "),
            file: None,
            repos: Repos::system(folders, &[]),
            settings: Settings::default(),
        });
    };
    let Some(base) = base else {
        return Ok(System::alone(folder));
    };
    let mut notes = Vec::new();
    let mut declared: Vec<PathBuf> = std::fs::read_dir(systems_folder(base))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect()
        })
        .unwrap_or_default();
    declared.sort();
    for file in declared {
        let name = file
            .file_stem()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let settings = match read_settings(&file, home) {
            Ok(settings) => settings,
            Err(e) => {
                notes.push(format!("{e}, so the system it declares is not known"));
                continue;
            }
        };
        let system = System {
            name,
            repos: Repos::system(&settings.repos, &settings.linked),
            file: Some(file),
            settings,
        };
        if system.holds(folder) {
            let mut system = system;
            system.settings.notes.extend(notes);
            return Ok(system);
        }
    }
    let file = settings_file(base, folder);
    let mut settings = read_settings(&file, home)?;
    settings.notes.extend(notes);
    // Read as code too, a repository an older version's settings name was
    // only ever read for where it runs.
    settings.repos.clear();
    Ok(System {
        name: folder_name(folder),
        repos: Repos::new(folder, &settings.linked),
        file: file.is_file().then_some(file),
        settings,
    })
}

/// Declares the system `name` of the repositories `repos`, read as code,
/// in a file of `<base>/systems`, `~` written for `home`. Returns the file.
///
/// # Errors
///
/// When the name is not a plain one, a system of that name is declared
/// already, or the file cannot be written.
pub fn save_system(
    base: &Path,
    name: &str,
    repos: &[PathBuf],
    home: &Path,
) -> std::io::Result<PathBuf> {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.');
    if !plain {
        return Err(std::io::Error::other(format!(
            "{name:?} is not a name for a system: letters, digits, - _ and . only"
        )));
    }
    let folder = systems_folder(base);
    std::fs::create_dir_all(&folder)?;
    let file = folder.join(format!("{name}.toml"));
    if file.exists() {
        return Err(std::io::Error::other(format!(
            "{} declares a system of that name already",
            file.display()
        )));
    }
    let written: Vec<String> = repos
        .iter()
        .map(|r| {
            let shown = match r.strip_prefix(home) {
                Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
                _ => r.display().to_string(),
            };
            format!("{:?}", shown)
        })
        .collect();
    std::fs::write(&file, format!("repos = [{}]\n", written.join(", ")))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_with_home_expanded_and_typos_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("code/deploy")).unwrap();
        let file = home.join("shop.toml");
        std::fs::write(
            &file,
            "linked = [\"~/code/deploy\", \"~/code/gone\"]\ncentre = \"api\"\nenvironments = [{ name = \"prod\", path = \"clusters/prod/app\" }]\n",
        )
        .unwrap();
        let settings = read_settings(&file, home).unwrap();
        assert!(settings.repos.is_empty());
        assert_eq!(settings.linked, [home.join("code/deploy")]);
        assert_eq!(settings.centre.as_deref(), Some("api"));
        assert_eq!(settings.environments[0].path, "clusters/prod/app");
        assert_eq!(settings.notes.len(), 1);

        std::fs::write(
            &file,
            "[app]\nfolder = \"web\"\nspec_command = \"make spec\"\ncookies = { session = \"simulated\" }\nenv = { PORT_OFFSET = \"1\" }\n",
        )
        .unwrap();
        // An older version's table is read and left aside.
        assert_eq!(read_settings(&file, home).unwrap(), Settings::default());

        std::fs::write(&file, "centr = \"api\"\n").unwrap();
        let error = read_settings(&file, home).unwrap_err();
        assert!(matches!(error, SettingsError::Parse { .. }));
        assert!(error.to_string().contains("shop.toml"));
        assert_eq!(
            read_settings(&home.join("none.toml"), home).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn a_folder_opens_the_system_that_holds_it_else_alone() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let base = home.join(".universe");
        for folder in ["code/api/app", "code/web", "code/deploy", "code/other"] {
            std::fs::create_dir_all(home.join(folder)).unwrap();
        }
        let file = save_system(
            &base,
            "shop",
            &[home.join("code/api"), home.join("code/web")],
            home,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "repos = [\"~/code/api\", \"~/code/web\"]\n"
        );
        assert!(save_system(&base, "shop", &[], home).is_err());
        assert!(save_system(&base, "../x", &[], home).is_err());
        std::fs::write(
            &file,
            "repos = [\"~/code/api\", \"~/code/web\"]\nlinked = [\"~/code/deploy\"]\ncentre = \"api\"\n",
        )
        .unwrap();
        // A broken file of another system is said, not in the way.
        std::fs::write(systems_folder(&base).join("broken.toml"), "repos = 1").unwrap();

        // From inside one of its repositories, or a linked one: the system.
        for folder in ["code/api/app", "code/deploy"] {
            let system = open_system(Some(&base), &[home.join(folder)], home).unwrap();
            assert_eq!(system.name, "shop");
            assert_eq!(system.file.as_deref(), Some(file.as_path()));
            let code: Vec<&str> = system.repos.code().map(|r| r.name.as_str()).collect();
            assert_eq!(code, ["api", "web"]);
            assert_eq!(system.repos.repos.len(), 3);
            assert_eq!(system.settings.centre.as_deref(), Some("api"));
            assert!(
                system
                    .settings
                    .notes
                    .iter()
                    .any(|n| n.contains("broken.toml"))
            );
        }

        // A folder no system holds: alone, with an older version's settings.
        std::fs::create_dir_all(base.join("projects")).unwrap();
        std::fs::write(
            base.join("projects/other.toml"),
            "linked = [\"~/code/deploy\"]\n",
        )
        .unwrap();
        let other = open_system(Some(&base), &[home.join("code/other")], home).unwrap();
        assert_eq!(other.name, "other");
        assert_eq!(other.repos.code().count(), 1);
        assert_eq!(other.repos.repos.len(), 2);

        // Several folders: those together, as they are.
        let both = open_system(
            Some(&base),
            &[home.join("code/web"), home.join("code/other")],
            home,
        )
        .unwrap();
        assert_eq!(both.name, "web + other");
        assert!(both.repos.prefixed());
        assert_eq!(cache_folder(&base, "a/b"), base.join("cache/a-b"));
    }
}
