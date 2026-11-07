//! The interface's state: the view of the project shown, its files, the
//! file open, and what each key and click does to them.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use universe_tools::{Definition, Found, Lookup};

use crate::backdrop::{Backdrop, Pattern};
use crate::command::{self, Command};
use crate::defaults::Defaults;
use crate::highlight::Highlighter;
use crate::input::{KeyCode, KeyEvent, KeyModifiers};
use crate::keymap::{self, Action, Focus, Mode};
use crate::reader::{Outcome, Reader};
use crate::tree::FileTree;

/// How opaque a see-through window is unless `opacity` says otherwise:
/// enough to read on any wallpaper, little enough to see it.
pub const DEFAULT_OPACITY: f32 = 0.78;

/// Below this, text on a busy wallpaper is lost.
const MIN_OPACITY: f32 = 0.2;

/// The opacities Ctrl-M goes through, after opaque.
const OPACITY_STEPS: [f32; 3] = [0.8, 0.6, 0.4];

/// The window's text sizes, a step apart.
const TEXT_SIZES: [f32; 9] = [0.75, 0.85, 1.0, 1.1, 1.25, 1.4, 1.6, 1.8, 2.0];

/// Lines moved by a half page scroll. Fixed rather than measured, so that the
/// state does not depend on the size of the window.
const HALF_PAGE: i32 = 10;

/// The views of the project, in the order Ctrl-N goes through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapView {
    /// Its files as stars in space, gathered into galaxies; its
    /// deployments a click away.
    Universe,
    /// Its design, flat: the components, what uses what, in layers.
    Plan,
    /// Its services: those it runs, what each serves, and who reaches
    /// whom, through an API, over MCP or by HTTP.
    Services,
    /// Its API: each route of its OpenAPI specs, what serves it and what
    /// calls it.
    Api,
    /// Its database: the tables its models define, their columns, and the
    /// foreign keys between them.
    Database,
}

impl MapView {
    /// The view after this one on Ctrl-N; none after the last, for the
    /// files.
    fn next(self) -> Option<Self> {
        match self {
            Self::Universe => Some(Self::Plan),
            Self::Plan => Some(Self::Services),
            Self::Services => Some(Self::Api),
            Self::Api => Some(Self::Database),
            Self::Database => None,
        }
    }

    /// What it shows and how it is used, for the status line.
    fn told(self) -> &'static str {
        match self {
            Self::Universe => {
                "The universe: drag to turn, scroll to come closer, click a star to centre it, \
                 a galaxy's name to enter it; Ctrl-N for the plan"
            }
            Self::Plan => {
                "The plan: what uses what, foundations at the bottom. Click a part to see its \
                 files, double-click to open it; Ctrl-N for the services"
            }
            Self::Services => {
                "The services: who calls the one in the middle on the left, what it calls on \
                 the right. Click a service or a link for what it holds; Ctrl-N for the API"
            }
            Self::Api => {
                "The API: each route of the OpenAPI specs, what serves it and what calls it. \
                 Click a route for its way through; Ctrl-N for the database"
            }
            Self::Database => {
                "The database: the tables the models define, foundations at the bottom, each \
                 foreign key from its column to the one it points at. Click a table for its \
                 columns; Ctrl-N for the files"
            }
        }
    }
}

/// What the state asks for and only the loop can do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Find where a name is defined, or used: asked of the language
    /// server, else read by patterns.
    Find(Lookup),
    /// Keep how the window looks for the next time it opens.
    SaveDefaults(Defaults),
}

/// A line being typed: the `:` command line, or a search.
#[derive(Debug, Clone, Default)]
pub struct LineEditor {
    text: String,
    cursor: usize,
}

impl LineEditor {
    /// The text being edited.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Where the cursor is, in characters from the start.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn byte(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn insert(&mut self, c: char) {
        let at = self.byte(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    fn insert_str(&mut self, text: &str) {
        for c in text.chars().filter(|c| !c.is_control()) {
            self.insert(c);
        }
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let at = self.byte(self.cursor);
            self.text.remove(at);
        }
    }

    fn delete_word(&mut self) {
        let before: Vec<char> = self.text.chars().take(self.cursor).collect();
        let mut start = before.len();
        while start > 0 && before[start - 1] == ' ' {
            start -= 1;
        }
        while start > 0 && before[start - 1] != ' ' {
            start -= 1;
        }
        let (from, to) = (self.byte(start), self.byte(self.cursor));
        self.text.replace_range(from..to, "");
        self.cursor = start;
    }

    fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    /// Replaces the text, the cursor at its end.
    fn set(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.text = text;
    }

    fn apply(&mut self, action: &Action) {
        let len = self.text.chars().count();
        match action {
            Action::Insert(c) => self.insert(*c),
            Action::Backspace => self.backspace(),
            Action::DeleteWord => self.delete_word(),
            Action::ClearLine => {
                self.take();
            }
            Action::Left => self.cursor = self.cursor.saturating_sub(1),
            Action::Right => self.cursor = (self.cursor + 1).min(len),
            Action::Home => self.cursor = 0,
            Action::End => self.cursor = len,
            _ => {}
        }
    }
}

/// The places of a name to choose from, when several were found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionChoice {
    /// The name.
    pub name: String,
    /// Whether these are its uses, rather than its definitions.
    pub uses: bool,
    /// What found them.
    pub by: String,
    /// Where it is defined or used, the most likely first.
    pub items: Vec<Definition>,
    /// The one chosen, by its place.
    pub selected: usize,
}

/// A place in a file, to come back to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    /// The file, from the project's root.
    path: PathBuf,
    /// The line, from 1.
    line: usize,
}

/// Everything on screen, and what each key and click does to it.
pub struct App {
    root: PathBuf,
    project: String,
    mode: Mode,
    focus: Focus,
    command: LineEditor,
    /// Tab completion of a `:` command, while it cycles.
    completion: Option<command::Completion>,
    search: LineEditor,
    /// The mode the search was started from, to go back to.
    search_from: Mode,
    tree: Option<FileTree>,
    file: Option<Reader>,
    /// Made the first time a file opens: its grammars take a while to load.
    highlighter: OnceCell<Rc<Highlighter>>,
    definitions: Option<DefinitionChoice>,
    /// Where `gd` left from, to come back to with Ctrl-O.
    back: Vec<Place>,
    /// Where going back left from, to go forward again.
    ahead: Vec<Place>,
    /// The view shown; none while the files are.
    map: Option<MapView>,
    /// The view the files were reached from, to go back to once they
    /// close.
    last_map: MapView,
    /// The list of shortcuts, while open, scrolled this many lines.
    keys_open: Option<usize>,
    /// The note in the status line until the next key, and whether it says
    /// something went wrong.
    notice: Option<(String, bool)>,
    /// A lookup was asked and has not come back.
    looking: bool,
    quit: bool,
    /// Ctrl-C was pressed once: a second quits.
    quit_armed: bool,
    /// How the window looks now.
    look: Defaults,
    /// How it looked when last kept.
    kept: Defaults,
    /// Why the window cannot let what is behind it show through, when it
    /// cannot: the system composes no transparency for it.
    opaque_only: Option<String>,
    /// Effects raised outside a key, by a click, for the loop.
    queued: Vec<Effect>,
}

impl App {
    /// The state of the window on the project at `root`, looking as `look`
    /// says: its universe shown first.
    pub fn new(root: PathBuf, look: Defaults) -> Self {
        // The folder name says which project this is; the full path only
        // crowds the screen.
        let project = root.file_name().map_or_else(
            || root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let mut app = Self {
            root,
            project,
            mode: Mode::Normal,
            focus: Focus::Tree,
            command: LineEditor::default(),
            completion: None,
            search: LineEditor::default(),
            search_from: Mode::Normal,
            tree: None,
            file: None,
            highlighter: OnceCell::new(),
            definitions: None,
            back: Vec::new(),
            ahead: Vec::new(),
            map: None,
            last_map: MapView::Universe,
            keys_open: None,
            notice: None,
            looking: false,
            quit: false,
            quit_armed: false,
            kept: look.clone(),
            look,
            opaque_only: None,
            queued: Vec::new(),
        };
        app.show_map(Some(MapView::Universe));
        app
    }

    // Read access for the view.

    /// The project's name, from its folder.
    pub fn project(&self) -> &str {
        &self.project
    }

    /// The project's folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The mode the keys type in.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The pane that receives movement keys, while the files show.
    pub fn focus(&self) -> Focus {
        self.focus
    }

    /// The `:` command line.
    pub fn command_line(&self) -> &LineEditor {
        &self.command
    }

    /// The search of the view shown, as typed so far.
    pub fn search_line(&self) -> &LineEditor {
        &self.search
    }

    /// What the view shown is searched for: empty when it is not, and it
    /// shows everything.
    pub fn map_search(&self) -> &str {
        self.search.text().trim()
    }

    /// The file tree, when open.
    pub fn tree(&self) -> Option<&FileTree> {
        self.tree.as_ref()
    }

    /// The file open.
    pub fn file(&self) -> Option<&Reader> {
        self.file.as_ref()
    }

    /// The places of a name to choose from, when several were found.
    pub fn definition_choice(&self) -> Option<&DefinitionChoice> {
        self.definitions.as_ref()
    }

    /// The view shown; none while the files are.
    pub fn map_view(&self) -> Option<MapView> {
        self.map
    }

    /// The list of shortcuts, while open, and how far it is scrolled.
    pub fn keys_open(&self) -> Option<usize> {
        self.keys_open
    }

    /// The note in the status line, and whether it says something went
    /// wrong.
    pub fn notice(&self) -> Option<(&str, bool)> {
        self.notice
            .as_ref()
            .map(|(text, bad)| (text.as_str(), *bad))
    }

    /// Whether a lookup was asked and has not come back: the window wakes
    /// to show it when it does.
    pub fn is_looking(&self) -> bool {
        self.looking
    }

    /// Whether the person asked to quit.
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The pattern behind the window's panels, and whether it moves.
    pub fn backdrop(&self) -> Backdrop {
        Backdrop {
            pattern: self.look.background,
            motion: self.look.background_motion.unwrap_or_default(),
        }
    }

    /// How large the window's text is: 1 at first.
    pub fn text_size(&self) -> f32 {
        self.look.text_size.unwrap_or(1.0)
    }

    /// How opaque the window's background is now: 1 unless see-through.
    pub fn opacity(&self) -> f32 {
        if self.look.see_through && self.opaque_only.is_none() {
            self.look
                .opacity
                .unwrap_or(DEFAULT_OPACITY)
                .clamp(MIN_OPACITY, 1.0)
        } else {
            1.0
        }
    }

    // What the window asks.

    /// Quits, as the window's close button asks.
    pub fn quit(&mut self) {
        self.quit = true;
    }

    /// The window cannot let what is behind it show through, for `why`:
    /// it stays opaque, and Ctrl-M says so.
    pub fn opaque_only(&mut self, why: &str) {
        self.opaque_only = Some(why.to_owned());
    }

    /// Shows `view`, or the files when none: what Ctrl-N does, and a click
    /// in a view that opens a file.
    pub fn show_map(&mut self, view: Option<MapView>) {
        if let Some(view) = view {
            self.last_map = view;
            self.notice = Some((view.told().to_owned(), false));
        }
        self.map = view;
        if view.is_none() && self.tree.is_none() && self.file.is_none() {
            self.tree = Some(FileTree::new(self.root.clone()));
            self.focus_on(Focus::Tree);
        }
    }

    /// Starts typing a search of the view shown, after what was kept: what
    /// Ctrl-F does, and what a click on a view's search box does.
    pub fn start_search(&mut self) {
        if self.map.is_none() {
            self.say("Ctrl-F searches the views (Ctrl-N); in a file, / searches it");
            return;
        }
        if self.mode != Mode::Search {
            self.search_from = self.mode;
            self.mode = Mode::Search;
        }
    }

    /// Clears the search of the view shown, and stops typing it.
    pub fn clear_search(&mut self) {
        self.search.take();
        if self.mode == Mode::Search {
            self.mode = self.search_from;
        }
    }

    /// Makes the window's text a step larger (`1`), smaller (`-1`), or as
    /// it was at first (`0`); kept for the next time.
    pub fn step_text_size(&mut self, step: i32) {
        let now = self.text_size();
        let at = TEXT_SIZES
            .iter()
            .position(|s| (*s - now).abs() < 0.01)
            .unwrap_or(TEXT_SIZES.iter().position(|s| *s >= now).unwrap_or(2));
        let next = match step {
            0 => 1.0,
            s if s > 0 => TEXT_SIZES[(at + 1).min(TEXT_SIZES.len() - 1)],
            _ => TEXT_SIZES[at.saturating_sub(1)],
        };
        self.look.text_size = ((next - 1.0f32).abs() > 0.01).then_some(next);
        self.say(&format!("Text at {:.0}%", next * 100.0));
        self.keep_look();
    }

    /// Says something in the status line.
    pub fn report_info(&mut self, text: &str) {
        self.say(text);
    }

    /// Says in the status line that something went wrong.
    pub fn report_error(&mut self, text: String) {
        self.notice = Some((text, true));
    }

    /// The effects raised outside a key, by a click, for the loop: one at a
    /// time, the oldest first.
    pub fn take_queued(&mut self) -> Option<Effect> {
        (!self.queued.is_empty()).then(|| self.queued.remove(0))
    }

    /// Opens a file of the project, from its root, at line `line`, counted
    /// from 1: where a route is served, say.
    pub fn open_path_at(&mut self, path: PathBuf, line: usize) {
        self.open_path(path);
        if let Some(file) = &mut self.file {
            file.go_to_line(line);
        }
    }

    /// Opens a file of the project, from its root, or by its whole path.
    pub fn open_path(&mut self, path: PathBuf) {
        // Kept from the root, as lookups and places name files.
        let path = path
            .strip_prefix(&self.root)
            .map(Path::to_owned)
            .unwrap_or(path);
        let highlighter = Rc::clone(self.highlighter.get_or_init(|| Rc::new(Highlighter::new())));
        self.file = Some(Reader::open(&self.root, path, highlighter));
        self.focus_on(Focus::File);
    }

    /// Closes the file open: the tree takes the focus, or, without one, the
    /// view the files were reached from shows again.
    pub fn close_file(&mut self) {
        self.file = None;
        if self.tree.is_some() {
            self.focus_on(Focus::Tree);
        } else if self.map.is_none() {
            self.show_map(Some(self.last_map));
        }
    }

    /// Scrolls `pane` by `lines`, down when positive, without moving the
    /// focus: the wheel scrolls what is under the pointer.
    pub fn wheel(&mut self, pane: Focus, lines: i32) {
        let focus = self.focus;
        self.focus = pane;
        self.move_focused(lines);
        self.focus = focus;
    }

    /// A click on the tree's entry `index`: it is chosen, a folder opens or
    /// closes, a file opens.
    pub fn click_tree(&mut self, index: usize) {
        self.focus_on(Focus::Tree);
        if let Some(tree) = &mut self.tree
            && index < tree.rows().len()
        {
            tree.select(index);
            self.open_selected();
        }
    }

    /// A click on the open file, `row` lines below the first shown and
    /// `column` characters into its text: the cursor goes there.
    pub fn click_file(&mut self, row: usize, column: usize) {
        self.focus_on(Focus::File);
        if let Some(file) = &mut self.file {
            file.click(row, column);
        }
    }

    /// Goes to where the name under `column` of the drawn `row` of the open
    /// file is defined: a Ctrl-click. The lookup is queued for the loop.
    pub fn definition_at(&mut self, row: usize, column: usize) {
        self.click_file(row, column);
        let word = self.file.as_ref().and_then(|f| {
            let (row, col) = f.cursor();
            f.word_at(row, col)
        });
        if let Some(word) = word
            && let Some(effect) = self.look_up(&word, false)
        {
            self.queued.push(effect);
        }
    }

    /// Goes to the place at `index` of those to choose from, for a click on
    /// it.
    pub fn choose_definition(&mut self, index: usize) {
        if let Some(choice) = self.definitions.take()
            && let Some(found) = choice.items.get(index)
        {
            self.go_to_found(found.clone());
        }
    }

    /// Pastes `text` where the keys type: the command line or the search.
    pub fn on_paste(&mut self, text: &str) {
        match self.mode {
            Mode::Command => self.command.insert_str(text),
            Mode::Search => self.search.insert_str(text),
            Mode::Normal => self.say("Paste in the command line (:) or a search (Ctrl-F)"),
        }
    }

    /// Goes to what a lookup found: at once when one place is the
    /// likeliest definition, else to choose among them.
    pub fn on_found(&mut self, found: Found) {
        self.looking = false;
        self.notice = None;
        let what = if found.uses { "use" } else { "definition" };
        match found.places.len() {
            0 => self.say(&format!("No {what} of {} found ({})", found.name, found.by)),
            1 if !found.uses => self.go_to_found(found.places[0].clone()),
            _ => {
                self.definitions = Some(DefinitionChoice {
                    name: found.name,
                    uses: found.uses,
                    by: found.by,
                    items: found.places,
                    selected: 0,
                });
            }
        }
    }

    // Keys.

    /// Does what a key asks for, and returns what only the loop can do.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let effect = self.key(key);
        self.keep_look();
        effect.or_else(|| self.take_queued())
    }

    fn key(&mut self, key: KeyEvent) -> Option<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let quitting = ctrl && key.code == KeyCode::Char('c');
        if !quitting {
            // Any other key between the two Ctrl-C means the person changed
            // their mind.
            self.quit_armed = false;
            self.notice = None;
            if self.keys_key(key) || self.definition_key(key) {
                return None;
            }
        }
        if self.map.is_none()
            && self.focus == Focus::File
            && self.mode == Mode::Normal
            && let Some(reader) = &mut self.file
        {
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            let idle = reader.is_idle();
            // At rest, Ctrl-O and Alt-← go back to where `gd` left from, as
            // in Vim; Alt-→ goes forward again.
            if idle
                && ((ctrl && key.code == KeyCode::Char('o')) || (alt && key.code == KeyCode::Left))
            {
                self.go_back();
                return None;
            }
            if idle && alt && key.code == KeyCode::Right {
                self.go_forward();
                return None;
            }
            // The window's own shortcuts work here too, and Tab leaves the
            // file when nothing is half typed.
            let window = ctrl
                && matches!(
                    key.code,
                    KeyCode::Char('c' | 'b' | 's' | 'a' | 'm' | 'n' | 'f' | 'x')
                );
            let pane = idle && key.code == KeyCode::Tab;
            if !window && !pane {
                return match reader.handle_key(key) {
                    Outcome::Stay => None,
                    Outcome::Close => {
                        self.close_file();
                        None
                    }
                    Outcome::Definition(name) => self.look_up(&name, false),
                    Outcome::Uses(name) => self.look_up(&name, true),
                    Outcome::Command(line) => self.run_command(&line),
                };
            }
        }
        if self.mode == Mode::Command && key.code == KeyCode::Tab {
            let line = self.command.text().to_owned();
            if let Some(done) = command::complete(&line, &mut self.completion, |l| {
                command::candidates(l, command::NAMES)
            }) {
                self.command.set(done);
            }
            return None;
        }
        self.completion = None;
        let action = keymap::action(self.mode, self.focus, key)?;
        self.on_action(action)
    }

    fn on_action(&mut self, action: Action) -> Option<Effect> {
        match action {
            // Esc while searching: the search is cleared, and the keys go
            // back to where they were.
            Action::Enter(Mode::Normal) if self.mode == Mode::Search => self.clear_search(),
            Action::Enter(mode) => {
                if mode == Mode::Command {
                    self.command.take();
                }
                self.mode = mode;
            }
            Action::Submit => match self.mode {
                Mode::Command => {
                    let line = self.command.take();
                    self.mode = Mode::Normal;
                    return self.run_command(&line);
                }
                // Enter keeps the search, and goes back to where it was.
                Mode::Search => self.mode = self.search_from,
                Mode::Normal => {}
            },
            Action::Move(lines) => self.move_focused(lines),
            Action::HalfPage(down) => self.move_focused(if down { HALF_PAGE } else { -HALF_PAGE }),
            Action::Top => match self.focus {
                Focus::Tree => {
                    if let Some(tree) = &mut self.tree {
                        tree.select(0);
                    }
                }
                Focus::File => self.move_focused(i32::MIN / 2),
            },
            Action::Bottom => match self.focus {
                Focus::Tree => {
                    if let Some(tree) = &mut self.tree {
                        tree.select_last();
                    }
                }
                Focus::File => self.move_focused(i32::MAX / 2),
            },
            Action::Open => self.open_selected(),
            Action::Collapse => {
                if let Some(tree) = &mut self.tree {
                    tree.close();
                }
            }
            Action::ToggleTree => {
                if self.map.is_some() {
                    self.show_files();
                } else if self.tree.take().is_some() {
                    if self.file.is_some() {
                        self.focus_on(Focus::File);
                    } else {
                        self.show_map(Some(self.last_map));
                    }
                } else {
                    self.tree = Some(FileTree::new(self.root.clone()));
                    self.focus_on(Focus::Tree);
                }
            }
            Action::FocusTree => self.show_files(),
            Action::ShowKeys => {
                self.keys_open = match self.keys_open {
                    Some(_) => None,
                    None => Some(0),
                };
            }
            Action::ClosePane => match self.focus {
                Focus::Tree => {
                    self.tree = None;
                    if self.file.is_some() {
                        self.focus_on(Focus::File);
                    } else {
                        self.show_map(Some(self.last_map));
                    }
                }
                Focus::File => self.close_file(),
            },
            Action::FocusNext => {
                if self.map.is_none() {
                    let next = match self.focus {
                        Focus::Tree if self.file.is_some() => Focus::File,
                        Focus::File if self.tree.is_some() => Focus::Tree,
                        same => same,
                    };
                    self.focus_on(next);
                }
            }
            Action::Quit => {
                if self.quit_armed {
                    self.quit = true;
                } else {
                    self.quit_armed = true;
                    self.say("Ctrl-C again to quit");
                }
            }
            // Ctrl-M: opaque, 80%, 60%, 40%, and opaque again.
            Action::ToggleSeeThrough if self.opaque_only.is_some() => {
                let why = self.opaque_only.clone().unwrap_or_default();
                self.say(&format!("The window stays opaque here: {why}"));
            }
            Action::ToggleSeeThrough => {
                let now = self.opacity();
                let next = OPACITY_STEPS
                    .iter()
                    .copied()
                    .find(|step| *step < now - 0.01);
                match next {
                    Some(step) => {
                        self.look.see_through = true;
                        self.look.opacity = Some(step);
                        self.say(&format!("Window at {:.0}%", step * 100.0));
                    }
                    None => {
                        self.look.see_through = false;
                        self.say("Opaque window");
                    }
                }
            }
            Action::NextMap => {
                let next = match self.map {
                    Some(view) => view.next(),
                    None => Some(MapView::Universe),
                };
                self.show_map(next);
            }
            Action::Search => self.start_search(),
            Action::NextBackdrop => {
                self.look.background = Pattern::next(self.look.background);
                let said = match self.look.background {
                    Some(pattern) => format!(
                        "Background: {} (Ctrl-X: the next one; :background slow moves it)",
                        pattern.name()
                    ),
                    None => "No background".to_owned(),
                };
                self.say(&said);
            }
            editing => match self.mode {
                Mode::Command => self.command.apply(&editing),
                Mode::Search => self.search.apply(&editing),
                Mode::Normal => {}
            },
        }
        None
    }

    fn run_command(&mut self, line: &str) -> Option<Effect> {
        let command = match command::parse(line) {
            Ok(command) => command,
            Err(e) => {
                self.report_error(e);
                return None;
            }
        };
        match command {
            Command::Quit => self.quit = true,
            Command::Help | Command::Keys => self.keys_open = Some(0),
            Command::Open(None) => {
                self.report_error("Which file? :open src/main.rs, from the project's root".into());
            }
            Command::Open(Some(path)) => {
                if self.root.join(&path).is_file() {
                    self.map = None;
                    self.open_path(PathBuf::from(path));
                } else {
                    self.report_error(format!("No file {path} in {}", self.project));
                }
            }
            Command::Background(None) => {
                let said = self.backdrop().describe();
                self.say(&said);
            }
            Command::Background(Some(words)) => {
                let mut backdrop = self.backdrop();
                match backdrop.apply(&words) {
                    Ok(said) => {
                        self.look.background = backdrop.pattern;
                        self.look.background_motion =
                            Some(backdrop.motion).filter(|m| *m != crate::backdrop::Motion::Still);
                        self.say(&said);
                    }
                    Err(e) => self.report_error(e),
                }
            }
        }
        None
    }

    // Helpers.

    fn say(&mut self, text: &str) {
        self.notice = Some((text.to_owned(), false));
    }

    /// Queues keeping how the window looks, when it changed since it was
    /// last kept.
    fn keep_look(&mut self) {
        if self.look != self.kept {
            self.kept = self.look.clone();
            self.queued.push(Effect::SaveDefaults(self.look.clone()));
        }
    }

    /// Shows the files, the tree chosen: opened, or read again.
    fn show_files(&mut self) {
        self.map = None;
        match &mut self.tree {
            Some(tree) => tree.refresh(),
            None => self.tree = Some(FileTree::new(self.root.clone())),
        }
        self.focus_on(Focus::Tree);
    }

    /// Moves the focus. Leaving a line being typed leaves its mode.
    fn focus_on(&mut self, focus: Focus) {
        self.focus = focus;
        if self.mode == Mode::Command {
            self.mode = Mode::Normal;
        }
    }

    fn move_focused(&mut self, lines: i32) {
        match self.focus {
            Focus::Tree => {
                if let Some(tree) = &mut self.tree {
                    tree.move_by(lines);
                }
            }
            Focus::File => {
                if let Some(file) = &mut self.file {
                    file.scroll_by(lines);
                }
            }
        }
    }

    fn open_selected(&mut self) {
        let Some(path) = self.tree.as_mut().and_then(FileTree::open) else {
            return;
        };
        if self.file.as_ref().is_some_and(|f| f.path() == path) {
            // Already open: the cursor stays where it was.
            self.focus_on(Focus::File);
            return;
        }
        self.open_path(path);
    }

    /// Keys while the list of shortcuts is open. Returns whether the key was
    /// for it. Ctrl-S itself goes through the keymap, to close it.
    fn keys_key(&mut self, key: KeyEvent) -> bool {
        let Some(offset) = self.keys_open else {
            return false;
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match key.code {
            KeyCode::Up => self.keys_open = Some(offset.saturating_sub(1)),
            KeyCode::Down => self.keys_open = Some(offset + 1),
            KeyCode::PageUp => self.keys_open = Some(offset.saturating_sub(10)),
            KeyCode::PageDown => self.keys_open = Some(offset + 10),
            _ => self.keys_open = None,
        }
        true
    }

    /// Keys while places are offered: the arrows choose, Enter goes, Esc
    /// gives up. Returns whether the key was theirs.
    fn definition_key(&mut self, key: KeyEvent) -> bool {
        let Some(choice) = &mut self.definitions else {
            return false;
        };
        let last = choice.items.len().saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => choice.selected = choice.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => choice.selected = (choice.selected + 1).min(last),
            KeyCode::Enter => {
                let index = choice.selected;
                self.choose_definition(index);
            }
            KeyCode::Esc | KeyCode::Char('q') => self.definitions = None,
            _ => {}
        }
        true
    }

    /// Where the cursor of the open file is, as a place to come back to.
    fn here(&self) -> Option<Place> {
        let file = self.file.as_ref()?;
        Some(Place {
            path: file.path().to_owned(),
            line: file.cursor().0 + 1,
        })
    }

    /// Goes to `place`, opening its file unless it is open.
    fn go_to_place(&mut self, place: &Place) {
        if self.here().is_none_or(|here| here.path != place.path) {
            self.open_path(place.path.clone());
        }
        if let Some(file) = &mut self.file {
            file.go_to_line(place.line);
        }
        self.focus_on(Focus::File);
    }

    /// Asks where `name`, under the cursor of the open file, is defined or,
    /// with `uses`, used: of the loop, which asks the language server.
    fn look_up(&mut self, name: &str, uses: bool) -> Option<Effect> {
        let file = self.file.as_ref()?;
        let (line, column) = file.cursor();
        let lookup = Lookup {
            name: name.to_owned(),
            path: file.path().to_owned(),
            line,
            column,
            text: file.lines().join("\n"),
            uses,
        };
        self.looking = true;
        self.say(&format!(
            "Looking for where {name} is {}…",
            if uses { "used" } else { "defined" }
        ));
        Some(Effect::Find(lookup))
    }

    /// Goes to a place found, remembering where it left from.
    fn go_to_found(&mut self, found: Definition) {
        if let Some(here) = self.here() {
            self.back.push(here);
            self.ahead.clear();
        }
        let place = Place {
            path: PathBuf::from(found.path),
            line: found.line,
        };
        self.go_to_place(&place);
    }

    /// Goes back to where the last `gd` left from.
    fn go_back(&mut self) {
        let Some(place) = self.back.pop() else {
            self.say("Nowhere to go back to: gd goes to a definition, Ctrl-O comes back");
            return;
        };
        if let Some(here) = self.here() {
            self.ahead.push(here);
        }
        self.go_to_place(&place);
    }

    /// Goes forward again to where going back left from.
    fn go_forward(&mut self) {
        let Some(place) = self.ahead.pop() else {
            return;
        };
        if let Some(here) = self.here() {
            self.back.push(here);
        }
        self.go_to_place(&place);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(app: &mut App, code: KeyCode) -> Option<Effect> {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(app: &mut App, c: char) -> Option<Effect> {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn project() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "one\ntwo\n").unwrap();
        let app = App::new(dir.path().to_owned(), Defaults::default());
        (dir, app)
    }

    /// A project in git where `b.py` uses `f`, defined in `a.py` and, as a
    /// decoy, in `c.py`.
    fn defined() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "x = 1\n\ndef f():\n    pass\n").unwrap();
        std::fs::write(dir.path().join("c.py"), "def f():\n    pass\n").unwrap();
        std::fs::write(dir.path().join("b.py"), "from a import f\n\nf()\n").unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let app = App::new(dir.path().to_owned(), Defaults::default());
        (dir, app)
    }

    #[test]
    fn ctrl_n_goes_through_the_views_then_the_files_then_the_universe_again() {
        let (_dir, mut app) = project();
        assert_eq!(app.map_view(), Some(MapView::Universe));
        let mut seen = Vec::new();
        for _ in 0..6 {
            ctrl(&mut app, 'n');
            seen.push(app.map_view());
        }
        assert_eq!(
            seen,
            [
                Some(MapView::Plan),
                Some(MapView::Services),
                Some(MapView::Api),
                Some(MapView::Database),
                None,
                Some(MapView::Universe),
            ]
        );
        // The files came with the tree chosen, not an empty window.
        assert!(app.tree().is_some());
    }

    #[test]
    fn ctrl_f_searches_the_view_enter_keeps_it_and_esc_clears_it() {
        let (_dir, mut app) = project();
        ctrl(&mut app, 'f');
        assert_eq!(app.mode(), Mode::Search);
        type_text(&mut app, "users");
        assert_eq!(app.map_search(), "users");
        press(&mut app, KeyCode::Enter);
        assert_eq!((app.mode(), app.map_search()), (Mode::Normal, "users"));
        ctrl(&mut app, 'f');
        press(&mut app, KeyCode::Esc);
        assert_eq!((app.mode(), app.map_search()), (Mode::Normal, ""));
        // Over the files, it says where searching is.
        ctrl(&mut app, 'a');
        ctrl(&mut app, 'f');
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.notice().is_some_and(|(n, _)| n.contains("/ searches")));
    }

    #[test]
    fn the_tree_opens_a_file_tab_goes_between_them_and_q_closes_it() {
        let (_dir, mut app) = project();
        ctrl(&mut app, 'a');
        assert_eq!((app.map_view(), app.focus()), (None, Focus::Tree));
        press(&mut app, KeyCode::Right); // expand src/
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // open src/lib.rs
        assert_eq!(app.focus(), Focus::File);
        assert_eq!(app.file().unwrap().lines(), ["one", "two"]);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus(), Focus::Tree);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus(), Focus::File);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.file().is_none());
        assert_eq!(app.focus(), Focus::Tree);
        // The tree closed too, the view the files were reached from shows.
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.map_view(), Some(MapView::Universe));
    }

    #[test]
    fn a_file_opened_from_a_view_closes_back_to_it() {
        let (_dir, mut app) = project();
        ctrl(&mut app, 'n');
        ctrl(&mut app, 'n');
        assert_eq!(app.map_view(), Some(MapView::Services));
        app.open_path_at(PathBuf::from("src/lib.rs"), 2);
        app.show_map(None);
        assert_eq!(app.file().unwrap().cursor(), (1, 0));
        assert!(app.tree().is_none());
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.map_view(), Some(MapView::Services));
    }

    #[test]
    fn gd_goes_to_the_definition_imported_ctrl_o_comes_back_alt_right_goes_again() {
        let (_dir, mut app) = defined();
        app.open_path_at(PathBuf::from("b.py"), 3);
        app.show_map(None);
        let here = |app: &App| {
            let file = app.file().unwrap();
            (
                file.path().to_string_lossy().into_owned(),
                file.cursor().0 + 1,
            )
        };
        press(&mut app, KeyCode::Char('g'));
        // gd asks the loop, which asks the language server, else git grep:
        // here, git grep, as the loop's fallback would.
        let effect = press(&mut app, KeyCode::Char('d'));
        let Some(Effect::Find(lookup)) = effect else {
            panic!("gd asks the loop to find, not {effect:?}");
        };
        assert_eq!(
            (lookup.name.as_str(), lookup.line, lookup.uses),
            ("f", 2, false)
        );
        assert_eq!(lookup.path, PathBuf::from("b.py"));
        assert!(app.is_looking());
        let lines: Vec<String> = lookup.text.lines().map(str::to_owned).collect();
        let places = universe_tools::definitions(app.root(), "f", &lookup.path, &lines);
        app.on_found(Found {
            name: "f".into(),
            uses: false,
            places,
            by: "git grep".into(),
        });
        assert!(!app.is_looking());
        // Two definitions, the one b.py imports first: offered to choose.
        let choice = app.definition_choice().unwrap();
        assert_eq!(
            choice
                .items
                .iter()
                .map(|d| d.path.as_str())
                .collect::<Vec<_>>(),
            ["a.py", "c.py"]
        );
        press(&mut app, KeyCode::Enter);
        assert!(app.definition_choice().is_none());
        assert_eq!(here(&app), ("a.py".to_owned(), 3));

        ctrl(&mut app, 'o');
        assert_eq!(here(&app), ("b.py".to_owned(), 3));
        app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(here(&app), ("a.py".to_owned(), 3));
        // Nothing further: the place stays.
        app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(here(&app), ("a.py".to_owned(), 3));
    }

    #[test]
    fn ctrl_c_twice_quits_and_another_key_in_between_does_not() {
        let (_dir, mut app) = project();
        ctrl(&mut app, 'c');
        assert!(!app.should_quit());
        press(&mut app, KeyCode::Down);
        ctrl(&mut app, 'c');
        assert!(!app.should_quit());
        ctrl(&mut app, 'c');
        assert!(app.should_quit());
    }

    #[test]
    fn the_look_is_kept_once_it_changes() {
        let (_dir, mut app) = project();
        let saved = ctrl(&mut app, 'm');
        assert_eq!(app.opacity(), 0.8);
        let Some(Effect::SaveDefaults(kept)) = saved else {
            panic!("Ctrl-M keeps the look, not {saved:?}");
        };
        assert!(kept.see_through);
        for _ in 0..3 {
            ctrl(&mut app, 'm');
        }
        assert_eq!(app.opacity(), 1.0);
        // Moving changes nothing worth keeping.
        assert_eq!(press(&mut app, KeyCode::Down), None);
        app.step_text_size(1);
        assert!(matches!(
            app.take_queued(),
            Some(Effect::SaveDefaults(d)) if d.text_size == Some(1.1)
        ));
    }

    #[test]
    fn a_window_that_cannot_be_see_through_stays_opaque_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let look = Defaults {
            see_through: true,
            opacity: Some(0.6),
            ..Defaults::default()
        };
        let mut app = App::new(dir.path().to_owned(), look);
        assert_eq!(app.opacity(), 0.6);
        app.opaque_only("X11 composes no transparency for it");
        assert_eq!(app.opacity(), 1.0);
        // Nothing to keep: the look kept is left as it was.
        assert_eq!(ctrl(&mut app, 'm'), None);
        assert!(app.notice().is_some_and(|(n, _)| n.contains("X11")));
    }

    #[test]
    fn commands_open_a_file_set_the_background_and_say_what_they_do_not_know() {
        let (_dir, mut app) = project();
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "open src/lib.rs");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.map_view(), None);
        assert_eq!(app.file().unwrap().path(), Path::new("src/lib.rs"));
        // From inside the file, the reader hands universe's commands on.
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "bg rain slow");
        let saved = press(&mut app, KeyCode::Enter);
        assert_eq!(app.backdrop().pattern, Some(Pattern::Rain));
        assert!(matches!(saved, Some(Effect::SaveDefaults(_))));
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "model x");
        press(&mut app, KeyCode::Enter);
        assert!(
            app.notice()
                .is_some_and(|(n, bad)| bad && n.contains(":model"))
        );
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "open nowhere.rs");
        press(&mut app, KeyCode::Enter);
        assert!(app.notice().is_some_and(|(_, bad)| bad));
    }

    #[test]
    fn ctrl_s_shows_the_shortcuts_and_any_key_hides_them() {
        let (_dir, mut app) = project();
        ctrl(&mut app, 's');
        assert_eq!(app.keys_open(), Some(0));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.keys_open(), Some(1));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.keys_open(), None);
    }

    #[test]
    fn a_paste_goes_into_the_line_being_typed() {
        let (_dir, mut app) = project();
        ctrl(&mut app, 'f');
        app.on_paste("user\nid");
        assert_eq!(app.map_search(), "userid");
    }
}
