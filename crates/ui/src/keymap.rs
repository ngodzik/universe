//! Which key does what, in which mode and which pane.
//!
//! Every binding lives in [`action`]. Making the bindings configurable later
//! means replacing this one function with a table read from a file; nothing
//! else in the interface knows about keys. Inside an open file, keys follow
//! Vim and are read by [`reader`](crate::reader).

use crate::input::{KeyCode, KeyEvent, KeyModifiers};

/// What the keys type into, as in Vim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Moving around the views and the panes.
    Normal,
    /// Typing a `:` command.
    Command,
    /// Typing a search of the view shown, which keeps what it names as it
    /// is typed.
    Search,
}

/// The pane that receives movement keys, while the files are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The file tree on the left.
    Tree,
    /// The open file.
    File,
}

/// What a key asks for, independent of which key it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Switch mode.
    Enter(Mode),
    /// Type a character.
    Insert(char),
    /// Delete the character before the cursor.
    Backspace,
    /// Delete the word before the cursor.
    DeleteWord,
    /// Clear the line.
    ClearLine,
    /// Move the cursor one character left.
    Left,
    /// Move the cursor one character right.
    Right,
    /// Move the cursor to the start of the line.
    Home,
    /// Move the cursor to the end of the line.
    End,
    /// Run the command, or keep the search.
    Submit,
    /// Move in the focused pane by this many lines; positive is down.
    Move(i32),
    /// Move by half a screen; `true` is down.
    HalfPage(bool),
    /// Jump to the top of the focused pane.
    Top,
    /// Jump to the bottom of the focused pane.
    Bottom,
    /// Open the selected tree row: expand a directory, show a file.
    Open,
    /// Collapse the selected directory, or go to its parent.
    Collapse,
    /// Show or hide the file tree.
    ToggleTree,
    /// Show the files and go to the tree, opening it if it is hidden.
    FocusTree,
    /// Show or hide the list of every shortcut.
    ShowKeys,
    /// Close the focused pane.
    ClosePane,
    /// Focus the next pane, wrapping around.
    FocusNext,
    /// Leave universe, once asked twice.
    Quit,
    /// Let what is behind the window show through, or make it opaque again.
    ToggleSeeThrough,
    /// Show the next of the project's views, then the files.
    NextMap,
    /// Search the view shown.
    Search,
    /// The next pattern behind the window's panels, then none.
    NextBackdrop,
}

/// Every shortcut, as Ctrl-S lists them. Kept next to the bindings below so
/// that a change to one shows the other needing the same change.
pub const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Anywhere",
        &[
            (
                "Ctrl-N",
                "the project's universe, its plan, its services, its API, its database, then its files",
            ),
            (
                "Ctrl-F",
                "search the view shown: Enter keeps the search, Esc clears it",
            ),
            ("Ctrl-A", "the files, the tree chosen"),
            ("Ctrl-B", "show or hide the file tree"),
            ("Ctrl-M", "the window see-through, 80%, 60%, 40%, or opaque"),
            (
                "Ctrl-X",
                "the pattern behind the panels: stars, waves, grid, rain, then none (:background)",
            ),
            ("Cmd + Cmd − Cmd 0", "larger text, smaller, as at first"),
            (":", "a command: :help lists them"),
            ("Ctrl-S ?", "this list"),
            ("Ctrl-C", "twice to quit"),
            ("Tab", "next pane"),
        ],
    ),
    (
        "In the universe",
        &[
            ("drag, wheel", "turn, come closer"),
            ("click", "a star to centre it, a galaxy's name to enter it"),
            ("double-click", "a star to open its file"),
        ],
    ),
    (
        "File tree",
        &[
            (
                "M ? A D",
                "modified, new, added, deleted since the last commit",
            ),
            ("Up Down", "move"),
            ("Right or Enter", "open a file, unfold a folder"),
            ("Left", "fold the folder, or go to its parent"),
            ("q", "hide the tree"),
        ],
    ),
    (
        "Open file, read only (Vim)",
        &[
            ("gg G w b 0 ^ $", "move"),
            ("v V y", "select by character / by line, copy"),
            ("yy", "copy the line"),
            ("/ n N", "search, next, previous"),
            (":42", "go to line 42"),
            (
                "gd Ctrl-click",
                "go to where the name under the cursor is defined, as its language server says \
                 (pyright, typescript-language-server, rust-analyzer, gopls), else git grep; several: \
                 choose with ↑ ↓ Enter",
            ),
            ("gr", "list where the name under the cursor is used"),
            (
                "Ctrl-O Alt-← Alt-→",
                "back to where gd left from, and forward again",
            ),
            ("q :q", "close the file"),
        ],
    ),
];

/// The action bound to `key` in `mode` with `focus`.
pub fn action(mode: Mode, focus: Focus, key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Bindings that work everywhere.
    match key.code {
        KeyCode::Char('c') if ctrl => return Some(Action::Quit),
        // As the sidebar shortcut of common editors, so that the tree is one
        // key away even while typing.
        KeyCode::Char('b') if ctrl => return Some(Action::ToggleTree),
        KeyCode::Char('s') if ctrl => return Some(Action::ShowKeys),
        // A, top left on AZERTY: the files, from anywhere.
        KeyCode::Char('a') if ctrl => return Some(Action::FocusTree),
        KeyCode::Char('m') if ctrl => return Some(Action::ToggleSeeThrough),
        KeyCode::Char('n') if ctrl => return Some(Action::NextMap),
        // F for find, as in most applications.
        KeyCode::Char('f') if ctrl => return Some(Action::Search),
        // X, beside the other keys of the window's look on the bottom row.
        KeyCode::Char('x') if ctrl => return Some(Action::NextBackdrop),
        _ => {}
    }

    match mode {
        Mode::Normal => normal(focus, key),
        Mode::Command | Mode::Search => match key.code {
            KeyCode::Esc => Some(Action::Enter(Mode::Normal)),
            KeyCode::Enter => Some(Action::Submit),
            KeyCode::Backspace => Some(Action::Backspace),
            KeyCode::Char('w') if ctrl => Some(Action::DeleteWord),
            KeyCode::Char('u') if ctrl => Some(Action::ClearLine),
            KeyCode::Left => Some(Action::Left),
            KeyCode::Right => Some(Action::Right),
            KeyCode::Home => Some(Action::Home),
            KeyCode::End => Some(Action::End),
            KeyCode::Char(c) if !ctrl => Some(Action::Insert(c)),
            _ => None,
        },
    }
}

fn normal(focus: Focus, key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Tab => return Some(Action::FocusNext),
        KeyCode::Char(':') => return Some(Action::Enter(Mode::Command)),
        KeyCode::Char('?') => return Some(Action::ShowKeys),
        KeyCode::Up => return Some(Action::Move(-1)),
        KeyCode::Down => return Some(Action::Move(1)),
        KeyCode::PageUp => return Some(Action::HalfPage(false)),
        KeyCode::PageDown => return Some(Action::HalfPage(true)),
        KeyCode::Home => return Some(Action::Top),
        KeyCode::End => return Some(Action::Bottom),
        _ => {}
    }
    match focus {
        Focus::Tree => match key.code {
            KeyCode::Enter | KeyCode::Right => Some(Action::Open),
            KeyCode::Left => Some(Action::Collapse),
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::ClosePane),
            _ => None,
        },
        // Keys in the open file go to its reader, which follows Vim: see
        // `reader.rs`. Only Tab reaches this function from there.
        Focus::File => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn letters_type_in_the_command_line_and_the_search() {
        for mode in [Mode::Command, Mode::Search] {
            assert_eq!(
                action(mode, Focus::Tree, key(KeyCode::Char(','))),
                Some(Action::Insert(','))
            );
        }
        assert_eq!(
            action(Mode::Normal, Focus::Tree, key(KeyCode::Char(','))),
            None
        );
    }

    #[test]
    fn arrows_drive_the_tree() {
        let tree = |code| action(Mode::Normal, Focus::Tree, key(code));
        assert_eq!(tree(KeyCode::Down), Some(Action::Move(1)));
        assert_eq!(tree(KeyCode::Right), Some(Action::Open));
        assert_eq!(tree(KeyCode::Left), Some(Action::Collapse));
        assert_eq!(tree(KeyCode::Char('j')), None);
    }

    #[test]
    fn the_window_wide_keys_work_in_every_mode() {
        for mode in [Mode::Normal, Mode::Command, Mode::Search] {
            for focus in [Focus::Tree, Focus::File] {
                let pressed = |c| action(mode, focus, ctrl(c));
                assert_eq!(pressed('c'), Some(Action::Quit));
                assert_eq!(pressed('b'), Some(Action::ToggleTree));
                assert_eq!(pressed('n'), Some(Action::NextMap));
                assert_eq!(pressed('f'), Some(Action::Search));
            }
        }
    }
}
