//! The `:` commands.

/// A parsed `:` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// `:q`, `:quit`
    Quit,
    /// `:help`
    Help,
    /// `:keys` lists every shortcut, as Ctrl-S does.
    Keys,
    /// `:open <path>` opens a file, as the tree names it.
    Open(Option<String>),
    /// `:background [pattern|off] [still|slow]`: what is drawn behind the
    /// window's panels; alone, what is set.
    Background(Option<String>),
}

/// Parses the text typed after `:`. Errors are messages for the person.
pub(crate) fn parse(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let (name, rest) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(n, r)| (n, r.trim()));
    let rest = (!rest.is_empty()).then(|| rest.to_owned());
    match name {
        "q" | "quit" | "q!" => Ok(Command::Quit),
        "help" | "h" => Ok(Command::Help),
        "keys" => Ok(Command::Keys),
        "open" | "e" => Ok(Command::Open(rest)),
        "background" | "bg" => Ok(Command::Background(rest)),
        "" => Err("Type a command after the colon: :help lists them".to_owned()),
        other => Err(format!("Unknown command :{other}. :help lists them")),
    }
}

/// The names the command line completes.
pub(crate) const NAMES: &[&str] = &["background", "help", "keys", "open", "quit"];

/// Where Tab completion stands while it cycles through several candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Completion {
    /// Every candidate for what was typed before the first Tab.
    pub(crate) matches: Vec<String>,
    next: usize,
}

/// Completes `line` among `candidates(line)`: the only candidate at once,
/// else the part all candidates share, then each candidate in turn on the
/// following Tabs. Returns the new line, or `None` when nothing matches.
pub(crate) fn complete(
    line: &str,
    state: &mut Option<Completion>,
    candidates: impl Fn(&str) -> Vec<String>,
) -> Option<String> {
    if let Some(current) = state.as_mut()
        && current.matches.iter().any(|m| m == line)
    {
        let pick = current.matches[current.next % current.matches.len()].clone();
        current.next += 1;
        return Some(pick);
    }
    let mut matches = candidates(line);
    matches.sort();
    matches.dedup();
    match matches.len() {
        0 => {
            *state = None;
            None
        }
        1 => {
            *state = None;
            matches.pop()
        }
        _ => {
            let shared = matches
                .iter()
                .skip(1)
                .fold(matches[0].clone(), |common, m| {
                    common
                        .chars()
                        .zip(m.chars())
                        .take_while(|(a, b)| a == b)
                        .map(|(a, _)| a)
                        .collect()
                });
            let first = if shared.len() > line.len() {
                shared
            } else {
                matches[0].clone()
            };
            let next = usize::from(first == matches[0]);
            *state = Some(Completion { matches, next });
            Some(first)
        }
    }
}

/// Candidates for a command line: command names while the first word is
/// being typed, then the words `background` takes.
pub(crate) fn candidates(line: &str, names: &[&str]) -> Vec<String> {
    match line.split_once(' ') {
        None => names
            .iter()
            .filter(|n| n.starts_with(line))
            .map(|n| (*n).to_owned())
            .collect(),
        Some(("background" | "bg", arg)) => {
            let command = &line[..line.len() - arg.len()];
            ["stars", "waves", "grid", "rain", "off", "still", "slow"]
                .iter()
                .filter(|w| w.starts_with(arg))
                .map(|w| format!("{command}{w}"))
                .collect()
        }
        Some(_) => Vec::new(),
    }
}

/// What `:help` says: the views, the commands and the file's keys.
pub const HELP: &str = "\
Ctrl-N goes through the views: the universe of files, the plan, the
services, the API and the database; the deployments are a click away in the
universe. Ctrl-F searches the view shown. Ctrl-A shows the files.
A system of several repositories, read as one, is declared in
~/.universe/systems/<name>.toml (repos = [...], linked = [...]), or with
universe <folders> --save <name>; universe in any of them opens it whole.
:open <path>         open a file, as the tree names it (:e)
:background [p] [m]  a quiet pattern behind the panels (:bg): stars, waves, grid,
                     rain or off; still (costs nothing) or slow. Ctrl-X: the next
                     one. Alone, what is set
:keys                every shortcut (Ctrl-S)
:q                   quit
Files: arrows move, → or Enter opens, ← closes a folder. The mouse works too.
Open file: read only, with Vim's keys. v V select, y copies, yy copies a line,
  /text searches then n N, :42 goes to a line, gg G, w b, 0 $, gd goes to where
  a name is defined (Ctrl-click too), gr lists where it is used, Ctrl-O comes
  back, q closes.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse_with_their_words() {
        assert_eq!(parse("q"), Ok(Command::Quit));
        assert_eq!(
            parse(" open src/main.rs "),
            Ok(Command::Open(Some("src/main.rs".into())))
        );
        assert_eq!(
            parse("bg rain slow"),
            Ok(Command::Background(Some("rain slow".into())))
        );
        assert_eq!(parse("background"), Ok(Command::Background(None)));
        assert!(parse("model x").unwrap_err().contains(":model"));
    }

    #[test]
    fn tab_completes_names_then_cycles_through_them() {
        let names = |l: &str| candidates(l, NAMES);
        let mut state = None;
        assert_eq!(complete("op", &mut state, names), Some("open".into()));
        assert_eq!(complete("bg r", &mut state, names), Some("bg rain".into()));
        assert_eq!(complete("zz", &mut state, names), None);
        let mut state = None;
        assert_eq!(
            complete("background s", &mut state, names),
            Some("background slow".into())
        );
        assert_eq!(
            complete("background slow", &mut state, names),
            Some("background stars".into())
        );
    }
}
