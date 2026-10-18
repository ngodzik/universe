//! Files that hold secrets, and what of them a model may see: the names a
//! file of variables sets, and the values that are no secret by their name
//! or their look. Never whether a value matches a guess, so that no
//! question a model chooses can tell a secret.

use std::fs;
use std::path::Path;

use ignore::WalkBuilder;

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

/// What a model may know of a variable's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    /// It is empty.
    Empty,
    /// It holds no secret, as far as its name and its look tell: shown as
    /// written.
    Plain(String),
    /// Its name says it is a secret, or it looks like one: a password in a
    /// URL, a long run of letters and digits, a value over several lines.
    Secret,
}

/// A value as a file of variables writes it.
enum Written {
    /// On the variable's line, its quotes kept.
    Line(String),
    /// Over several lines, between the quotes that open and close it.
    Lines(Vec<String>),
}

/// The variables `text` sets, in its order, each with its value as written;
/// one set again keeps its place and takes its last value. Comments, and
/// lines that set nothing, are left out, and so are the lines of a quoted
/// value that runs over several, such as a key: one of them may look like
/// `NAME=`.
fn parse(text: &str) -> Vec<(String, Written)> {
    let mut variables: Vec<(String, Written)> = Vec::new();
    let mut set =
        |name: String, written: Written| match variables.iter_mut().find(|(n, _)| *n == name) {
            Some(seen) => seen.1 = written,
            None => variables.push((name, written)),
        };
    // A value over several lines not closed yet: its quote, its variable's
    // name if it is one, its lines so far.
    let mut pending: Option<(char, Option<String>, Vec<String>)> = None;
    for line in text.lines() {
        if let Some((quote, name, mut lines)) = pending.take() {
            match closes(line, quote) {
                Some(end) => {
                    lines.push(line[..end].to_owned());
                    if let Some(name) = name {
                        set(name, Written::Lines(lines));
                    }
                }
                None => {
                    lines.push(line.to_owned());
                    pending = Some((quote, name, lines));
                }
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
        let quote = value
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\'' | '`'));
        let written = match quote {
            Some(quote) => match closes(&value[1..], quote) {
                Some(end) => value[..end + 2].to_owned(),
                None => {
                    let name = is_name(name).then(|| name.to_owned());
                    pending = Some((quote, name, vec![value[1..].to_owned()]));
                    continue;
                }
            },
            // What follows ` #` is a comment.
            None => value
                .split(" #")
                .next()
                .unwrap_or(value)
                .trim_end()
                .to_owned(),
        };
        if is_name(name) {
            set(name.to_owned(), Written::Line(written));
        }
    }
    // A value never closed is still its variable's, and a secret.
    if let Some((_, Some(name), lines)) = pending {
        set(name, Written::Lines(lines));
    }
    variables
}

/// Whether the value `written` of the variable `name` holds a secret.
fn holds_secret(name: &str, written: &Written) -> bool {
    match written {
        Written::Lines(_) => true,
        Written::Line(value) => secret_name(name) || looks_secret(unquoted(value)),
    }
}

/// The variables `text`, a file of variables, sets, in its order: each name
/// and what may be known of its value.
pub(crate) fn variables(text: &str) -> Vec<(String, Value)> {
    parse(text)
        .into_iter()
        .map(|(name, written)| {
            let value = match &written {
                Written::Line(value) if matches!(value.as_str(), "" | "\"\"" | "''" | "``") => {
                    Value::Empty
                }
                Written::Line(value) if !holds_secret(&name, &written) => {
                    Value::Plain(value.clone())
                }
                _ => Value::Secret,
            };
            (name, value)
        })
        .collect()
}

/// The secrets `text`, a file of variables, holds, each with its
/// variable's name: a value written on its line without its quotes, and
/// each line of one over several. For taking them out of what commands
/// print, never for a model.
fn secret_values(text: &str) -> Vec<(String, String)> {
    parse(text)
        .into_iter()
        .filter(|(name, written)| holds_secret(name, written))
        .flat_map(|(name, written)| {
            let values = match written {
                Written::Line(value) => vec![unquoted(&value).to_owned()],
                Written::Lines(lines) => lines.iter().map(|l| l.trim().to_owned()).collect(),
            };
            values
                .into_iter()
                .filter(|value| !value.is_empty())
                .map(move |value| (name.clone(), value))
        })
        .collect()
}

/// Folders no file of variables is looked for in: what tools download or
/// build.
const NOT_SEARCHED: [&str; 12] = [
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".next",
    "vendor",
    ".terraform",
    ".cache",
];

/// How deep under the project files of variables are looked for.
const DEPTH: usize = 4;

/// The largest file of variables read.
const LARGEST: u64 = 256 * 1024;

/// The secrets the project's files of variables hold, git ignored ones
/// first of all, each with its variable's name.
pub(crate) fn project_secrets(root: &Path) -> Vec<(String, String)> {
    WalkBuilder::new(root)
        .standard_filters(false)
        .max_depth(Some(DEPTH))
        .filter_entry(|entry| {
            !(entry.file_type().is_some_and(|t| t.is_dir())
                && NOT_SEARCHED.iter().any(|name| entry.file_name() == *name))
        })
        .build()
        .filter_map(Result::ok)
        .filter(|entry| variables_file(&entry.file_name().to_string_lossy()))
        // Through a link too: a command reads through it.
        .filter(|entry| fs::metadata(entry.path()).is_ok_and(|m| m.is_file() && m.len() <= LARGEST))
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .flat_map(|text| secret_values(&text))
        .collect()
}

/// `text` with the values of `secrets` taken out, the longest first so
/// that a part of one is not left over: one of 8 characters or more
/// wherever it stands, a shorter one, too common to be told from ordinary
/// words, where it follows its name.
pub(crate) fn redact_values(text: &str, secrets: &[(String, String)]) -> String {
    let mut secrets: Vec<&(String, String)> = secrets.iter().collect();
    secrets.sort_by_key(|(_, value)| std::cmp::Reverse(value.len()));
    let mut out = text.to_owned();
    for (name, value) in secrets {
        let hidden = format!("<redacted:{name}>");
        if value.len() >= 8 {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &hidden);
            }
            continue;
        }
        for between in ["=", " = ", "= ", " ="] {
            for quote in ["", "\"", "'"] {
                out = replace_whole(
                    &out,
                    &format!("{name}{between}{quote}{value}{quote}"),
                    &format!("{name}{between}{quote}{hidden}{quote}"),
                );
            }
        }
    }
    out
}

/// `text` with `from` replaced by `to` where it is not the start of a
/// longer value: `PASS=abc` is not in `PASS=abcd`.
fn replace_whole(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(from) {
        let after = &rest[at + from.len()..];
        let whole = after
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || "_-.+/=@:".contains(c)));
        out.push_str(&rest[..at]);
        out.push_str(if whole { to } else { from });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Whether a variable's name says it holds a secret: as an environment's
/// does, or a word of it is a password, a private key, a salt or a DSN,
/// which carries a key.
fn secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    crate::command::is_secret(name)
        || (!matches!(upper.as_str(), "PWD" | "OLDPWD")
            && upper
                .split(['_', '-', '.'])
                .any(|word| matches!(word, "PASS" | "PWD" | "PRIVATE" | "SALT" | "DSN")))
}

/// `written` without the quotes around it.
fn unquoted(written: &str) -> &str {
    let mut chars = written.chars();
    match (chars.next(), chars.next_back()) {
        (Some(a), Some(b)) if a == b && matches!(a, '"' | '\'' | '`') => {
            &written[1..written.len() - 1]
        }
        _ => written,
    }
}

/// Whether `value` looks like a secret whatever its name: a URL with a
/// password, or a piece of it that is a long run of letters and digits, as
/// keys and tokens are.
fn looks_secret(value: &str) -> bool {
    let password_in_url = value.split_once("://").is_some_and(|(_, rest)| {
        let authority = rest.split('/').next().unwrap_or(rest);
        authority
            .rsplit_once('@')
            .is_some_and(|(user, _)| user.contains(':'))
    });
    password_in_url
        || value
            .split([' ', '/', '?', '&', '=', ':', '@', ',', ';'])
            .any(|piece| {
                let mixed = piece.chars().any(|c| c.is_ascii_digit())
                    && piece.chars().any(|c| c.is_ascii_alphabetic());
                (piece.len() >= 16 && mixed) || piece.len() >= 32
            })
}

/// Where `rest` closes a value opened by `quote`, if it does: a double
/// quote escaped by a backslash does not.
fn closes(rest: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (at, c) in rest.char_indices() {
        if c == quote && !escaped {
            return Some(at);
        }
        escaped = quote == '"' && c == '\\' && !escaped;
    }
    None
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
    fn a_value_is_known_only_when_neither_its_name_nor_its_look_says_secret() {
        let text = "\
# the database
DATABASE_URL=postgres://user:hunter22@db/app
export API_KEY = 'sk-live-abc'
PORT=8080 # the default
HOST=\"localhost\"
EMPTY=
QUOTED_EMPTY=\"\"
not a variable line
PRIVATE_KEY=\"-----BEGIN KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASC=
SECRET_LOOKING=line
-----END KEY-----\"
ESCAPED=\"a \\\" b\"
STRIPE=sk9live0abcdef123456
SENTRY_DSN=https://abc@o1.ingest.example.io/1
DB_PASS=hunter2
AFTER=1
API_KEY=
";
        let plain = |v: &str| Value::Plain(v.to_owned());
        let variables = variables(text);
        assert_eq!(
            variables,
            [
                ("DATABASE_URL".to_owned(), Value::Secret),
                ("API_KEY".to_owned(), Value::Empty),
                ("PORT".to_owned(), plain("8080")),
                ("HOST".to_owned(), plain("\"localhost\"")),
                ("EMPTY".to_owned(), Value::Empty),
                ("QUOTED_EMPTY".to_owned(), Value::Empty),
                ("PRIVATE_KEY".to_owned(), Value::Secret),
                ("ESCAPED".to_owned(), plain("\"a \\\" b\"")),
                ("STRIPE".to_owned(), Value::Secret),
                ("SENTRY_DSN".to_owned(), Value::Secret),
                ("DB_PASS".to_owned(), Value::Secret),
                ("AFTER".to_owned(), plain("1")),
            ]
        );
        let all = format!("{variables:?}");
        for value in [
            "hunter2",
            "sk-live",
            "sk9live",
            "MIIE",
            "SECRET_LOOKING",
            "abc@",
        ] {
            assert!(!all.contains(value), "{value} in {all}");
        }
    }

    #[test]
    fn a_short_secret_is_taken_out_only_after_its_name() {
        let secrets = secret_values("PASS=abc\nTOKEN='t0k3n-0123456789'\nPORT=80\n");
        assert_eq!(
            secrets,
            [
                ("PASS".to_owned(), "abc".to_owned()),
                ("TOKEN".to_owned(), "t0k3n-0123456789".to_owned()),
            ]
        );
        assert_eq!(
            redact_values(
                "PASS=abc PASS=abcd PASS = 'abc' abc PORT=80 x t0k3n-0123456789",
                &secrets
            ),
            "PASS=<redacted:PASS> PASS=abcd PASS = '<redacted:PASS>' abc PORT=80 x \
             <redacted:TOKEN>"
        );
    }

    #[test]
    fn keys_and_tokens_look_like_secrets_and_settings_do_not() {
        for setting in [
            "http://localhost:3000/api",
            "us-east-1",
            "/var/lib/app",
            "2026-01-01T00:00:00Z",
            "development",
            "1.24.3",
        ] {
            assert!(!looks_secret(setting), "{setting}");
        }
        for secret in [
            "123e4567-e89b-12d3-a456-426614174000",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abc",
            "https://api.example.com/v1?key=AbCdEf0123456789xyz",
            "redis://:hunter22@cache:6379",
        ] {
            assert!(looks_secret(secret), "{secret}");
        }
        assert!(secret_name("DB_PASS") && secret_name("SENTRY_DSN"));
        assert!(!secret_name("PWD") && !secret_name("PORT"));
    }
}
