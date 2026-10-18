//! What a front end calls of a back end through an OpenAPI spec: each
//! operation of the spec, the back end's function that serves it, and the
//! front end's files that call it, by the names a generated client gives it.
//!
//! All by reading the files, no model: the spec is read as YAML (JSON being
//! YAML too), the latest of the versions of one API, and an operation is
//! named by its `operationId`; the function that serves it is the one of
//! that name under a route decorator (`@router.get(...)`), else the route
//! of its method and path, the nearest to the spec when several are; a
//! call is an identifier that ends with the operation's name
//! in the forms clients are generated with (`getDag`, `useDagServiceGetDag`,
//! `UseDagServiceGetDagKeyFn`). Files of the generated client itself are
//! not callers: they name every operation.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use yaml_rust2::{Yaml, YamlLoader};

use crate::map::{CodeMap, Language, NodeKind, worth};

static ROUTE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*@[\w.]+\.(?:get|post|put|patch|delete|head|options|route|api_route|websocket)\(",
    )
    .ok()
});
static OPERATION_ID_ARG: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"\boperation_id\s*=\s*["']([^"']+)["']"#).ok());
static DEF: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:async\s+)?def\s+(\w+)\s*\(").ok());
static IDENTIFIER: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"[A-Za-z_$][\w$]*").ok());

/// What generated clients add after an operation's name, in the names of
/// what they generate around it: React Query's keys and loaders.
const CLIENT_SUFFIXES: [&str; 5] = ["KeyFn", "Key", "Data", "Fn", "Query"];

/// An operation of an API's spec, with the function that serves it and the
/// files that call it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// Its `operationId`, or its method and path when it has none.
    pub id: String,
    /// `GET`, `POST`…
    pub method: String,
    /// Its path, parameters in braces: `/api/v2/dags/{dag_id}`.
    pub path: String,
    /// What it does, in a line, as the spec says.
    pub summary: String,
    /// What it does, at length, as the spec says.
    pub description: String,
    /// What it is asked with: in its path, its query, its headers.
    pub parameters: Vec<Parameter>,
    /// What it is sent, if anything.
    pub body: Option<Body>,
    /// What it may answer, by status, in the spec's order.
    pub responses: Vec<Response>,
    /// Whether it asks for credentials.
    pub secured: bool,
    /// Where the API is, as the spec says, if it does.
    pub server: Option<String>,
    /// The spec's tags for it: the resource it is about, mostly.
    pub tags: Vec<String>,
    /// Whether the spec says it is deprecated.
    pub deprecated: bool,
    /// The spec it is from, as a node of the map; for a route read from
    /// the code, the file that declares it.
    pub spec: usize,
    /// Whether it was read from the code's route decorators, there being no
    /// spec.
    pub from_code: bool,
    /// The file and the line, from 1, of the function that serves it.
    pub handler: Option<(usize, usize)>,
    /// The front end's files that call it.
    pub callers: Vec<usize>,
}

/// Something an operation is asked with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    /// Its name.
    pub name: String,
    /// Where it goes: `path`, `query`, `header` or `cookie`.
    pub location: String,
    /// Whether it must be given.
    pub required: bool,
    /// Its type, as read: `string`, `integer`, `string[]`, `DagRunState`.
    pub kind: String,
    /// What it is, as the spec says.
    pub description: String,
    /// A value it may take, as the spec gives one, if it does.
    pub example: Option<String>,
}

/// What an operation is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// Its media type: `application/json`.
    pub media: String,
    /// The name of its schema, when it is one of the spec's.
    pub schema: Option<String>,
    /// Whether it must be sent.
    pub required: bool,
    /// An example of it: the spec's, or made from its schema.
    pub example: Option<String>,
}

/// An answer an operation may give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// Its status: `200`, `404`, `default`.
    pub status: String,
    /// What it means, as the spec says.
    pub description: String,
    /// The name of its schema, when it is one of the spec's.
    pub schema: Option<String>,
    /// An example of it: the spec's, or made from its schema.
    pub example: Option<String>,
}

/// How deep an example is made from nested schemas before it stops: deep
/// enough to show the shape, short enough to read.
const EXAMPLE_DEPTH: usize = 4;

/// The methods an OpenAPI path item may hold.
const METHODS: [&str; 8] = [
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

/// The operations of every OpenAPI spec of the project, each with its
/// handler and callers, and the calls through them: for each file that
/// calls an operation, the file that serves it, each pair once.
pub(crate) fn read(
    map: &CodeMap,
    texts: &[(usize, String)],
) -> (Vec<Operation>, Vec<(usize, usize)>) {
    let (Some(route), Some(def), Some(identifier)) =
        (ROUTE.as_ref(), DEF.as_ref(), IDENTIFIER.as_ref())
    else {
        return (Vec::new(), Vec::new());
    };
    let language = |node: usize| match map.nodes[node].kind {
        NodeKind::File { language, .. } => Some(language),
        NodeKind::Folder => None,
    };

    let specs: Vec<(usize, Yaml)> = texts
        .iter()
        .filter(|(node, text)| language(*node) == Some(Language::Config) && is_spec(text))
        .filter_map(|(node, text)| Some((*node, load(text)?)))
        .collect();
    let mut operations: Vec<Operation> = latest(map, &specs)
        .into_iter()
        .flat_map(|(node, doc)| operations_of(node, doc))
        .collect();
    // Without a spec, the routes the code declares, as FastAPI and Flask
    // write them.
    if operations.is_empty() {
        operations = routes_in_code(map, texts);
        if operations.is_empty() {
            return (Vec::new(), Vec::new());
        }
    }
    let wanted: HashSet<&str> = operations
        .iter()
        .filter(|o| !o.from_code)
        .map(|o| o.id.as_str())
        .collect();

    // The functions under a route decorator, by name, with their line.
    let mut handlers: HashMap<String, Vec<(usize, usize, bool)>> = HashMap::new();
    for (node, text) in texts {
        if language(*node) != Some(Language::Python) || !designed(map, *node) {
            continue;
        }
        for decorator in route.find_iter(text) {
            let after = &text[decorator.end()..];
            let Some(name) = def.captures(after).and_then(|c| c.get(1)) else {
                continue;
            };
            // An id given to the decorator (`operation_id="get_dags_ui"`)
            // rather than taken from the function's name.
            let given = OPERATION_ID_ARG
                .as_ref()
                .and_then(|re| re.captures(&after[..name.start()]))
                .and_then(|c| c.get(1))
                .map(|m| m.as_str());
            let id = given.unwrap_or(name.as_str());
            if !wanted.contains(id) {
                continue;
            }
            let at = decorator.end() + name.start();
            let line = text[..at].matches('\n').count() + 1;
            handlers
                .entry(id.to_owned())
                .or_default()
                .push((*node, line, given.is_some()));
        }
    }

    // Each operation's handler: the one given its id, else the nearest to
    // its spec, when several are.
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, operation) in operations.iter_mut().enumerate() {
        let spec_path = &map.nodes[operation.spec].path;
        if operation.from_code {
            continue;
        }
        operation.handler = handlers.get(&operation.id).and_then(|candidates| {
            candidates
                .iter()
                .copied()
                .max_by_key(|&(c, _, given)| {
                    (
                        given,
                        shared_prefix(spec_path, &map.nodes[c].path),
                        usize::MAX - c,
                    )
                })
                .map(|(c, line, _)| (c, line))
        });
        let pascal = pascal(&operation.id);
        let camel = lower_first(&pascal);
        for name in [pascal, camel] {
            let list = by_name.entry(name).or_default();
            if !list.contains(&index) {
                list.push(index);
            }
        }
    }

    // An id the code does not write, as when FastAPI makes them with a
    // `generate_unique_id_function` (`organizations:get`): the operation is
    // served by the route of its method and path, the nearest to the spec
    // when several are.
    if operations
        .iter()
        .any(|o| !o.from_code && o.handler.is_none())
    {
        let mut routes: HashMap<(String, String), Vec<(usize, usize)>> = HashMap::new();
        for route in routes_in_code(map, texts) {
            if let Some(handler) = route.handler {
                routes
                    .entry((route.method, route_key(&route.path)))
                    .or_default()
                    .push(handler);
            }
        }
        for operation in operations
            .iter_mut()
            .filter(|o| !o.from_code && o.handler.is_none())
        {
            let spec_path = &map.nodes[operation.spec].path;
            operation.handler = routes
                .get(&(operation.method.clone(), route_key(&operation.path)))
                .and_then(|candidates| {
                    candidates.iter().copied().max_by_key(|&(c, _)| {
                        (shared_prefix(spec_path, &map.nodes[c].path), usize::MAX - c)
                    })
                });
        }
    }

    // The front end's files that call them.
    for (node, text) in texts {
        let is_script = matches!(
            language(*node),
            Some(Language::TypeScript | Language::JavaScript)
        );
        if !is_script || !designed(map, *node) || generated(&map.nodes[*node].path, text) {
            continue;
        }
        let mut called_here = HashSet::new();
        for word in identifier.find_iter(text) {
            let after_service = text[..word.start()].ends_with("Service.");
            let word = word.as_str();
            for name in called(word) {
                // A name of one word (`state`, `login`) is everywhere
                // (`useState`): it counts only in the forms a client is
                // generated with, `useEdgeServiceState`, `EdgeService.state`.
                if one_word(name) {
                    let before = word.rfind(name).map_or(word, |at| &word[..at]);
                    let generated_form =
                        before.ends_with("Service") || (before.is_empty() && after_service);
                    if !generated_form {
                        continue;
                    }
                }
                called_here.extend(by_name.get(name).into_iter().flatten().copied());
            }
        }
        for index in called_here {
            operations[index].callers.push(*node);
        }
    }

    // And those whose paths they write: `'$baseUrl/users/me'`.
    path_callers(map, texts, &mut operations);

    let mut calls = HashSet::new();
    for operation in &mut operations {
        operation.callers.sort_unstable();
        operation.callers.dedup();
        if let Some((handler, _)) = operation.handler {
            calls.extend(operation.callers.iter().map(|&c| (c, handler)));
        }
    }
    let mut calls: Vec<(usize, usize)> = calls.into_iter().collect();
    calls.sort_unstable();
    (operations, calls)
}

/// A router: the file that declares it, and its name there.
type RouterKey = (usize, String);

/// Where each router is included: the router that includes it, by its file
/// and name, and the prefix it is given there.
type Includers = HashMap<RouterKey, Vec<(usize, String, String)>>;

/// The routes Python code declares with decorators, there being no spec:
/// `@router.get("/me")` under `router = APIRouter(prefix="/users")`, that
/// router included with `app.include_router(users.router, prefix="/api")`.
fn routes_in_code(map: &CodeMap, texts: &[(usize, String)]) -> Vec<Operation> {
    static ROUTER: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^\s*(\w+)\s*(?::[^=\n]+)?=\s*(?:\w+\.)*(APIRouter|FastAPI|Blueprint|Flask)\(([^)]*)\)",
        )
        .ok()
    });
    static PREFIX: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r#"\b(?:prefix|url_prefix|root_path)\s*=\s*["']([^"']*)["']"#).ok()
    });
    static TAG: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r#"\btags\s*=\s*\[\s*["']([^"']+)["']"#).ok());
    static INCLUDE: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r"(\w+)\.(?:include_router|register_blueprint)\(\s*([\w.]+)\s*(?:,([^)]*))?\)")
            .ok()
    });
    static IMPORT: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?m)^\s*from\s+([\w.]+)\s+import\s+([^\n#]+)").ok());
    static DECORATOR: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(
            r#"(?m)^\s*@(\w+)\.(get|post|put|patch|delete|head|options|websocket|api_route|route)\(\s*["']([^"']*)["']([^)]*)\)"#,
        )
        .ok()
    });
    static METHODS_ARG: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"\bmethods\s*=\s*\[([^\]]*)\]").ok());
    let (Some(router), Some(include), Some(import), Some(decorator), Some(def)) = (
        ROUTER.as_ref(),
        INCLUDE.as_ref(),
        IMPORT.as_ref(),
        DECORATOR.as_ref(),
        DEF.as_ref(),
    ) else {
        return Vec::new();
    };
    let python: Vec<&(usize, String)> = texts
        .iter()
        .filter(|(n, _)| {
            matches!(
                map.nodes[*n].kind,
                NodeKind::File {
                    language: Language::Python,
                    ..
                }
            ) && designed(map, *n)
        })
        .collect();
    let capture = |re: &LazyLock<Option<Regex>>, text: &str| {
        re.as_ref()
            .and_then(|r| r.captures(text))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
    };
    // Each router: its file and name, with its own prefix and its tag.
    let mut routers: HashMap<RouterKey, (String, Option<String>)> = HashMap::new();
    for (node, text) in &python {
        for c in router.captures_iter(text) {
            let args = &c[3];
            routers.insert(
                (*node, c[1].to_owned()),
                (
                    capture(&PREFIX, args).unwrap_or_default(),
                    capture(&TAG, args),
                ),
            );
        }
    }
    // Where each router is included, and under which prefix. A module
    // imported from `importer`: `polar.users` wherever the package's root
    // is, `.users` beside it, `..users` a folder up.
    let module_file = |importer: usize, module: &str| -> Option<usize> {
        let dots = module.len() - module.trim_start_matches('.').len();
        let rest = module[dots..].replace('.', "/");
        let path = if dots == 0 {
            rest
        } else {
            let mut base = crate::imports::parent(&map.nodes[importer].path);
            for _ in 1..dots {
                base = crate::imports::parent(base);
            }
            crate::imports::join(base, &rest)
        };
        let candidates = [format!("{path}.py"), format!("{path}/__init__.py")];
        python.iter().map(|(n, _)| *n).find(|n| {
            let p = &map.nodes[*n].path;
            candidates
                .iter()
                .any(|c| p == c || (dots == 0 && p.ends_with(&format!("/{c}"))))
        })
    };
    let mut included: Includers = HashMap::new();
    for (node, text) in &python {
        // What the file imports, by the name it uses.
        let mut imported: HashMap<String, (String, String)> = HashMap::new();
        for c in import.captures_iter(text) {
            let module = c[1].to_owned();
            for name in c[2].trim_matches(['(', ')', ' ']).split(',') {
                let mut words = name.split_whitespace();
                let Some(original) = words.next() else {
                    continue;
                };
                let alias = match (words.next(), words.next()) {
                    (Some("as"), Some(alias)) => alias,
                    _ => original,
                };
                imported.insert(alias.to_owned(), (module.clone(), original.to_owned()));
            }
        }
        for c in include.captures_iter(text) {
            let target = &c[2];
            let prefix = c
                .get(3)
                .and_then(|a| capture(&PREFIX, a.as_str()))
                .unwrap_or_default();
            let resolved = match target.rsplit_once('.') {
                // `users.router`: the module `users`, as imported.
                Some((module, var)) => imported
                    .get(module)
                    .map(|(from, name)| {
                        if from.ends_with('.') {
                            format!("{from}{name}")
                        } else {
                            format!("{from}.{name}")
                        }
                    })
                    .or_else(|| Some(module.to_owned()))
                    .and_then(|m| module_file(*node, &m))
                    .map(|n| (n, var.to_owned())),
                // `ws_router`: imported under that name, or the file's own.
                None => match imported.get(target) {
                    Some((from, name)) => module_file(*node, from).map(|n| (n, name.clone())),
                    None => Some((*node, target.to_owned())),
                },
            };
            // A router may be included in several, and serves under each.
            if let Some(key) = resolved {
                let includers = included.entry(key).or_default();
                let includer = (*node, c[1].to_owned(), prefix);
                if !includers.contains(&includer) {
                    includers.push(includer);
                }
            }
        }
    }
    let mut operations = Vec::new();
    for (node, text) in &python {
        for c in decorator.captures_iter(text) {
            let key = (*node, c[1].to_owned());
            if !routers.contains_key(&key) && !included.contains_key(&key) {
                continue;
            }
            let Some(whole) = c.get(0) else {
                continue;
            };
            let after = &text[whole.end()..];
            let Some(name) = def.captures(after).and_then(|d| d.get(1)) else {
                continue;
            };
            let line = text[..whole.end() + name.start()].matches('\n').count() + 1;
            let paths: Vec<String> = full_prefixes(&key, &routers, &included, 0)
                .into_iter()
                .map(|prefix| {
                    let path = format!("{prefix}{}", &c[3]);
                    if path.is_empty() {
                        "/".to_owned()
                    } else {
                        path.replace("//", "/")
                    }
                })
                .collect();
            let methods: Vec<String> = match &c[2] {
                "websocket" => vec!["WEBSOCKET".to_owned()],
                "route" | "api_route" => METHODS_ARG
                    .as_ref()
                    .and_then(|re| re.captures(&c[4]))
                    .map(|m| {
                        m[1].split(',')
                            .map(|w| w.trim().trim_matches(['"', '\'']).to_uppercase())
                            .filter(|w| !w.is_empty())
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["GET".to_owned()]),
                method => vec![method.to_uppercase()],
            };
            let summary = docstring(&after[name.end()..]);
            let tag = routers.get(&key).and_then(|(_, t)| t.clone());
            for path in paths {
                let parameters: Vec<Parameter> = path
                    .split('/')
                    .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
                    .map(|p| Parameter {
                        name: p.split(':').next().unwrap_or(p).to_owned(),
                        location: "path".to_owned(),
                        required: true,
                        kind: String::new(),
                        description: String::new(),
                        example: None,
                    })
                    .collect();
                for method in &methods {
                    operations.push(Operation {
                        id: name.as_str().to_owned(),
                        method: method.clone(),
                        path: path.clone(),
                        summary: summary.clone(),
                        description: String::new(),
                        parameters: parameters.clone(),
                        body: None,
                        responses: Vec::new(),
                        secured: false,
                        server: None,
                        tags: tag.iter().cloned().collect(),
                        deprecated: false,
                        spec: *node,
                        from_code: true,
                        handler: Some((*node, line)),
                        callers: Vec::new(),
                    });
                }
            }
        }
    }
    operations.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    operations
}

/// A route's path as spec and code both write it: its parameters unnamed,
/// since FastAPI's `{id:uuid}` is the spec's `{id}`, and no slash at its
/// end, which one may write and the other not.
fn route_key(path: &str) -> String {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| if s.starts_with('{') { "{}" } else { s })
        .collect();
    format!("/{}", parts.join("/"))
}

/// A router's whole prefixes, one for each place it is included: those of
/// the routers that include it, then its own.
fn full_prefixes(
    key: &RouterKey,
    routers: &HashMap<RouterKey, (String, Option<String>)>,
    included: &Includers,
    depth: usize,
) -> Vec<String> {
    let own = routers.get(key).map_or("", |(p, _)| p.as_str());
    let mut prefixes: Vec<String> = match included.get(key) {
        Some(includers) if depth < 8 => includers
            .iter()
            .flat_map(|(node, var, prefix)| {
                full_prefixes(&(*node, var.clone()), routers, included, depth + 1)
                    .into_iter()
                    .map(move |above| format!("{above}{prefix}{own}"))
            })
            .collect(),
        _ => vec![own.to_owned()],
    };
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

/// The first line of the docstring that opens a function's body.
fn docstring(after_name: &str) -> String {
    let Some(colon) = after_name.find("):").or_else(|| after_name.find(":\n")) else {
        return String::new();
    };
    let body = after_name[colon..].split_once('\n').map_or("", |(_, b)| b);
    let first = body.trim_start();
    for quote in ["\"\"\"", "'''"] {
        if let Some(rest) = first.strip_prefix(quote) {
            return rest
                .split(quote)
                .next()
                .unwrap_or("")
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .to_owned();
        }
    }
    String::new()
}

/// The front end's files that write an operation's path, as a client
/// calls it: `'$baseUrl/users/me'`, `` `${API}/dags/${id}` ``. The written
/// path must be the operation's whole path, or its end after a base the
/// client's URL holds (`/api`, `/v1`); a method called next to it narrows
/// it to that method's operation.
fn path_callers(map: &CodeMap, texts: &[(usize, String)], operations: &mut [Operation]) {
    static STRING: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r#"'([^'\n]*/[^'\n]*)'|"([^"\n]*/[^"\n]*)"|`([^`\n]*/[^`\n]*)`"#).ok()
    });
    static METHOD: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?i)\b(get|post|put|patch|delete)\s*(?:<[^>]*>)?\s*\(").ok());
    let Some(string) = STRING.as_ref() else {
        return;
    };
    let wanted: Vec<Vec<String>> = operations
        .iter()
        .map(|o| {
            o.path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .collect();
    for (node, text) in texts {
        let client = matches!(
            map.nodes[*node].kind,
            NodeKind::File {
                language: Language::TypeScript | Language::JavaScript | Language::Dart,
                ..
            }
        );
        if !client || !designed(map, *node) || generated(&map.nodes[*node].path, text) {
            continue;
        }
        for c in string.captures_iter(text) {
            let Some(found) = c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3)) else {
                continue;
            };
            let Some(written) = written_path(found.as_str()) else {
                continue;
            };
            // A method called on this line or the two before.
            let start = text[..found.start()]
                .rmatch_indices('\n')
                .nth(2)
                .map_or(0, |(i, _)| i);
            let method = METHOD.as_ref().and_then(|re| {
                re.captures_iter(&text[start..found.start()])
                    .last()
                    .map(|m| m[1].to_uppercase())
            });
            for (operation, path) in operations.iter_mut().zip(&wanted) {
                if method.as_ref().is_some_and(|m| *m != operation.method)
                    || !path_matches(path, &written)
                {
                    continue;
                }
                if !operation.callers.contains(node) {
                    operation.callers.push(*node);
                }
            }
        }
    }
}

/// The parts of the path a string writes, `*` for what is put in, or
/// `None` when it writes none.
fn written_path(text: &str) -> Option<Vec<String>> {
    static PUT_IN: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"^(?:\$\{[^}]*\}|\$[A-Za-z_]\w*|\{[^}]*\})+").ok());
    let mut rest = text.trim();
    // An address's path.
    if let Some((_, after)) = rest.split_once("://") {
        rest = after.find('/').map_or("", |i| &after[i..]);
    }
    // What a base URL put in before it.
    if let Some(m) = PUT_IN.as_ref().and_then(|re| re.find(rest)) {
        rest = &rest[m.end()..];
    }
    if !rest.starts_with('/') || rest.contains(' ') {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let parts: Vec<String> = rest
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.contains('$') || s.starts_with(':') || s.starts_with('{') {
                "*".to_owned()
            } else {
                s.to_owned()
            }
        })
        .collect();
    (!parts.is_empty() && parts.iter().any(|p| p != "*")).then_some(parts)
}

/// Whether a written path is an operation's: all of it, or its end after
/// a base of `api`, `rest` or a version.
fn path_matches(operation: &[String], written: &[String]) -> bool {
    if written.len() > operation.len() {
        return false;
    }
    let base = operation.len() - written.len();
    let base_ok = operation[..base].iter().all(|s| {
        s == "api"
            || s == "rest"
            || (s.len() > 1 && s.starts_with('v') && s[1..].chars().all(|c| c.is_ascii_digit()))
    });
    base_ok
        && operation[base..].iter().zip(written).all(|(o, w)| {
            let parameter = o.starts_with('{') && o.ends_with('}');
            if parameter { w == "*" } else { o == w }
        })
}

/// The operations a spec's text declares, in its order, with no handler
/// nor caller: to compare two versions of a spec.
pub(crate) fn spec_operations(text: &str) -> Vec<Operation> {
    load(text).map_or_else(Vec::new, |doc| operations_of(0, &doc))
}

/// A spec's document, read as YAML, JSON being YAML too; JSON is read as
/// JSON, many times faster, which a spec of megabytes notices.
fn load(text: &str) -> Option<Yaml> {
    if text.trim_start().starts_with('{')
        && let Ok(Json(doc)) = serde_json::from_str(text)
    {
        return Some(doc);
    }
    YamlLoader::load_from_str(text).ok()?.into_iter().next()
}

/// A JSON document read straight into YAML's types, its keys in their
/// order, which serde's own `Value` would sort.
struct Json(Yaml);

impl<'de> serde::Deserialize<'de> for Json {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor).map(Json)
    }
}

struct JsonVisitor;

impl<'de> serde::de::Visitor<'de> for JsonVisitor {
    type Value = Yaml;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Yaml, E> {
        Ok(Yaml::Boolean(v))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Yaml, E> {
        Ok(Yaml::Integer(v))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Yaml, E> {
        Ok(i64::try_from(v).map_or_else(|_| Yaml::Real(v.to_string()), Yaml::Integer))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Yaml, E> {
        Ok(Yaml::Real(v.to_string()))
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Yaml, E> {
        Ok(Yaml::String(v.to_owned()))
    }

    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Yaml, E> {
        Ok(Yaml::String(v))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Yaml, E> {
        Ok(Yaml::Null)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Yaml, A::Error> {
        let mut items = Vec::new();
        while let Some(Json(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(Yaml::Array(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Yaml, A::Error> {
        let mut hash = yaml_rust2::yaml::Hash::new();
        while let Some((key, Json(value))) = map.next_entry::<String, Json>()? {
            hash.insert(Yaml::String(key), value);
        }
        Ok(Yaml::Hash(hash))
    }
}

/// The specs to read operations from: of the versions of one API, the
/// latest. Versions share a title and most of their paths, as an API that
/// keeps a spec per version does (`2026-04`, `2026-10`), or a copy of one
/// for its tests.
fn latest<'a>(map: &CodeMap, specs: &'a [(usize, Yaml)]) -> Vec<(usize, &'a Yaml)> {
    let title = |doc: &'a Yaml| doc["info"]["title"].as_str().unwrap_or_default().trim();
    let paths = |doc: &'a Yaml| -> HashSet<&'a str> {
        doc["paths"]
            .as_hash()
            .into_iter()
            .flat_map(|h| h.keys())
            .filter_map(Yaml::as_str)
            .collect()
    };
    let versions_of_one = |a: &'a Yaml, b: &'a Yaml| {
        let (ours, theirs) = (paths(a), paths(b));
        let shared = ours.intersection(&theirs).count();
        !title(a).is_empty()
            && title(a) == title(b)
            && shared * 2 >= ours.len().min(theirs.len()).max(1)
    };
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, (_, doc)) in specs.iter().enumerate() {
        match groups
            .iter_mut()
            .find(|g| g.iter().any(|&j| versions_of_one(doc, &specs[j].1)))
        {
            Some(group) => group.push(i),
            None => groups.push(vec![i]),
        }
    }
    // The highest version, then one that is not a test's nor a document's,
    // then the first.
    let version = |i: usize| specs[i].1["info"]["version"].as_str().unwrap_or_default();
    let aside = |i: usize| worth(Path::new(&map.nodes[specs[i].0].path));
    groups
        .iter()
        .filter_map(|group| {
            group.iter().copied().max_by(|&a, &b| {
                version_order(version(a), version(b))
                    .then(aside(b).cmp(&aside(a)))
                    .then(b.cmp(&a))
            })
        })
        .map(|i| (specs[i].0, &specs[i].1))
        .collect()
}

/// Two versions in their natural order: their numbers as numbers, so that
/// `1.10` comes after `1.9` and `2027-01` after `2026-10`.
fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(v: &str) -> Vec<(bool, &str)> {
        let mut parts = Vec::new();
        let mut start = 0;
        for (i, c) in v.char_indices().skip(1) {
            let previous = v[..i]
                .chars()
                .next_back()
                .is_some_and(|p| p.is_ascii_digit());
            if c.is_ascii_digit() != previous {
                parts.push(&v[start..i]);
                start = i;
            }
        }
        if !v.is_empty() {
            parts.push(&v[start..]);
        }
        parts
            .into_iter()
            .map(|p| (p.starts_with(|c: char| c.is_ascii_digit()), p))
            .collect()
    }
    for (x, y) in parts(a).into_iter().zip(parts(b)) {
        let order = match (x, y) {
            ((true, x), (true, y)) => {
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                x.len().cmp(&y.len()).then(x.cmp(y))
            }
            ((_, x), (_, y)) => x.cmp(y),
        };
        if order.is_ne() {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// The operations a spec declares, in its order.
fn operations_of(spec: usize, doc: &Yaml) -> Vec<Operation> {
    let Yaml::Hash(paths) = &doc["paths"] else {
        return Vec::new();
    };
    let server = doc["servers"][0]["url"].as_str().map(str::to_owned);
    let secured_by_default = doc["security"].as_vec().is_some_and(|s| !s.is_empty());
    let mut operations = Vec::new();
    for (path, item) in paths {
        let Some(path) = path.as_str() else {
            continue;
        };
        let item = resolve(doc, item);
        for method in METHODS {
            let operation = &item[method];
            if operation.is_badvalue() {
                continue;
            }
            let text = |key: &str| {
                operation[key]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            };
            let id = match text("operationId") {
                id if id.is_empty() => format!("{} {path}", method.to_uppercase()),
                id => id,
            };
            // The path's own parameters, then the operation's, which win.
            let mut parameters: Vec<Parameter> = Vec::new();
            for list in [&item["parameters"], &operation["parameters"]] {
                for parameter in list.as_vec().into_iter().flatten() {
                    let Some(parameter) = parameter_of(doc, parameter) else {
                        continue;
                    };
                    parameters.retain(|p| {
                        !(p.name == parameter.name && p.location == parameter.location)
                    });
                    parameters.push(parameter);
                }
            }
            let secured = match operation["security"].as_vec() {
                Some(list) => !list.is_empty(),
                None => secured_by_default,
            };
            operations.push(Operation {
                from_code: false,
                id,
                method: method.to_uppercase(),
                path: path.to_owned(),
                summary: text("summary"),
                description: text("description"),
                parameters,
                body: body_of(doc, &operation["requestBody"]),
                responses: responses_of(doc, &operation["responses"]),
                secured,
                server: server.clone(),
                tags: operation["tags"]
                    .as_vec()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect(),
                deprecated: operation["deprecated"].as_bool().unwrap_or(false),
                spec,
                handler: None,
                callers: Vec::new(),
            });
        }
    }
    operations
}

/// What `node` stands for: itself, or what its `$ref` names in the same
/// spec, followed a few times.
fn resolve<'a>(doc: &'a Yaml, node: &'a Yaml) -> &'a Yaml {
    let mut node = node;
    for _ in 0..8 {
        let Some(reference) = node["$ref"].as_str() else {
            return node;
        };
        let Some(pointer) = reference.strip_prefix("#/") else {
            return node;
        };
        let mut target = doc;
        for part in pointer.split('/') {
            target = &target[part.replace("~1", "/").replace("~0", "~").as_str()];
        }
        if target.is_badvalue() {
            return node;
        }
        node = target;
    }
    node
}

/// The name of the spec's schema `schema` refers to, if it does.
fn schema_name(schema: &Yaml) -> Option<String> {
    let reference = schema["$ref"].as_str()?;
    Some(reference.rsplit('/').next().unwrap_or(reference).to_owned())
}

fn parameter_of(doc: &Yaml, parameter: &Yaml) -> Option<Parameter> {
    let parameter = resolve(doc, parameter);
    let name = parameter["name"].as_str()?.to_owned();
    let location = parameter["in"].as_str().unwrap_or("query").to_owned();
    let schema = &parameter["schema"];
    let example = parameter["example"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| plain(&resolve(doc, schema)["example"]))
        .or_else(|| plain(&resolve(doc, schema)["default"]));
    Some(Parameter {
        required: parameter["required"]
            .as_bool()
            .unwrap_or(location == "path"),
        // A path's parameter is text, whatever the spec leaves unsaid.
        kind: match kind(schema) {
            any if any == "any" && location == "path" => "string".to_owned(),
            kind => kind,
        },
        description: parameter["description"]
            .as_str()
            .or_else(|| resolve(doc, schema)["description"].as_str())
            .unwrap_or_default()
            .trim()
            .to_owned(),
        name,
        location,
        example,
    })
}

/// A scalar of YAML as text, for a parameter's example.
fn plain(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(s) | Yaml::Real(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A schema's type, in a word or two: `string`, `integer`, `DagRun[]`,
/// `string | null`.
fn kind(schema: &Yaml) -> String {
    if let Some(name) = schema_name(schema) {
        return name;
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(options) = schema[key].as_vec() {
            let kinds: Vec<String> = options.iter().map(kind).collect();
            return kinds.join(" | ");
        }
    }
    match schema["type"].as_str() {
        Some("array") => format!("{}[]", kind(&schema["items"])),
        Some(kind) => match schema["format"].as_str() {
            Some(format) => format!("{kind} ({format})"),
            None => kind.to_owned(),
        },
        None if schema["enum"].as_vec().is_some() => "enum".to_owned(),
        None => "any".to_owned(),
    }
}

/// The media type to show of a `content`: JSON when there is.
fn media_of(content: &Yaml) -> Option<(String, &Yaml)> {
    let Yaml::Hash(content) = content else {
        return None;
    };
    let mut first = None;
    for (media, value) in content {
        let Some(media) = media.as_str() else {
            continue;
        };
        if media.contains("json") {
            return Some((media.to_owned(), value));
        }
        first.get_or_insert((media.to_owned(), value));
    }
    first
}

fn body_of(doc: &Yaml, body: &Yaml) -> Option<Body> {
    if body.is_badvalue() {
        return None;
    }
    let body = resolve(doc, body);
    let (media, value) = media_of(&body["content"])?;
    Some(Body {
        schema: schema_name(&value["schema"]),
        required: body["required"].as_bool().unwrap_or(false),
        example: example_of(doc, value),
        media,
    })
}

fn responses_of(doc: &Yaml, responses: &Yaml) -> Vec<Response> {
    let Yaml::Hash(responses) = responses else {
        return Vec::new();
    };
    let mut found: Vec<Response> = responses
        .iter()
        .map(|(status, response)| {
            let status = match status {
                Yaml::Integer(code) => code.to_string(),
                other => other.as_str().unwrap_or_default().to_owned(),
            };
            let response = resolve(doc, response);
            let media = media_of(&response["content"]);
            Response {
                status,
                description: response["description"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
                schema: media
                    .as_ref()
                    .and_then(|(_, value)| schema_name(&value["schema"])),
                example: media.and_then(|(_, value)| example_of(doc, value)),
            }
        })
        .collect();
    // Successes first, then the errors, each in the order of their codes.
    found.sort_by(|a, b| a.status.cmp(&b.status));
    found
}

/// An example of what a media type holds, as pretty JSON: the spec's own,
/// or one made from its schema.
fn example_of(doc: &Yaml, media: &Yaml) -> Option<String> {
    let given = if media["example"].is_badvalue() {
        media["examples"]
            .as_hash()
            .and_then(|examples| examples.values().next())
            .map(|example| &resolve(doc, example)["value"])
            .filter(|value| !value.is_badvalue())
    } else {
        Some(&media["example"])
    };
    let value = match given {
        Some(value) => json(value),
        None => {
            let schema = &media["schema"];
            if schema.is_badvalue() {
                return None;
            }
            made(doc, schema, 0)
        }
    };
    serde_json::to_string_pretty(&value).ok()
}

/// YAML as JSON.
fn json(value: &Yaml) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Yaml::String(s) => Value::String(s.clone()),
        Yaml::Integer(i) => Value::from(*i),
        Yaml::Real(r) => r.parse::<f64>().map_or(Value::Null, Value::from),
        Yaml::Boolean(b) => Value::Bool(*b),
        Yaml::Array(items) => Value::Array(items.iter().map(json).collect()),
        Yaml::Hash(hash) => Value::Object(
            hash.iter()
                .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), json(v))))
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// A value of the shape `schema` describes: its example, default or first
/// choice where it gives one, a placeholder of its type otherwise.
fn made(doc: &Yaml, schema: &Yaml, depth: usize) -> serde_json::Value {
    use serde_json::Value;
    let schema = resolve(doc, schema);
    for key in ["example", "default"] {
        if !schema[key].is_badvalue() {
            return json(&schema[key]);
        }
    }
    if let Some(first) = schema["enum"].as_vec().and_then(|e| e.first()) {
        return json(first);
    }
    if depth > EXAMPLE_DEPTH {
        return Value::Null;
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(options) = schema[key].as_vec() {
            // The first that is not null.
            let option = options
                .iter()
                .find(|o| resolve(doc, o)["type"].as_str() != Some("null"))
                .or(options.first());
            return option.map_or(Value::Null, |o| made(doc, o, depth + 1));
        }
    }
    if let Some(parts) = schema["allOf"].as_vec() {
        let mut merged = serde_json::Map::new();
        for part in parts {
            if let Value::Object(fields) = made(doc, part, depth + 1) {
                merged.extend(fields);
            }
        }
        return Value::Object(merged);
    }
    match schema["type"].as_str() {
        Some("object") | None if schema["properties"].as_hash().is_some() => {
            let fields = schema["properties"]
                .as_hash()
                .into_iter()
                .flatten()
                .filter_map(|(name, property)| {
                    Some((name.as_str()?.to_owned(), made(doc, property, depth + 1)))
                })
                .collect();
            Value::Object(fields)
        }
        Some("object") => Value::Object(serde_json::Map::new()),
        Some("array") => Value::Array(vec![made(doc, &schema["items"], depth + 1)]),
        Some("integer") => Value::from(0),
        Some("number") => Value::from(0.0),
        Some("boolean") => Value::Bool(true),
        Some("string") => Value::String(
            match schema["format"].as_str() {
                Some("date-time") => "2025-01-01T00:00:00Z",
                Some("date") => "2025-01-01",
                Some("uuid") => "00000000-0000-0000-0000-000000000000",
                Some("uri" | "url") => "https://example.com",
                Some("email") => "someone@example.com",
                _ => "string",
            }
            .to_owned(),
        ),
        _ => Value::Null,
    }
}

impl Operation {
    /// A request to it with `curl`: its parameters as placeholders, unless
    /// the spec gives an example, and its body made from its schema.
    #[must_use]
    pub fn curl(&self) -> String {
        let base = self
            .server
            .as_deref()
            .filter(|s| s.starts_with("http"))
            .unwrap_or("http://localhost:8080")
            .trim_end_matches('/');
        let mut path = self.path.clone();
        for parameter in self.parameters.iter().filter(|p| p.location == "path") {
            let value = parameter
                .example
                .clone()
                .unwrap_or_else(|| format!("<{}>", parameter.name));
            path = path.replace(&format!("{{{}}}", parameter.name), &value);
        }
        let query: Vec<String> = self
            .parameters
            .iter()
            .filter(|p| p.location == "query" && p.required)
            .map(|p| {
                let value = p.example.clone().unwrap_or_else(|| format!("<{}>", p.name));
                format!("{}={value}", p.name)
            })
            .collect();
        if !query.is_empty() {
            path = format!("{path}?{}", query.join("&"));
        }
        let mut lines = vec![format!("curl -X {} \"{base}{path}\"", self.method)];
        if self.secured {
            lines.push("  -H \"Authorization: Bearer $TOKEN\"".to_owned());
        }
        for header in self
            .parameters
            .iter()
            .filter(|p| p.location == "header" && p.required)
        {
            lines.push(format!("  -H \"{}: <{}>\"", header.name, header.name));
        }
        if let Some(body) = &self.body {
            lines.push(format!("  -H \"Content-Type: {}\"", body.media));
            if let Some(example) = &body.example {
                lines.push(format!("  -d '{}'", example.replace('\'', "'\\''")));
            }
        }
        lines.join(" \\\n")
    }
}

/// Whether a settings file is an OpenAPI (or Swagger) spec: it has an
/// `openapi` or `swagger` key at its top, wherever that key is, as a
/// generator that sorts keys writes `components` first. A JSON spec's top
/// keys are read once it parses; a YAML one's start their line. Only JSON
/// cut short is judged by its start.
pub(crate) fn is_spec(text: &str) -> bool {
    let top = |key: &str| key == "openapi" || key == "swagger";
    if text.trim_start().starts_with('{') {
        if let Ok(serde_json::Value::Object(keys)) = serde_json::from_str(text) {
            return keys.keys().any(|k| top(k));
        }
        let start: String = text.chars().take(400).collect();
        return start.lines().any(|line| {
            let line = line.trim_start_matches(['{', ' ', '\t']);
            line.starts_with("\"openapi\"") || line.starts_with("\"swagger\"")
        });
    }
    text.lines().any(|line| {
        let key = line
            .split(':')
            .next()
            .unwrap_or("")
            .trim_matches(['"', '\'']);
        line.contains(':') && !line.starts_with([' ', '\t']) && top(key)
    })
}

/// Code of the design: not a test, an example or a document.
fn designed(map: &CodeMap, node: usize) -> bool {
    worth(Path::new(&map.nodes[node].path)) == 0
}

/// Whether a file is generated from a spec, as clients are: it names every
/// operation, and calls none. Known by its path, or by what its first lines
/// say, as a generator writing into an ordinary folder says.
fn generated(path: &str, text: &str) -> bool {
    if path.contains("openapi-gen/") || path.contains("/generated/") || path.contains(".gen.") {
        return true;
    }
    text.lines().take(12).any(|line| {
        let line = line.to_lowercase();
        line.contains("@generated") || line.contains("do not edit") || line.contains("generated by")
    })
}

/// The names an identifier may call an operation by: itself, and each of
/// its ends that starts with a capital (`useDagServiceGetDag` ends with
/// `GetDag`), with or without what clients add after (`GetDagKeyFn`).
fn called(word: &str) -> Vec<&str> {
    let mut names = vec![word];
    for (i, c) in word.char_indices().skip(1) {
        if c.is_ascii_uppercase() {
            names.push(&word[i..]);
        }
    }
    let mut trimmed = Vec::new();
    for name in &names {
        for suffix in CLIENT_SUFFIXES {
            if let Some(stem) = name.strip_suffix(suffix).filter(|s| !s.is_empty()) {
                trimmed.push(stem);
            }
        }
    }
    names.extend(trimmed);
    names
}

/// Whether a name is one word: `State`, `login`, not `GetDag`.
fn one_word(name: &str) -> bool {
    name.chars().skip(1).all(|c| !c.is_ascii_uppercase())
}

/// `get_dag_runs` as `GetDagRuns`; `getDag` as `GetDag`.
pub(crate) fn pascal(id: &str) -> String {
    id.split(['_', '-'])
        .filter(|p| !p.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or(String::new(), |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect()
}

pub(crate) fn lower_first(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or(String::new(), |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

/// How many leading path parts two paths share.
fn shared_prefix(a: &str, b: &str) -> usize {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| x == y)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{EdgeKind, map};

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_page_calls_the_route_that_serves_the_operation_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "api/openapi/v2.yaml",
            "openapi: 3.1.0\npaths:\n  /dags/{id}:\n    get:\n      tags:\n      - DAG\n      summary: Get Dag\n      operationId: get_dag\n  /dags:\n    get:\n      operationId: get_dags\n  /jobs/{state}:\n    patch:\n      operationId: state\n",
        );
        write(
            root,
            "api/routes/dags.py",
            "@router.get(\n    \"/{id}\",\n    responses={},\n)\ndef get_dag(id):\n    pass\n\n@router.get(\"\")\nasync def get_dags():\n    pass\n\n@router.patch(\"/jobs\")\ndef state():\n    pass\n",
        );
        // An id given to the decorator rather than the function's name.
        write(
            root,
            "api/routes/ui.py",
            "@ui_router.get(\"/ui/dags\", operation_id=\"get_dags\")\ndef dags_for_the_ui():\n    pass\n",
        );
        // The same name elsewhere, farther from the spec, unrouted or not.
        write(
            root,
            "worker/routes.py",
            "@app.get('/x')\ndef get_dag():\n    pass\n",
        );
        write(
            root,
            "ui/src/Dag.tsx",
            "const { data } = useDagServiceGetDag({ id });\nconst key = UseDagServiceGetDagsKeyFn();\n",
        );
        write(
            root,
            "ui/src/Other.tsx",
            "const forgetDag = 1;\nconst [s, setState] = useState(0);\n",
        );
        write(root, "ui/src/Jobs.tsx", "EdgeService.state(job);\n");
        write(
            root,
            "ui/openapi-gen/queries.ts",
            "export const useDagServiceGetDag = () => DagService.getDag();\n",
        );
        let map = map(root, 100);
        let calls: Vec<(&str, &str)> = map
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| {
                (
                    map.nodes[e.from].path.as_str(),
                    map.nodes[e.to].path.as_str(),
                )
            })
            .collect();
        assert_eq!(
            calls,
            [
                ("api/routes/dags.py", "ui/src/Dag.tsx"),
                ("api/routes/ui.py", "ui/src/Dag.tsx"),
                ("api/routes/dags.py", "ui/src/Jobs.tsx"),
            ]
            .map(|(to, from)| (from, to))
        );
        // A one-word operation: called by the client's form only.
        let state = map.operations.iter().find(|o| o.id == "state").unwrap();
        let callers: Vec<&str> = state
            .callers
            .iter()
            .map(|&c| map.nodes[c].path.as_str())
            .collect();
        assert_eq!(callers, ["ui/src/Jobs.tsx"]);

        // Each operation, with what serves and what calls it.
        let get_dag = map.operations.iter().find(|o| o.id == "get_dag").unwrap();
        assert_eq!(
            (get_dag.method.as_str(), get_dag.path.as_str()),
            ("GET", "/dags/{id}")
        );
        assert_eq!(get_dag.summary, "Get Dag");
        assert_eq!(get_dag.tags, ["DAG"]);
        let (handler, line) = get_dag.handler.unwrap();
        assert_eq!(
            (map.nodes[handler].path.as_str(), line),
            ("api/routes/dags.py", 5)
        );
        let callers: Vec<&str> = get_dag
            .callers
            .iter()
            .map(|&c| map.nodes[c].path.as_str())
            .collect();
        assert_eq!(callers, ["ui/src/Dag.tsx"]);
        // Called by its key's name too; served by the function given its id.
        let get_dags = map.operations.iter().find(|o| o.id == "get_dags").unwrap();
        let handlers: Vec<&str> = get_dags
            .handler
            .iter()
            .map(|(n, _)| map.nodes[*n].path.as_str())
            .collect();
        assert_eq!(handlers, ["api/routes/ui.py"]);
        assert_eq!(get_dags.callers, get_dag.callers);
    }

    #[test]
    fn versions_of_one_api_count_once_the_latest_and_not_a_tests_copy() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let shop = |version: &str, paths: &str| {
            format!(
                r#"{{"openapi": "3.1.0", "info": {{"title": "Shop API", "version": "{version}"}}, "paths": {{{paths}}}}}"#
            )
        };
        let items = r#""/v1/items/": {"get": {"operationId": "items:list"}}"#;
        let item = r#""/v1/items/{id}": {"get": {"operationId": "items:get"}}"#;
        write(
            root,
            "api/openapi.json",
            &shop("2026-10", &format!("{items}, {item}")),
        );
        write(
            root,
            "server/tests/openapi.json",
            &shop("2026-10", &format!("{items}, {item}")),
        );
        write(
            root,
            "docs/2026-04.yaml",
            "openapi: 3.1.0\ninfo:\n  title: Shop API\n  version: 2026-04\npaths:\n  /v1/items/:\n    get:\n      operationId: items:list\n  /v1/old:\n    get:\n      operationId: old\n",
        );
        // Another API, though it shares a path.
        write(
            root,
            "admin/openapi.yaml",
            "openapi: 3.1.0\ninfo:\n  title: Admin API\n  version: '1'\npaths:\n  /v1/items/:\n    get:\n      operationId: admin_items\n",
        );
        let map = map(root, 100);
        let read: Vec<(&str, &str)> = map
            .operations
            .iter()
            .map(|o| (map.nodes[o.spec].path.as_str(), o.id.as_str()))
            .collect();
        assert_eq!(
            read,
            [
                ("admin/openapi.yaml", "admin_items"),
                ("api/openapi.json", "items:list"),
                ("api/openapi.json", "items:get"),
            ]
        );
    }

    #[test]
    fn a_json_spec_is_read_in_its_order() {
        let doc = load(r#"{"paths": {"/b": {"get": {}}, "/a": {"get": {}}}}"#).unwrap();
        let paths: Vec<String> = operations_of(0, &doc).into_iter().map(|o| o.path).collect();
        assert_eq!(paths, ["/b", "/a"]);
    }

    #[test]
    fn versions_in_their_natural_order() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        assert_eq!(version_order("1.9", "1.10"), Less);
        assert_eq!(version_order("2026-10", "2027-01"), Less);
        assert_eq!(version_order("v2", "v1"), Greater);
        assert_eq!(version_order("1.0", "1.0"), Equal);
        assert_eq!(version_order("1.0", "1.0.1"), Less);
        assert_eq!(version_order("", "1"), Less);
    }

    #[test]
    fn an_operation_the_code_does_not_name_is_served_by_its_method_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Ids made by FastAPI from tags and names, which no code writes.
        write(
            root,
            "docs/openapi.json",
            r#"{"openapi": "3.1.0", "info": {"title": "Shop API"}, "paths": {"/v1/items/{id}": {"get": {"operationId": "items:get"}}, "/v1/portal/items/": {"get": {"operationId": "portal:items:list"}}, "/v1/users/items/": {"get": {"operationId": "users:items:list"}}}}"#,
        );
        write(
            root,
            "server/shop/api.py",
            "from fastapi import APIRouter\nfrom shop.items import router as items_router\nfrom shop.portal import router as portal_router\nfrom shop.users import router as users_router\n\nrouter = APIRouter(prefix=\"/v1\")\nrouter.include_router(items_router)\nrouter.include_router(portal_router)\nrouter.include_router(users_router)\n",
        );
        write(
            root,
            "server/shop/items.py",
            "router = APIRouter(prefix=\"/items\")\n\n\n@router.get(\n    \"/{id:uuid}\",\n    summary=\"Get\",\n)\nasync def get(id):\n    pass\n",
        );
        // Included by a relative import, and in two routers.
        write(
            root,
            "server/shop/portal/__init__.py",
            "from .items import router as items_router\n\nrouter = APIRouter(prefix=\"/portal\")\nrouter.include_router(items_router)\n",
        );
        write(
            root,
            "server/shop/portal/items.py",
            "router = APIRouter(prefix=\"/items\")\n\n@router.get(\"/\")\nasync def list():\n    pass\n",
        );
        write(
            root,
            "server/shop/users.py",
            "from shop.portal.items import router as portal_items\n\nrouter = APIRouter(prefix=\"/users\")\nrouter.include_router(portal_items)\n",
        );
        let map = map(root, 100);
        let served: Vec<(&str, Option<(&str, usize)>)> = map
            .operations
            .iter()
            .map(|o| {
                (
                    o.id.as_str(),
                    o.handler
                        .map(|(n, line)| (map.nodes[n].path.as_str(), line)),
                )
            })
            .collect();
        assert_eq!(
            served,
            [
                ("items:get", Some(("server/shop/items.py", 8))),
                (
                    "portal:items:list",
                    Some(("server/shop/portal/items.py", 4))
                ),
                ("users:items:list", Some(("server/shop/portal/items.py", 4))),
            ]
        );
    }

    #[test]
    fn a_json_spec_and_an_operation_with_no_id() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "spec.json",
            r#"{"openapi": "3.0.0", "paths": {"/ping": {"post": {"summary": "Ping", "deprecated": true}}}}"#,
        );
        let map = map(dir.path(), 100);
        assert_eq!(map.operations.len(), 1);
        assert_eq!(map.operations[0].id, "POST /ping");
        assert!(map.operations[0].deprecated);
    }

    #[test]
    fn names_a_client_calls_an_operation_by() {
        assert_eq!(pascal("get_dag_runs"), "GetDagRuns");
        assert_eq!(pascal("listBackfills"), "ListBackfills");
        assert_eq!(lower_first("GetDag"), "getDag");
        let names = called("UseDagServiceGetDagKeyFn");
        assert!(names.contains(&"GetDag"));
        assert!(!called("forgetDag").contains(&"GetDag"));
        assert!(is_spec("{\n  \"openapi\": \"3.0.0\""));
        assert!(!is_spec("name: provider\n"));
    }

    #[test]
    fn a_spec_whose_keys_are_sorted_is_still_a_spec() {
        // Its components first, far from the start.
        let schemas: String = (0..200)
            .map(|i| format!("    Thing{i}:\n      type: object\n"))
            .collect();
        let yaml = format!(
            "components:\n  schemas:\n{schemas}info:\n  title: x\nopenapi: 3.1.0\npaths: {{}}\n"
        );
        assert!(is_spec(&yaml));
        let json = format!(
            "{{\"components\": {{\"schemas\": {{{}}}}}, \"openapi\": \"3.1.0\", \"paths\": {{}}}}",
            (0..200)
                .map(|i| format!("\"T{i}\": {{}}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert!(is_spec(&json));
        // An openapi key inside, not at the top, makes no spec.
        assert!(!is_spec("tools:\n  openapi: 3.0.0\n"));
        assert!(!is_spec("{\"tools\": {\"openapi\": \"3.0.0\"}}"));
    }

    #[test]
    fn a_generated_client_says_so_at_its_top() {
        let client = "/**\n * Generated by orval v7\n * Do not edit manually.\n */\nexport const getThing = () => 1;\n";
        assert!(generated("src/api/client.ts", client));
        assert!(generated(
            "src/x.ts",
            "// @generated\nexport const a = 1;\n"
        ));
        assert!(generated("ui/openapi-gen/queries.ts", ""));
        assert!(!generated(
            "src/pages/Things.tsx",
            "import { getThing } from \"../api/client\";\n"
        ));
        // Said far down, it is not a header.
        let late = format!("{}// generated by hand\n", "const a = 1;\n".repeat(20));
        assert!(!generated("src/a.ts", &late));
    }

    #[test]
    fn what_an_operation_takes_and_answers_and_a_request_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let spec = r#"openapi: 3.1.0
servers:
- url: https://api.example.com
security:
- HTTPBearer: []
paths:
  /dags/{dag_id}/dagRuns:
    parameters:
    - name: dag_id
      in: path
      required: true
      schema:
        type: string
    post:
      summary: Trigger Dag Run
      description: Trigger a Dag.
      operationId: trigger_dag_run
      parameters:
      - $ref: '#/components/parameters/Limit'
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/TriggerBody'
      responses:
        '404':
          description: Not Found
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Error'
        '200':
          description: Successful Response
          content:
            application/json:
              example: {dag_run_id: manual_1}
components:
  parameters:
    Limit:
      name: limit
      in: query
      schema:
        type: integer
        default: 50
  schemas:
    TriggerBody:
      type: object
      properties:
        logical_date:
          anyOf:
          - type: 'null'
          - type: string
            format: date-time
        conf:
          type: object
        note:
          type: string
          example: it's late
    Error:
      type: object
      properties:
        detail:
          type: string
"#;
        write(dir.path(), "openapi.yaml", spec);
        let map = map(dir.path(), 100);
        let operation = &map.operations[0];
        assert_eq!(operation.description, "Trigger a Dag.");
        let parameters: Vec<(&str, &str, bool, &str)> = operation
            .parameters
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    p.location.as_str(),
                    p.required,
                    p.kind.as_str(),
                )
            })
            .collect();
        assert_eq!(
            parameters,
            [
                ("dag_id", "path", true, "string"),
                ("limit", "query", false, "integer")
            ]
        );
        assert_eq!(operation.parameters[1].example.as_deref(), Some("50"));
        let body = operation.body.as_ref().unwrap();
        assert_eq!(body.schema.as_deref(), Some("TriggerBody"));
        assert!(body.required);
        let example: serde_json::Value =
            serde_json::from_str(body.example.as_deref().unwrap()).unwrap();
        assert_eq!(
            example,
            serde_json::json!({"logical_date": "2025-01-01T00:00:00Z", "conf": {}, "note": "it's late"})
        );
        let statuses: Vec<&str> = operation
            .responses
            .iter()
            .map(|r| r.status.as_str())
            .collect();
        assert_eq!(statuses, ["200", "404"]);
        assert_eq!(operation.responses[1].schema.as_deref(), Some("Error"));
        assert!(
            operation.responses[0]
                .example
                .as_deref()
                .unwrap()
                .contains("manual_1")
        );
        assert!(operation.secured);

        let curl = operation.curl();
        assert!(curl.starts_with("curl -X POST \"https://api.example.com/dags/<dag_id>/dagRuns\""));
        assert!(curl.contains("Authorization: Bearer $TOKEN"));
        assert!(curl.contains("-H \"Content-Type: application/json\""));
        // A quote in the body is escaped for the shell.
        assert!(curl.contains("it'\\''s late"));
    }

    #[test]
    fn routes_read_from_the_code_and_called_by_the_paths_an_app_writes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "compose.yaml",
            "services:\n  api:\n    build:\n      context: .\n      dockerfile: server/Dockerfile\n  web:\n    build:\n      context: .\n      dockerfile: app/Dockerfile.web\n",
        );
        write(
            root,
            "server/main.py",
            "from fastapi import FastAPI\nfrom server.routers import orders\nfrom server.live import router as live_router\n\napp = FastAPI()\napp.include_router(orders.router, prefix=\"/api\")\napp.include_router(live_router)\n\n@app.get(\"/health\")\ndef health():\n    return {}\n",
        );
        write(
            root,
            "server/routers/orders.py",
            "from fastapi import APIRouter\n\nrouter = APIRouter(prefix=\"/orders\", tags=[\"orders\"])\n\n@router.get(\"/{order_id}\")\nasync def get_order(order_id: int):\n    \"\"\"One order.\"\"\"\n    return {}\n\n@router.post(\"\")\ndef create_order():\n    return {}\n",
        );
        write(
            root,
            "server/live.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n\n@router.websocket(\"/ws/live\")\nasync def live(ws):\n    pass\n",
        );
        write(
            root,
            "app/pubspec.yaml",
            "name: shop_app\nenvironment:\n  sdk: ^3.0.0\ndependencies:\n  flutter:\n    sdk: flutter\n",
        );
        write(
            root,
            "app/Dockerfile.web",
            "FROM flutter AS build\nRUN flutter build web --release\n",
        );
        write(
            root,
            ".github/workflows/android.yml",
            "jobs:\n  build:\n    steps:\n      - run: flutter build appbundle\n",
        );
        write(
            root,
            "app/lib/main.dart",
            "import 'package:shop_app/services/client.dart';\nimport 'dart:async';\nvoid main() {}\n",
        );
        write(
            root,
            "app/lib/services/client.dart",
            "import '../models/order.dart';\nFuture<void> load(int id) async {\n  await http.get(Uri.parse('$baseUrl/orders/$id'));\n  await http.post(\n    Uri.parse('$baseUrl/orders'),\n  );\n  final ws = connect('${wsBase}/ws/live');\n}\n",
        );
        write(root, "app/lib/models/order.dart", "class Order {}\n");
        let map = map(root, 100);
        let found: Vec<String> = map
            .operations
            .iter()
            .map(|o| format!("{} {} {}", o.method, o.path, o.id))
            .collect();
        assert_eq!(
            found,
            [
                "POST /api/orders create_order",
                "GET /api/orders/{order_id} get_order",
                "GET /health health",
                "WEBSOCKET /ws/live live",
            ]
        );
        let order = map.operations.iter().find(|o| o.id == "get_order").unwrap();
        assert!(order.from_code);
        assert_eq!(order.summary, "One order.");
        assert_eq!(order.tags, ["orders"]);
        assert_eq!(
            map.nodes[order.handler.unwrap().0].path,
            "server/routers/orders.py"
        );
        let client = map.find("app/lib/services/client.dart").unwrap();
        for id in ["get_order", "create_order", "live"] {
            let o = map.operations.iter().find(|o| o.id == id).unwrap();
            assert_eq!(o.callers, [client], "{id}");
        }
        assert!(
            map.operations
                .iter()
                .find(|o| o.id == "health")
                .unwrap()
                .callers
                .is_empty()
        );
        // Dart imports: a package's own, and a relative one.
        let main = map.find("app/lib/main.dart").unwrap();
        let imports = |from: usize| -> Vec<&str> {
            map.edges
                .iter()
                .filter(|e| e.from == from && e.kind == EdgeKind::Imports)
                .map(|e| map.nodes[e.to].path.as_str())
                .collect()
        };
        assert_eq!(imports(main), ["app/lib/services/client.dart"]);
        assert_eq!(imports(client), ["app/lib/models/order.dart"]);
        // The app is built for the web by its Dockerfile and for Android by
        // a workflow; it reaches the server through its routes.
        let services = &map.services;
        let web = services
            .services
            .iter()
            .position(|s| s.name == "web")
            .unwrap();
        let api = services
            .services
            .iter()
            .position(|s| s.name == "api")
            .unwrap();
        assert_eq!(services.services[web].folder.as_deref(), Some("app"));
        assert_eq!(services.services[web].platforms, ["Android", "Web"]);
        assert!(
            services
                .links
                .iter()
                .any(|l| l.from == web && l.to == api && l.via == crate::services::Via::OpenApi)
        );
    }
}
