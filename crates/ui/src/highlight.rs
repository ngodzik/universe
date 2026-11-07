//! Syntax colouring for open files, with Sublime Text grammars through syntect.

use std::path::Path;

use crate::style::Rgb;
use syntect::highlighting::{
    HighlightIterator, HighlightState, Highlighter as Painter, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// A dark theme that reads well on the window's dark background. Only its
/// foreground colours are used: the window keeps its own background.
const THEME: &str = "base16-ocean.dark";

/// Beyond this a file is shown plain: a generated file that long is not
/// read for its colours.
const MAX_LINES: usize = 100_000;

/// A line as coloured runs of text.
pub(crate) type StyledLine = Vec<(Rgb, String)>;

/// Where the grammar and the theme stand at the start of a line: all that
/// colouring a line needs from the lines before it.
type Start = (ParseState, HighlightState);

/// A file's lines, coloured as far as they were shown: a file opens with
/// only what is in sight coloured, and what comes into sight is coloured
/// from where the colouring stopped.
pub(crate) struct Highlighted {
    /// The lines.
    texts: Vec<String>,
    /// Each line as coloured runs; one plain run past `coloured`.
    styled: Vec<StyledLine>,
    /// How many lines, from the first, are coloured.
    coloured: usize,
    /// The start of the first line not coloured yet.
    next: Start,
}

/// A line not coloured yet: its text, plain.
fn plain(line: &str) -> StyledLine {
    vec![(Rgb(192, 197, 206), line.to_owned())]
}

impl Highlighted {
    /// The lines, coloured as far as [`Highlighted::colour_to`] was asked.
    pub(crate) fn lines(&self) -> &[StyledLine] {
        &self.styled
    }

    /// Colours the lines up to `end`, for them to be shown. `false` when
    /// the grammar fails on them, and they are better shown plain.
    pub(crate) fn colour_to(&mut self, highlighter: &Highlighter, end: usize) -> bool {
        let end = end.min(self.texts.len());
        let painter = Painter::new(&highlighter.theme);
        for i in self.coloured..end {
            let Some(line) = highlighter.colour(&painter, &self.texts[i], &mut self.next) else {
                return false;
            };
            self.styled[i] = line;
        }
        self.coloured = self.coloured.max(end);
        true
    }
}

/// The grammars and the theme, loaded once and shared by every file opened.
pub(crate) struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    pub(crate) fn new() -> Self {
        let mut themes = ThemeSet::load_defaults();
        Self {
            // Lines are handed over without their newline, which is what the
            // "nonewlines" grammars expect.
            syntaxes: SyntaxSet::load_defaults_nonewlines(),
            theme: themes.themes.remove(THEME).unwrap_or_default(),
        }
    }

    /// The grammar for `path`: by extension, then by file name (`Makefile`),
    /// then by first line (a `#!/usr/bin/env python3` shebang).
    fn syntax_for(&self, path: &Path, first_line: Option<&str>) -> Option<&SyntaxReference> {
        let by_name = |name: Option<&std::ffi::OsStr>| {
            name.and_then(|n| n.to_str())
                .and_then(|n| self.syntaxes.find_syntax_by_extension(n))
        };
        by_name(path.extension())
            .or_else(|| by_name(path.file_name()))
            .or_else(|| first_line.and_then(|l| self.syntaxes.find_syntax_by_first_line(l)))
    }

    /// `lines` ready to be coloured, none of them yet: they are as they are
    /// shown, with [`Highlighted::colour_to`]. `None` when the file is
    /// better shown plain: no known grammar, or too long.
    pub(crate) fn highlight(&self, path: &Path, lines: &[String]) -> Option<Highlighted> {
        if lines.len() > MAX_LINES {
            return None;
        }
        let syntax = self.syntax_for(path, lines.first().map(String::as_str))?;
        let painter = Painter::new(&self.theme);
        // Nothing coloured yet: what is shown is, when it is.
        Some(Highlighted {
            texts: lines.to_vec(),
            styled: lines.iter().map(|l| plain(l)).collect(),
            coloured: 0,
            next: (
                ParseState::new(syntax),
                HighlightState::new(&painter, ScopeStack::new()),
            ),
        })
    }

    /// Colours one line from `state`, the start of the line, which becomes
    /// the start of the next.
    fn colour(&self, painter: &Painter, line: &str, state: &mut Start) -> Option<StyledLine> {
        let ops = state.0.parse_line(line, &self.syntaxes).ok()?;
        Some(
            HighlightIterator::new(&mut state.1, &ops, line, painter)
                .map(|(style, text)| {
                    let c = style.foreground;
                    (Rgb(c.r, c.g, c.b), text.to_owned())
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    /// `lines` coloured whole, as a file shown from top to bottom.
    fn whole(highlighter: &Highlighter, path: &Path, lines: &[String]) -> Highlighted {
        let mut highlighted = highlighter.highlight(path, lines).unwrap();
        assert!(highlighted.colour_to(highlighter, lines.len()));
        highlighted
    }

    #[test]
    fn python_keywords_and_strings_get_different_colours() {
        let highlighter = Highlighter::new();
        let styled = whole(
            &highlighter,
            Path::new("hello.py"),
            &lines("def greet():\n    return \"hi\""),
        );

        let colour_of = |line: &StyledLine, word: &str| {
            line.iter()
                .find(|(_, text)| text.contains(word))
                .map(|(c, _)| *c)
        };
        let keyword = colour_of(&styled.lines()[0], "def").unwrap();
        let string = colour_of(&styled.lines()[1], "hi").unwrap();
        assert_ne!(keyword, string);

        // Colouring never changes the text.
        let rebuilt: String = styled.lines()[1].iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(rebuilt, "    return \"hi\"");
    }

    #[test]
    fn a_shebang_is_enough_without_an_extension() {
        let highlighter = Highlighter::new();
        assert!(
            highlighter
                .highlight(Path::new("run"), &lines("#!/usr/bin/env python3\nprint(1)"))
                .is_some()
        );
    }

    #[test]
    fn unknown_files_stay_plain() {
        let highlighter = Highlighter::new();
        assert!(
            highlighter
                .highlight(Path::new("notes.unknownext"), &lines("just text"))
                .is_none()
        );
    }

    /// Each edit, coloured from where it is, reads as the whole coloured
    /// again: the line typed on alone, or as far as an opened comment
    /// reaches, lines added and taken away.
    /// A file opens with nothing coloured; what is shown is coloured when
    /// shown, and all ends as the whole.
    #[test]
    fn only_what_is_shown_is_coloured() {
        let highlighter = Highlighter::new();
        let path = Path::new("a.rs");
        let mut now: Vec<String> = (0..100).map(|i| format!("let v{i} = {i};")).collect();
        now[5] = "/* a comment".into();
        now[8] = "*/".into();
        let mut highlighted = highlighter.highlight(path, &now).unwrap();
        assert!(highlighted.colour_to(&highlighter, 10));
        let all = whole(&highlighter, path, &now);
        assert_eq!(highlighted.lines()[..10], all.lines()[..10]);
        assert_eq!(highlighted.lines()[50], plain("let v50 = 50;"));
        assert!(highlighted.colour_to(&highlighter, now.len()));
        assert_eq!(highlighted.lines(), all.lines());
    }
}
