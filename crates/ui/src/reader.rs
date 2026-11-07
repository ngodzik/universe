//! The file opened from the tree or from a star, read only.
//!
//! Its keys live here rather than in [`keymap`](crate::keymap): inside the
//! file they follow Vim's for moving and reading. Covered: moving, words,
//! `gg` and `G`, visual mode (`v`, `V`) and `y` to copy to the system
//! clipboard, `yy` for a line, `/` search with `n` and `N`, `:42` to go to a
//! line, `gd` and `gr` to follow a name, `q` or `:q` to close. Nothing here
//! writes to the file.

use std::cell::{Cell, Ref, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::clipboard;
use crate::command;
use crate::highlight::{Highlighted, Highlighter, StyledLine};
use crate::input::{KeyCode, KeyEvent, KeyModifiers};

/// The reader's own mode, as in Vim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderMode {
    /// Moving around.
    Normal,
    /// `v` selects characters, `V` whole lines.
    Visual {
        /// Whether whole lines are selected, as with `V`.
        line: bool,
    },
    /// Typing a `:` command in the frame under the file.
    Command,
    /// Typing a `/` search in the frame under the file.
    Search,
}

/// What the interface should do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Stay,
    /// `q` or `:q`: close the file.
    Close,
    /// `gd`: go to where this name is defined.
    Definition(String),
    /// `gr`: list where this name is used.
    Uses(String),
    /// A `:` command the reader does not know, for universe to run, so
    /// that its commands work from inside a file too.
    Command(String),
}

/// Where the system clipboard is reached. A trait so that tests do not
/// touch the real one.
pub(crate) trait Clipboard {
    fn copy(&self, text: &str) -> Result<&'static str, String>;
}

/// The clipboard of the machine universe runs on.
pub(crate) struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn copy(&self, text: &str) -> Result<&'static str, String> {
        clipboard::copy(text)
    }
}

/// The selected region, ends included, start before end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub(crate) start: (usize, usize),
    pub(crate) end: (usize, usize),
    pub(crate) line: bool,
}

impl Selection {
    /// The selected characters of `row`, as a half-open range of columns,
    /// or `None` when the row is outside the selection.
    pub fn columns(&self, row: usize, len: usize) -> Option<(usize, usize)> {
        if row < self.start.0 || row > self.end.0 {
            return None;
        }
        if self.line {
            return Some((0, len.max(1)));
        }
        let from = if row == self.start.0 { self.start.1 } else { 0 };
        let to = if row == self.end.0 {
            self.end.1 + 1
        } else {
            len + 1
        };
        Some((from, to.max(from + 1)))
    }
}

/// One open file.
pub struct Reader {
    /// As it was opened: from the project's root.
    path: PathBuf,
    lines: Vec<String>,
    /// Coloured as the lines are shown: drawing them colours what comes
    /// into sight.
    styled: RefCell<Option<Highlighted>>,
    highlighter: Rc<Highlighter>,
    clipboard: Rc<dyn Clipboard>,
    row: usize,
    /// In characters, not bytes.
    col: usize,
    scroll: usize,
    /// Lines on screen, written by the view; movement keeps the cursor inside.
    height: Cell<usize>,
    /// Text columns on screen, written by the view, for mapping mouse clicks.
    width: Cell<usize>,
    mode: ReaderMode,
    /// Where visual mode started; the other end is the cursor.
    anchor: (usize, usize),
    prompt: String,
    /// The first key of `gg`, `gd`, `gr` or `yy`.
    pending: Option<char>,
    /// Tab completion of a `:` command, while it cycles.
    completion: Option<command::Completion>,
    last_search: Option<String>,
    message: Option<(String, bool)>,
}

/// What a file on disk holds, as lines; a binary or unreadable file as a
/// line saying so.
fn load(absolute: &Path) -> (Vec<String>, bool) {
    match fs::read(absolute) {
        Ok(bytes) if bytes.iter().take(8000).any(|b| *b == 0) => {
            (vec![format!("(binary file, {} bytes)", bytes.len())], false)
        }
        Ok(bytes) => {
            let lines: Vec<String> = String::from_utf8_lossy(&bytes)
                .lines()
                .map(str::to_owned)
                .collect();
            let lines = if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            };
            (lines, true)
        }
        Err(e) => (vec![format!("(cannot read: {e})")], false),
    }
}

impl Reader {
    pub(crate) fn open(root: &Path, path: PathBuf, highlighter: Rc<Highlighter>) -> Self {
        let (lines, text) = load(&root.join(&path));
        let styled = if text {
            highlighter.highlight(&path, &lines)
        } else {
            None
        };
        Self {
            path,
            lines,
            styled: RefCell::new(styled),
            highlighter,
            clipboard: Rc::new(SystemClipboard),
            row: 0,
            col: 0,
            scroll: 0,
            height: Cell::new(0),
            width: Cell::new(0),
            mode: ReaderMode::Normal,
            anchor: (0, 0),
            prompt: String::new(),
            pending: None,
            completion: None,
            last_search: None,
            message: None,
        }
    }

    #[cfg(test)]
    fn with_clipboard(mut self, clipboard: Rc<dyn Clipboard>) -> Self {
        self.clipboard = clipboard;
        self
    }

    /// Moves to line `line`, counted from 1.
    pub(crate) fn go_to_line(&mut self, line: usize) {
        let row = line
            .saturating_sub(1)
            .min(self.lines.len().saturating_sub(1));
        self.go_to(row);
    }

    /// The name under `col` of `row`: letters, digits, `_` and `$`, not
    /// starting with a digit.
    pub fn word_at(&self, row: usize, col: usize) -> Option<String> {
        let chars: Vec<char> = self.lines.get(row)?.chars().collect();
        let part = |c: &char| c.is_alphanumeric() || *c == '_' || *c == '$';
        let col = col.min(chars.len().checked_sub(1)?);
        if !part(&chars[col]) {
            return None;
        }
        let start = (0..=col).rev().take_while(|i| part(&chars[*i])).last()?;
        let end = (col..chars.len()).take_while(|i| part(&chars[*i])).last()?;
        let word: String = chars[start..=end].iter().collect();
        (!word.chars().next()?.is_ascii_digit()).then_some(word)
    }

    // Read access for the view.

    /// The file's path, from the project's root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The lines.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The lines in view, `height` at most from the first in view.
    pub fn in_view(&self, height: usize) -> std::ops::Range<usize> {
        let start = self.scroll.min(self.lines.len());
        start..(start + height).min(self.lines.len())
    }

    /// The lines coloured by their language, when it is known: those in
    /// view, and those seen before, coloured; the others plain until shown.
    pub fn styled(&self) -> Option<Ref<'_, [StyledLine]>> {
        {
            let mut styled = self.styled.borrow_mut();
            let end = self.scroll + self.view_height() + 1;
            if let Some(highlighted) = styled.as_mut()
                && !highlighted.colour_to(&self.highlighter, end)
            {
                *styled = None;
            }
        }
        Ref::filter_map(self.styled.borrow(), |s| s.as_ref().map(Highlighted::lines)).ok()
    }

    /// The cursor, as a row and a column in characters.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// The first line in view.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Records the size of the text area, line numbers excluded.
    pub fn set_viewport(&self, height: usize, width: usize) {
        self.height.set(height);
        self.width.set(width);
    }

    /// The reader's mode.
    pub fn mode(&self) -> ReaderMode {
        self.mode
    }

    /// What is typed after `:` or `/`, while it is.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// A message for the person, and whether it is an error.
    pub fn message(&self) -> Option<(&str, bool)> {
        self.message.as_ref().map(|(m, e)| (m.as_str(), *e))
    }

    /// The selection, in visual mode.
    pub fn selection(&self) -> Option<Selection> {
        let ReaderMode::Visual { line } = self.mode else {
            return None;
        };
        let (start, end) = if self.anchor <= (self.row, self.col) {
            (self.anchor, (self.row, self.col))
        } else {
            ((self.row, self.col), self.anchor)
        };
        Some(Selection { start, end, line })
    }

    /// Whether keys should go to the interface (Tab, the `,` leader) rather
    /// than to the reader: only in normal mode with nothing half typed.
    pub(crate) fn is_idle(&self) -> bool {
        self.mode == ReaderMode::Normal && self.pending.is_none()
    }

    /// The first column shown, so that the cursor stays on screen in long
    /// lines. Every line is shifted by the same amount, as in Vim with
    /// `nowrap`.
    pub fn left_offset(&self) -> usize {
        (self.col + 1).saturating_sub(self.width.get().max(1))
    }

    /// Scrolls the view by `lines`, dragging the cursor along, as the mouse
    /// wheel does.
    pub(crate) fn scroll_by(&mut self, lines: i32) {
        let last = self.lines.len().saturating_sub(1) as i64;
        self.scroll = (self.scroll as i64 + i64::from(lines)).clamp(0, last) as usize;
        let height = self.view_height();
        self.row = self
            .row
            .clamp(self.scroll, (self.scroll + height - 1).min(last as usize));
        self.clamp_col();
    }

    /// Puts the cursor where the mouse clicked: on the `row`th line in
    /// view, `column` characters into its text as shown.
    pub(crate) fn click(&mut self, row: usize, column: usize) {
        if matches!(self.mode, ReaderMode::Command | ReaderMode::Search) {
            self.mode = ReaderMode::Normal;
        }
        self.row = (self.scroll + row).min(self.lines.len() - 1);
        self.col = column + self.left_offset();
        self.clamp_col();
    }

    // Keys.

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match self.mode {
            ReaderMode::Normal => self.normal_key(key),
            ReaderMode::Visual { line } => {
                self.visual_key(key, line);
                Outcome::Stay
            }
            ReaderMode::Command | ReaderMode::Search => self.prompt_key(key),
        }
    }

    /// Cursor movement shared by normal and visual mode. Returns whether the
    /// key was a movement.
    fn motion(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half = (self.view_height() / 2).max(1) as i32;
        match key.code {
            KeyCode::Left | KeyCode::Char('h') if !ctrl => self.col = self.col.saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') if !ctrl => {
                self.col += 1;
                self.clamp_col();
            }
            KeyCode::Up | KeyCode::Char('k') if !ctrl => self.move_rows(-1),
            KeyCode::Down | KeyCode::Char('j') if !ctrl => self.move_rows(1),
            KeyCode::Char('d') if ctrl => self.move_rows(half),
            KeyCode::Char('u') if ctrl => self.move_rows(-half),
            KeyCode::PageDown => self.move_rows(half),
            KeyCode::PageUp => self.move_rows(-half),
            KeyCode::Home | KeyCode::Char('0') => self.col = 0,
            KeyCode::End | KeyCode::Char('$') => {
                self.col = usize::MAX;
                self.clamp_col();
            }
            KeyCode::Char('^') => self.col = self.first_non_blank(),
            KeyCode::Char('w') => self.word_forward(),
            KeyCode::Char('b') => self.word_backward(),
            KeyCode::Char('G') => self.go_to(self.lines.len() - 1),
            KeyCode::Char('n') => self.search_next(true),
            KeyCode::Char('N') => self.search_next(false),
            _ => return false,
        }
        true
    }

    fn normal_key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(first) = self.pending.take() {
            let word = self.word_at(self.row, self.col);
            match (first, key.code, word) {
                ('g', KeyCode::Char('d'), Some(word)) => return Outcome::Definition(word),
                ('g', KeyCode::Char('r'), Some(word)) => return Outcome::Uses(word),
                ('g', KeyCode::Char('g'), _) => self.go_to(0),
                ('y', KeyCode::Char('y'), _) => {
                    let text = format!("{}\n", self.lines[self.row]);
                    self.copy(&text, 1);
                }
                _ => {}
            }
            self.keep_visible();
            return Outcome::Stay;
        }
        self.message = None;
        if self.motion(key) {
            self.keep_visible();
            return Outcome::Stay;
        }
        match key.code {
            KeyCode::Char(c @ ('g' | 'y')) if !ctrl => self.pending = Some(c),
            KeyCode::Char('v') if !ctrl => self.start_visual(false),
            KeyCode::Char('V') if !ctrl => self.start_visual(true),
            KeyCode::Char(':') => self.open_prompt(ReaderMode::Command),
            KeyCode::Char('/') => self.open_prompt(ReaderMode::Search),
            KeyCode::Char('q') if !ctrl => return Outcome::Close,
            _ => {}
        }
        self.keep_visible();
        Outcome::Stay
    }

    fn visual_key(&mut self, key: KeyEvent, line: bool) {
        if let Some(first) = self.pending.take() {
            if (first, key.code) == ('g', KeyCode::Char('g')) {
                self.go_to(0);
            }
            self.keep_visible();
            return;
        }
        self.message = None;
        if self.motion(key) {
            self.keep_visible();
            return;
        }
        match key.code {
            KeyCode::Esc => self.mode = ReaderMode::Normal,
            KeyCode::Char('v') if line => self.mode = ReaderMode::Visual { line: false },
            KeyCode::Char('V') if !line => self.mode = ReaderMode::Visual { line: true },
            KeyCode::Char('v' | 'V') => self.mode = ReaderMode::Normal,
            KeyCode::Char('g') => self.pending = Some('g'),
            KeyCode::Char('o') => {
                let cursor = (self.row, self.col);
                (self.row, self.col) = self.anchor;
                self.anchor = cursor;
            }
            KeyCode::Char('y') => {
                if let Some(selection) = self.selection() {
                    let text = self.selected(selection);
                    let count = selection.end.0 + 1 - selection.start.0;
                    self.mode = ReaderMode::Normal;
                    (self.row, self.col) = selection.start;
                    self.copy(&text, count);
                }
            }
            _ => {}
        }
        self.keep_visible();
    }

    fn open_prompt(&mut self, mode: ReaderMode) {
        self.prompt.clear();
        self.mode = mode;
    }

    fn prompt_key(&mut self, key: KeyEvent) -> Outcome {
        if key.code != KeyCode::Tab {
            self.completion = None;
        }
        match key.code {
            KeyCode::Tab if self.mode == ReaderMode::Command => {
                if let Some(line) = command::complete(&self.prompt, &mut self.completion, |l| {
                    command::candidates(l, command::NAMES)
                }) {
                    self.prompt = line;
                }
            }
            KeyCode::Esc => self.mode = ReaderMode::Normal,
            KeyCode::Backspace => {
                if self.prompt.pop().is_none() {
                    self.mode = ReaderMode::Normal;
                }
            }
            KeyCode::Enter => {
                let text = std::mem::take(&mut self.prompt);
                let mode = self.mode;
                self.mode = ReaderMode::Normal;
                let outcome = if mode == ReaderMode::Command {
                    self.command(text.trim())
                } else {
                    if !text.is_empty() {
                        self.last_search = Some(text);
                    }
                    self.search_next(true);
                    Outcome::Stay
                };
                self.clamp_col();
                self.keep_visible();
                return outcome;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt.push(c);
            }
            _ => {}
        }
        Outcome::Stay
    }

    /// A `:` command: a line number, `$`, `q`, or one of universe's.
    fn command(&mut self, command: &str) -> Outcome {
        match command {
            "" => Outcome::Stay,
            "q" | "q!" | "quit" => Outcome::Close,
            "$" => {
                self.go_to(self.lines.len() - 1);
                Outcome::Stay
            }
            number if number.chars().all(|c| c.is_ascii_digit()) => {
                self.go_to_line(number.parse().unwrap_or(1));
                Outcome::Stay
            }
            other => Outcome::Command(other.to_owned()),
        }
    }

    /// The selected text: whole lines ending with a new line, else the
    /// characters selected.
    fn selected(&self, selection: Selection) -> String {
        let (first, last) = (selection.start.0, selection.end.0);
        if selection.line {
            let mut text = self.lines[first..=last].join("\n");
            text.push('\n');
            return text;
        }
        (first..=last)
            .map(|row| {
                let line = &self.lines[row];
                let len = line.chars().count();
                let (from, to) = selection.columns(row, len).unwrap_or((0, 0));
                line[byte_index(line, from)..byte_index(line, to.min(len))].to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Puts `text`, of `lines` lines, on the system clipboard, and says so.
    fn copy(&mut self, text: &str, lines: usize) {
        match self.clipboard.copy(text) {
            Ok(tool) => self.say(&format!(
                "{lines} line{} copied ({tool})",
                if lines == 1 { "" } else { "s" }
            )),
            Err(e) => self.say_error(&e),
        }
    }

    fn start_visual(&mut self, line: bool) {
        self.anchor = (self.row, self.col);
        self.mode = ReaderMode::Visual { line };
    }

    // Movement.

    fn view_height(&self) -> usize {
        match self.height.get() {
            0 => 20,
            h => h,
        }
    }

    fn line_len(&self) -> usize {
        self.lines[self.row].chars().count()
    }

    fn first_non_blank(&self) -> usize {
        self.lines[self.row]
            .chars()
            .take_while(|c| c.is_whitespace())
            .count()
    }

    /// The cursor sits on a character, so it stops one before the end.
    fn clamp_col(&mut self) {
        self.col = self.col.min(self.line_len().saturating_sub(1));
    }

    fn move_rows(&mut self, delta: i32) {
        let last = self.lines.len() as i64 - 1;
        self.row = (self.row as i64 + i64::from(delta)).clamp(0, last) as usize;
        self.clamp_col();
    }

    fn go_to(&mut self, row: usize) {
        self.row = row;
        self.col = self.first_non_blank();
        self.keep_visible();
    }

    fn keep_visible(&mut self) {
        let height = self.view_height().max(1);
        if self.row < self.scroll {
            self.scroll = self.row;
        } else if self.row >= self.scroll + height {
            self.scroll = self.row + 1 - height;
        }
    }

    fn word_forward(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut i = self.col;
        let class = |c: char| {
            if c.is_alphanumeric() || c == '_' {
                1
            } else if c.is_whitespace() {
                0
            } else {
                2
            }
        };
        if i < chars.len() {
            let start = class(chars[i]);
            while i < chars.len() && class(chars[i]) == start && start != 0 {
                i += 1;
            }
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() && self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.first_non_blank();
        } else {
            self.col = i;
            self.clamp_col();
        }
    }

    fn word_backward(&mut self) {
        if self.col == 0 {
            if self.row > 0 {
                self.row -= 1;
                self.col = usize::MAX;
                self.clamp_col();
            }
            return;
        }
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut i = self.col.min(chars.len());
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let word = |c: char| c.is_alphanumeric() || c == '_';
        if i > 0 {
            let in_word = word(chars[i - 1]);
            while i > 0 && !chars[i - 1].is_whitespace() && word(chars[i - 1]) == in_word {
                i -= 1;
            }
        }
        self.col = i;
    }

    fn search_next(&mut self, forward: bool) {
        let Some(pattern) = self.last_search.clone() else {
            self.say_error("No previous search");
            return;
        };
        let count = self.lines.len();
        // Scan whole lines from the cursor, wrapping around the file once.
        for step in 0..=count {
            let row = if forward {
                (self.row + step) % count
            } else {
                (self.row + count - step % count) % count
            };
            let line = &self.lines[row];
            let found = if step == 0 {
                let here = byte_index(line, self.col);
                if forward {
                    let from = byte_index(line, self.col + 1).max(here);
                    line[from..].find(&pattern).map(|i| from + i)
                } else {
                    line[..here].rfind(&pattern)
                }
            } else if forward {
                line.find(&pattern)
            } else {
                line.rfind(&pattern)
            };
            if let Some(at) = found {
                let col = line[..at].chars().count();
                self.row = row;
                self.col = col;
                self.say(&format!("/{pattern}"));
                return;
            }
        }
        self.say_error(&format!("Pattern not found: {pattern}"));
    }

    // Messages.

    fn say(&mut self, text: &str) {
        self.message = (!text.is_empty()).then(|| (text.to_owned(), false));
    }

    fn say_error(&mut self, text: &str) {
        self.message = Some((text.to_owned(), true));
    }
}

/// The byte offset of character `chars` in `line`, or its end.
fn byte_index(line: &str, chars: usize) -> usize {
    line.char_indices()
        .nth(chars)
        .map_or(line.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clipboard in memory.
    #[derive(Default)]
    struct Memory(RefCell<String>);

    impl Clipboard for Memory {
        fn copy(&self, text: &str) -> Result<&'static str, String> {
            *self.0.borrow_mut() = text.to_owned();
            Ok("memory")
        }
    }

    fn reader(text: &str) -> (tempfile::TempDir, Reader, Rc<Memory>) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.py"), text).unwrap();
        let clipboard = Rc::new(Memory::default());
        let reader = Reader::open(
            dir.path(),
            PathBuf::from("a.py"),
            Rc::new(Highlighter::new()),
        )
        .with_clipboard(clipboard.clone());
        (dir, reader, clipboard)
    }

    fn keys(reader: &mut Reader, keys: &str) -> Outcome {
        let mut outcome = Outcome::Stay;
        for c in keys.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                '\u{1b}' => KeyCode::Esc,
                c => KeyCode::Char(c),
            };
            outcome = reader.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        outcome
    }

    #[test]
    fn colon_number_and_search_move_the_cursor() {
        let (_dir, mut r, _) = reader("a\nfoo\nb\nfoo bar\n");
        keys(&mut r, ":3\n");
        assert_eq!(r.cursor(), (2, 0));
        keys(&mut r, "/foo\n");
        assert_eq!(r.cursor(), (3, 0));
        keys(&mut r, "n");
        assert_eq!(r.cursor(), (1, 0));
        keys(&mut r, "/nothing\n");
        assert_eq!(r.message(), Some(("Pattern not found: nothing", true)));
        keys(&mut r, ":$\n");
        assert_eq!(r.cursor(), (3, 0));
    }

    #[test]
    fn words_and_line_ends() {
        let (_dir, mut r, _) = reader("let total = price * 2\n");
        keys(&mut r, "ww");
        assert_eq!(r.cursor(), (0, 10));
        keys(&mut r, "b");
        assert_eq!(r.cursor(), (0, 4));
        keys(&mut r, "$");
        assert_eq!(r.cursor(), (0, 20));
        keys(&mut r, "0");
        assert_eq!(r.cursor(), (0, 0));
    }

    #[test]
    fn what_would_edit_a_file_changes_nothing_and_writes_nothing() {
        let (dir, mut r, _) = reader("keep\nthis\n");
        keys(&mut r, "ixddpuoA!\u{1b}");
        assert_eq!(r.lines(), ["keep", "this"]);
        assert_eq!(keys(&mut r, ":w\n"), Outcome::Command("w".into()));
        assert_eq!(
            fs::read_to_string(dir.path().join("a.py")).unwrap(),
            "keep\nthis\n"
        );
    }

    #[test]
    fn a_selection_or_a_line_is_copied_to_the_clipboard() {
        let (_dir, mut r, clipboard) = reader("a\nb\nc\n");
        keys(&mut r, "Vjy");
        assert_eq!(r.mode(), ReaderMode::Normal);
        assert_eq!(clipboard.0.borrow().as_str(), "a\nb\n");
        assert_eq!(r.message(), Some(("2 lines copied (memory)", false)));
        keys(&mut r, "Gyy");
        assert_eq!(clipboard.0.borrow().as_str(), "c\n");
        let (_dir, mut r, clipboard) = reader("one two\nthree\n");
        keys(&mut r, "wvjhhy");
        assert_eq!(clipboard.0.borrow().as_str(), "two\nthr");
    }

    #[test]
    fn a_name_is_followed_and_q_closes() {
        let (_dir, mut r, _) = reader("total = price(2)\n");
        assert_eq!(keys(&mut r, "wwgd"), Outcome::Definition("price".into()));
        assert_eq!(keys(&mut r, "gr"), Outcome::Uses("price".into()));
        assert_eq!(keys(&mut r, "q"), Outcome::Close);
        assert_eq!(keys(&mut r, ":q\n"), Outcome::Close);
    }

    #[test]
    fn a_binary_file_shows_what_it_is() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bin"), [0x7f, 0, 1]).unwrap();
        let r = Reader::open(
            dir.path(),
            PathBuf::from("bin"),
            Rc::new(Highlighter::new()),
        );
        assert_eq!(r.lines(), ["(binary file, 3 bytes)"]);
        assert!(r.styled().is_none());
    }

    #[test]
    fn the_wheel_and_a_click_move_the_cursor_within_the_view() {
        let text: String = (1..=50).map(|n| format!("line {n}\n")).collect();
        let (_dir, mut r, _) = reader(&text);
        r.set_viewport(10, 40);
        r.scroll_by(15);
        assert_eq!(r.scroll(), 15);
        assert_eq!(r.cursor().0, 15);
        assert_eq!(r.in_view(10), 15..25);
        r.click(3, 2);
        assert_eq!(r.cursor(), (18, 2));
    }
}
