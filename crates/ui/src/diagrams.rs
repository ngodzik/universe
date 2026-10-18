//! Mermaid diagrams drawn as PNG images by a program of the machine
//! (mermaid-cli, mmdr, or one the settings name), each once, kept in a
//! cache, so that a diagram is drawn once for every time it shows.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a diagram may take to draw.
const DRAW_TIMEOUT: Duration = Duration::from_secs(60);

/// Where drawn diagrams are kept: `~/.universe/cache/diagrams`.
#[must_use]
pub fn cache() -> Option<PathBuf> {
    crate::Defaults::path().map(|p| p.with_file_name("cache").join("diagrams"))
}

/// The program that draws Mermaid diagrams as PNG images, with its
/// arguments; `{input}` and `{output}` stand for the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renderer {
    program: String,
    args: Vec<String>,
}

impl Renderer {
    /// The renderer `configured` (`mermaid` in the configuration), or the
    /// first installed of mermaid-cli and mmdr; `None` when there is none,
    /// or the configuration says `[]`.
    pub fn find(configured: Option<&[String]>, installed: impl Fn(&str) -> bool) -> Option<Self> {
        if let Some(command) = configured {
            let (program, args) = command.split_first()?;
            return Some(Self {
                program: program.clone(),
                args: args.to_vec(),
            });
        }
        let words = |text: &str| text.split(' ').map(str::to_owned).collect();
        if installed("mmdc") {
            // Transparent, drawn for a dark background, at twice the size so
            // that it stays sharp when fitted.
            Some(Self {
                program: "mmdc".into(),
                args: words("-q -i {input} -o {output} -b transparent -t dark -s 2"),
            })
        } else if installed("mmdr") {
            Some(Self {
                program: "mmdr".into(),
                args: words("-i {input} -o {output} -e png"),
            })
        } else {
            None
        }
    }

    /// Whether `program` is a file in one of the directories of `PATH`.
    pub fn installed(program: &str) -> bool {
        std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
    }

    /// The key of the diagram of `source`: the same for the same source
    /// drawn the same way, from one session to the next.
    pub fn key(&self, source: &str) -> u64 {
        // FNV-1a: stable across versions, unlike the standard hasher.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let parts = std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .chain(std::iter::once(source));
        for part in parts {
            for byte in part.bytes().chain(std::iter::once(0)) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        hash
    }

    /// Draws the diagram of `source` into `cache`, waiting for it; the
    /// image, or why it could not be drawn.
    ///
    /// # Errors
    ///
    /// What the renderer said, or that it could not start or took too long.
    pub fn draw(&self, source: &str, cache: &Path) -> Result<PathBuf, String> {
        let key = self.key(source);
        std::fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
        let name = format!("{key:016x}");
        let input = cache.join(format!("{name}.mmd"));
        let output = cache.join(format!("{name}.part.png"));
        let done = cache.join(format!("{name}.png"));
        let log_path = cache.join(format!("{name}.log"));
        std::fs::write(&input, source).map_err(|e| e.to_string())?;
        let log = File::create(&log_path).map_err(|e| e.to_string())?;
        let args: Vec<String> = self
            .args
            .iter()
            .map(|a| {
                a.replace("{input}", &input.to_string_lossy())
                    .replace("{output}", &output.to_string_lossy())
            })
            .collect();
        let mut child = Command::new(&self.program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .map_err(|e| format!("{}: {e}", self.program))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() > DRAW_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{} took over a minute", self.program));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(e.to_string()),
            }
        };
        let said = std::fs::read_to_string(&log_path).unwrap_or_default();
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&log_path);
        if status.success() && output.is_file() {
            std::fs::rename(&output, &done).map_err(|e| e.to_string())?;
            return Ok(done);
        }
        let _ = std::fs::remove_file(&output);
        // The line that says what went wrong, else the last one.
        let lines: Vec<&str> = said
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let reason = lines
            .iter()
            .find(|l| l.to_lowercase().contains("error"))
            .or(lines.last())
            .map_or_else(
                || format!("{} failed ({status})", self.program),
                |l| (*l).to_owned(),
            );
        Err(reason)
    }
}

/// `source` with the colours a model set made readable on a dark theme: a
/// theme it asked for that is drawn for light backgrounds becomes the dark
/// one, a node's text gets the colour that stands out on its fill, and
/// strokes and text too dark for a dark background are lightened. What is
/// not a colour stays as written.
#[must_use]
pub fn readable(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let indent = &line[..line.len() - trimmed.len()];
            if trimmed.starts_with("%%{") {
                return format!("{indent}{}", dark_theme(trimmed));
            }
            let mut words = trimmed.splitn(3, char::is_whitespace);
            match (words.next(), words.next(), words.next()) {
                (
                    Some(keyword @ ("style" | "classDef" | "linkStyle")),
                    Some(name),
                    Some(styles),
                ) => {
                    let styles = styles.trim_end().trim_end_matches(';');
                    format!(
                        "{indent}{keyword} {name} {}",
                        readable_styles(styles, keyword == "linkStyle")
                    )
                }
                _ => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An init directive with its theme made the dark one, unless it is
/// already, or `base`, whose colours the directive sets itself.
fn dark_theme(directive: &str) -> String {
    let Some(at) = directive
        .find("theme\"")
        .or_else(|| directive.find("theme'"))
    else {
        return directive.to_owned();
    };
    let after = &directive[at + "theme'".len()..];
    let Some(colon) = after.find(':') else {
        return directive.to_owned();
    };
    let value = after[colon + 1..].trim_start();
    let Some(quote) = value.chars().next().filter(|c| *c == '\'' || *c == '"') else {
        return directive.to_owned();
    };
    let Some(end) = value[1..].find(quote) else {
        return directive.to_owned();
    };
    let theme = &value[1..=end];
    if matches!(theme, "dark" | "base") {
        return directive.to_owned();
    }
    let start = directive.len() - value.len() + 1;
    format!(
        "{}dark{}",
        &directive[..start],
        &directive[start + theme.len()..]
    )
}

/// The declarations of a style, `fill:#fff,stroke:#333`, made readable: a
/// link's are its line's.
fn readable_styles(styles: &str, link: bool) -> String {
    let mut declarations: Vec<(String, String)> = styles
        .split(',')
        .filter_map(|d| {
            let (key, value) = d.split_once(':')?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    let said = |key: &str, declarations: &[(String, String)]| {
        declarations
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| lightness(v))
    };
    let fill = if link {
        None
    } else {
        said("fill", &declarations)
    };
    let text = said("color", &declarations);
    let wanted = match (fill, text) {
        // Text that would not stand out on its fill, or none said: the one
        // that does.
        (Some(fill), text) if text.is_none_or(|t| (t - fill).abs() < 0.45) => {
            Some(if fill > 0.55 { DARK_TEXT } else { LIGHT_TEXT })
        }
        // On the theme's dark fill.
        (None, Some(text)) if text < 0.35 => Some(LIGHT_TEXT),
        _ => None,
    };
    if let Some(colour) = wanted {
        match declarations.iter_mut().find(|(k, _)| k == "color") {
            Some(declaration) => declaration.1 = colour.to_owned(),
            None => declarations.push(("color".to_owned(), colour.to_owned())),
        }
    }
    for (key, value) in &mut declarations {
        if key == "stroke" && lightness(value).is_some_and(|l| l < 0.25) {
            *value = LIGHT_STROKE.to_owned();
        }
    }
    declarations
        .iter()
        .map(|(k, v)| format!("{k}:{v}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Text on a light fill.
const DARK_TEXT: &str = "#16181d";
/// Text on a dark fill.
const LIGHT_TEXT: &str = "#eceff4";
/// A line that shows on a dark background.
const LIGHT_STROKE: &str = "#9aa3b2";

/// How light `colour` is, from 0 to 1: `#rgb`, `#rrggbb` (an alpha after
/// it ignored) or a common name; `None` for anything else.
fn lightness(colour: &str) -> Option<f32> {
    let colour = colour.trim().to_lowercase();
    let hex = match colour.strip_prefix('#') {
        Some(hex) => hex.to_owned(),
        None => NAMED
            .iter()
            .find(|(name, _)| *name == colour)
            .map(|(_, hex)| (*hex).to_owned())?,
    };
    let channel = |s: &str| u8::from_str_radix(s, 16).ok().map(f32::from);
    let (r, g, b) = match hex.len() {
        3 | 4 => {
            let at = |i: usize| channel(&hex[i..=i].repeat(2));
            (at(0)?, at(1)?, at(2)?)
        }
        6 | 8 => (
            channel(&hex[0..2])?,
            channel(&hex[2..4])?,
            channel(&hex[4..6])?,
        ),
        _ => return None,
    };
    Some((0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0)
}

/// The colour names models use most, as hex.
const NAMED: [(&str, &str); 16] = [
    ("white", "ffffff"),
    ("black", "000000"),
    ("red", "ff0000"),
    ("green", "008000"),
    ("blue", "0000ff"),
    ("yellow", "ffff00"),
    ("orange", "ffa500"),
    ("pink", "ffc0cb"),
    ("purple", "800080"),
    ("gray", "808080"),
    ("grey", "808080"),
    ("lightgray", "d3d3d3"),
    ("lightgrey", "d3d3d3"),
    ("lightblue", "add8e6"),
    ("lightgreen", "90ee90"),
    ("lightyellow", "ffffe0"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_renderer_is_the_configured_one_else_the_first_installed() {
        assert_eq!(Renderer::find(Some(&[]), |_| true), None);
        assert_eq!(Renderer::find(None, |_| false), None);
        assert_eq!(
            Renderer::find(None, |p| p == "mmdr").unwrap().program,
            "mmdr"
        );
        let mmdc = Renderer::find(None, |_| true).unwrap();
        assert_eq!(mmdc.program, "mmdc");
        assert!(mmdc.args.join(" ").contains("-b transparent"));
        // Another source, or another renderer, is another diagram.
        let mmdr = Renderer::find(None, |p| p == "mmdr").unwrap();
        assert_ne!(mmdc.key("a"), mmdc.key("b"));
        assert_ne!(mmdc.key("a"), mmdr.key("a"));
        assert_eq!(mmdc.key("a"), mmdc.key("a"));
    }

    #[cfg(unix)]
    #[test]
    fn draw_runs_the_renderer_and_says_why_it_failed() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("render.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\ncase \"$(cat \"$1\")\" in bad*) echo 'Error: Parse error on line 1' >&2; exit 1;; esac\ncp \"$1\" \"$2\"\n",
        )
        .unwrap();
        let renderer = Renderer {
            program: "sh".into(),
            args: vec![
                script.to_string_lossy().into_owned(),
                "{input}".into(),
                "{output}".into(),
            ],
        };
        let cache = dir.path().join("cache");
        let drawn = renderer.draw("graph", &cache).unwrap();
        assert_eq!(std::fs::read_to_string(&drawn).unwrap(), "graph");
        assert!(drawn.ends_with(format!("{:016x}.png", renderer.key("graph"))));
        assert_eq!(
            renderer.draw("bad", &cache),
            Err("Error: Parse error on line 1".into())
        );
        // Nothing is left behind but the drawn image.
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
    }

    #[test]
    fn a_models_colours_are_made_readable_on_the_dark_theme() {
        let source = "%%{init: {'theme': 'default'}}%%\ngraph TD\n  A-->B\n  style A fill:#fff,stroke:#333\n  classDef warm fill:#f9f,color:#fff;\n  classDef deep fill:#123,color:#ddd\n  style B color:#000\n  linkStyle 0 stroke:black";
        let made = readable(source);
        let lines: Vec<&str> = made.lines().collect();
        assert_eq!(lines[0], "%%{init: {'theme': 'dark'}}%%");
        assert_eq!(lines[2], "  A-->B");
        // Dark text on a light fill, a stroke that shows.
        assert_eq!(lines[3], "  style A fill:#fff,stroke:#9aa3b2,color:#16181d");
        // Light text on a light fill: made dark.
        assert_eq!(lines[4], "  classDef warm fill:#f9f,color:#16181d");
        // Light on dark already stands out.
        assert_eq!(lines[5], "  classDef deep fill:#123,color:#ddd");
        // Black text on the theme's dark fill.
        assert_eq!(lines[6], "  style B color:#eceff4");
        assert_eq!(lines[7], "  linkStyle 0 stroke:#9aa3b2");
        // Themes meant for dark, or that set their own colours, stay.
        let kept = "%%{init: {\"theme\": \"base\"}}%%";
        assert_eq!(readable(kept), kept);
        assert_eq!(readable("graph LR\n  a-->b"), "graph LR\n  a-->b");
    }
}
