//! What a preview started, written down while it runs: a universe stopped
//! without leaving it (killed, crashed) would leave its front end and its
//! browser running, and the next start stops them. A process is stopped
//! only while it still runs the very command written down, so that a
//! number the system gave since to another program is never touched.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The file a preview of the project at `root` writes down what it starts
/// in: in universe's own folder `base`, beside what is cached of it.
#[must_use]
pub fn leftovers_file(base: &Path, root: &Path) -> PathBuf {
    let folder = root.file_name().map_or_else(
        || "project".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    base.join("cache").join(folder).join("app-processes")
}

/// What a preview writes down, as it starts each thing.
pub(crate) struct Record {
    file: Option<PathBuf>,
}

impl Record {
    pub(crate) fn new(file: Option<PathBuf>) -> Self {
        Self { file }
    }

    /// Writes down a process, or a group of them led by `pid`, started with
    /// `command`. A failure to write is not the preview's.
    pub(crate) fn add(&self, group: bool, pid: u32, command: &str) {
        let Some(file) = &self.file else {
            return;
        };
        let line = format!(
            "{} {pid} {}\n",
            if group { "group" } else { "process" },
            command.replace('\n', " ")
        );
        let write = || -> std::io::Result<()> {
            use std::io::Write as _;
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(file)?
                .write_all(line.as_bytes())
        };
        let _ = write();
    }

    /// Forgets what was written down, all of it stopped.
    pub(crate) fn clear(&self) {
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// Stops what a preview written down in `file` left running, then forgets
/// it.
pub fn stop_leftovers(file: &Path) {
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    for line in text.lines() {
        let mut words = line.splitn(3, ' ');
        let (Some(kind), Some(pid), Some(command)) = (words.next(), words.next(), words.next())
        else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        if pid < 2 || !runs(pid, command) {
            continue;
        }
        let target = if kind == "group" {
            format!("-{pid}")
        } else {
            pid.to_string()
        };
        for signal in ["TERM", "KILL"] {
            let _ = Command::new("kill")
                .args(["-s", signal, "--", &target])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
    let _ = std::fs::remove_file(file);
}

/// Whether process `pid` runs `command` still.
fn runs(pid: u32, command: &str) -> bool {
    let Ok(output) = Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    let args = String::from_utf8_lossy(&output.stdout);
    let args = args.trim();
    !args.is_empty() && args.contains(command.trim())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn what_was_left_is_stopped_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let file = leftovers_file(dir.path(), Path::new("/code/shop"));
        assert!(file.ends_with("cache/shop/app-processes"));
        let mut left = Command::new("sleep").arg("97").spawn().unwrap();
        let mut other = Command::new("sleep").arg("98").spawn().unwrap();
        let record = Record::new(Some(file.clone()));
        record.add(false, left.id(), "sleep 97");
        // A number written down for a command it no longer runs.
        record.add(false, other.id(), "something else");
        stop_leftovers(&file);
        assert!(left.wait().is_ok_and(|s| !s.success()));
        assert!(other.try_wait().unwrap().is_none());
        assert!(!file.exists());
        let _ = other.kill();
        let _ = other.wait();
    }
}
