//! Files that hold secrets, and the one thing of them a model may see: the
//! names a file of variables sets. Not their values, nor whether a value
//! matches a guess, so that no question a model chooses can tell them.

/// Files that hold secrets: reading them shows them.
const SECRET_FILES: [&str; 16] = [
    ".aws/credentials",
    ".aws/sso/cache",
    ".aws/cli/cache",
    ".ssh/",
    ".kube/config",
    ".docker/config.json",
    ".netrc",
    ".git-credentials",
    ".config/gh/hosts.yml",
    ".pgpass",
    ".npmrc",
    ".pypirc",
    ".claude/.credentials.json",
    ".codex/auth.json",
    ".config/gcloud/",
    ".azure/",
];

/// Whether `word` names a file of secrets: one above, or a file of
/// variables.
pub(crate) fn secret_file(word: &str) -> bool {
    SECRET_FILES.iter().any(|f| word.contains(f)) || variables_file(word)
}

/// Whether `word` names a file of variables: `.env`, `.env.local`,
/// `prod.env`.
pub(crate) fn variables_file(word: &str) -> bool {
    let name = word.rsplit('/').next().unwrap_or(word);
    name == ".env" || name.starts_with(".env.") || name.ends_with(".env")
}

/// The longest name read as one: past it, a line is more likely a piece of
/// a value than a variable.
const LONGEST_NAME: usize = 80;

/// The variables `text`, a file of variables, sets, in its order: each name,
/// and whether its value is empty. Comments, and lines that set nothing, are
/// left out, and so are the lines of a quoted value that runs over several,
/// such as a key: one of them may look like `NAME=`.
pub(crate) fn names_set(text: &str) -> Vec<(String, bool)> {
    let mut names: Vec<(String, bool)> = Vec::new();
    // The quote a value opened and has not closed yet.
    let mut open: Option<char> = None;
    for line in text.lines() {
        if let Some(quote) = open {
            if closes(line, quote) {
                open = None;
            }
            continue;
        }
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if let Some(quote) = value
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\'' | '`'))
            && !closes(&value[1..], quote)
        {
            open = Some(quote);
        }
        if !is_name(name) {
            continue;
        }
        let empty = matches!(value, "" | "\"\"" | "''" | "``");
        match names.iter_mut().find(|(n, _)| n == name) {
            // Set again: the last value is the one read.
            Some(seen) => seen.1 = empty,
            None => names.push((name.to_owned(), empty)),
        }
    }
    names
}

/// Whether `rest` closes a value opened by `quote`: a double quote escaped
/// by a backslash does not.
fn closes(rest: &str, quote: char) -> bool {
    let mut escaped = false;
    for c in rest.chars() {
        if c == quote && !escaped {
            return true;
        }
        escaped = quote == '"' && c == '\\' && !escaped;
    }
    false
}

/// Whether `name` can be a variable's.
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= LONGEST_NAME
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_of_secrets_and_of_variables_are_told_by_their_names() {
        for secret in [
            ".env",
            "api/.env.local",
            "deploy/prod.env",
            "/home/me/.aws/credentials",
            ".ssh/id_ed25519",
        ] {
            assert!(secret_file(secret), "{secret}");
        }
        assert!(variables_file("api/.env.production"));
        assert!(!variables_file("/home/me/.aws/credentials"));
        for plain in ["env.py", "src/environment.ts", ".envrc.md", "README.md"] {
            assert!(!secret_file(plain), "{plain}");
        }
    }

    #[test]
    fn only_the_names_are_read_never_a_value() {
        let text = "\
# the database
DATABASE_URL=postgres://user:hunter22@db/app
export API_KEY = 'sk-live-abc'
EMPTY=
QUOTED_EMPTY=\"\"
not a variable line
PRIVATE_KEY=\"-----BEGIN KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASC=
SECRET_LOOKING=line
-----END KEY-----\"
ESCAPED=\"a \\\" quote\"
AFTER=1
API_KEY=
";
        let names = names_set(text);
        assert_eq!(
            names,
            [
                ("DATABASE_URL".to_owned(), false),
                ("API_KEY".to_owned(), true),
                ("EMPTY".to_owned(), true),
                ("QUOTED_EMPTY".to_owned(), true),
                ("PRIVATE_KEY".to_owned(), false),
                ("ESCAPED".to_owned(), false),
                ("AFTER".to_owned(), false),
            ]
        );
        let all = format!("{names:?}");
        for value in ["hunter22", "sk-live", "MIIE", "SECRET_LOOKING", "quote"] {
            assert!(!all.contains(value), "{value} in {all}");
        }
    }
}
