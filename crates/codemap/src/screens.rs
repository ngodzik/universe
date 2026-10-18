//! The screens of a project's front ends, as their routers declare them,
//! and what a person does on them: the links from one to another, the
//! operations of the API each calls through the files it imports, and the
//! workflows these make, each an action of the API (a `POST`, a `PATCH`, a
//! `DELETE`) at the screen that calls it most directly, with the way there.
//!
//! All by reading the files, no model: routes from the folders of a
//! Next.js app (`app/**/page.tsx`, `pages/**`), from React Router's `path`s
//! and TanStack Router's `createFileRoute`; links from the paths a file
//! writes that a route matches, `/dashboard/${org.slug}/products` among
//! them, in a link or in the data of a menu; what runs the front end from
//! its `package.json`, its lock file and its middleware.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::map::{CodeMap, EdgeKind, Language, NodeKind, worth};

/// A front end of the project: a package whose router serves screens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontEnd {
    /// Its folder, from the project's root: `clients/apps/web`; empty at
    /// the root.
    pub folder: String,
    /// Its name, as its `package.json` gives it, without its scope.
    pub name: String,
    /// The router its screens are read from.
    pub router: Router,
    /// What builds and serves it while it is worked on.
    pub framework: Framework,
    /// The tool that runs its scripts: `pnpm`, `npm`, `yarn` or `bun`.
    pub runner: String,
    /// What its `dev` script runs, as written, if it has one.
    pub dev: Option<String>,
    /// The variable of the environment it reads its API's address from:
    /// `NEXT_PUBLIC_API_URL`.
    pub api_env: Option<String>,
    /// The cookies its middleware reads, a session's among them.
    pub cookies: Vec<String>,
    /// Its screens, by route.
    pub screens: Vec<Screen>,
    /// What a person does on them, by group.
    pub workflows: Vec<Workflow>,
}

/// How a front end routes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Router {
    /// Next.js's `app` folder.
    #[default]
    NextApp,
    /// Next.js's `pages` folder.
    NextPages,
    /// React Router's routes, as objects or as `<Route>` elements.
    ReactRouter,
    /// TanStack Router's file routes.
    TanStack,
}

/// What builds and serves a front end while it is worked on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Framework {
    /// `next dev`.
    Next,
    /// `vite`.
    Vite,
    /// Anything else, run as its script says.
    #[default]
    Other,
}

/// A screen: a route its router serves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    /// Its route, parameters in brackets: `/dashboard/[organization]`.
    pub route: String,
    /// The file that renders it, from the project's root.
    pub file: String,
    /// The layouts around it, the outermost first.
    pub layouts: Vec<String>,
    /// The screens it links to, by their place in the front end's.
    pub links: Vec<usize>,
    /// The operations it calls through the files it imports, the nearest
    /// first.
    pub calls: Vec<Call>,
}

/// An operation a screen calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    /// Its method: `GET`.
    pub method: String,
    /// Its path in the spec: `/v1/products/{id}`.
    pub path: String,
    /// How many imports away from the screen's own files: 0 in one of
    /// them.
    pub depth: usize,
}

/// Something a person does on a front end: an action of the API, at the
/// screen that calls it most directly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    /// What it does, as the spec sums it up: `Create Product`.
    pub name: String,
    /// What it is about: the operation's first tag, else its path's first
    /// part after a version.
    pub group: String,
    /// The action's method: `POST`.
    pub method: String,
    /// The action's path in the spec: `/v1/products/`.
    pub path: String,
    /// The screens on the way, by their place in the front end's: the
    /// first where it starts, the last where it acts.
    pub steps: Vec<usize>,
}

/// The address of a route with each parameter given a value that serves
/// as an example: `demo` for a name, an id that looks like one for an id.
///
/// # Examples
///
/// ```
/// let url = universe_codemap::example_url("/dashboard/[organization]/customers/[id]");
/// assert_eq!(url, "/dashboard/demo/customers/00000000-0000-4000-8000-000000000001");
/// ```
#[must_use]
pub fn example_url(route: &str) -> String {
    let parts: Vec<String> = route
        .split('/')
        .filter(|s| !s.is_empty())
        .filter_map(|segment| {
            let Some(name) = segment.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
                return Some(segment.to_owned());
            };
            // An optional catch-all takes nothing.
            if name.starts_with("[...") {
                return None;
            }
            let name = name.trim_start_matches("...");
            let id = name == "id"
                || name.ends_with("Id")
                || name.ends_with("_id")
                || name.ends_with("ID")
                || name == "uuid";
            Some(if id {
                "00000000-0000-4000-8000-000000000001".to_owned()
            } else {
                "demo".to_owned()
            })
        })
        .collect();
    format!("/{}", parts.join("/"))
}

/// The most screens a path is followed through to reach a workflow's.
const LONGEST_WAY: usize = 6;

/// The front ends of the project, read from the map and the files'
/// texts.
pub(crate) fn read(map: &CodeMap, sources: &[(usize, String)]) -> Vec<FrontEnd> {
    let text_of: HashMap<usize, &str> = sources.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let mut manifests: Vec<(usize, &str)> = sources
        .iter()
        .filter(|(n, _)| {
            let path = &map.nodes[*n].path;
            (path == "package.json" || path.ends_with("/package.json"))
                && !crate::map::is_test(Path::new(path))
                && worth(Path::new(path)) == 0
                && !in_aside(path)
        })
        .map(|(n, t)| (*n, t.as_str()))
        .collect();
    manifests.sort_by_key(|(n, _)| map.nodes[*n].path.clone());
    let calls = calls_in(map, &text_of);
    let mut fronts = Vec::new();
    for (node, text) in manifests {
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        let path = &map.nodes[node].path;
        let folder = path
            .strip_suffix("package.json")
            .unwrap_or_default()
            .trim_end_matches('/');
        if let Some(front) = front_end(map, &text_of, &calls, folder, &manifest) {
            fronts.push(front);
        }
    }
    fronts
}

/// Whether a path is in a folder of examples or documents, whose apps are
/// not the project's.
fn in_aside(path: &str) -> bool {
    path.split('/')
        .any(|f| f.starts_with("example") || f == "docs" || f == "fixtures" || f == "node_modules")
}

/// The front end of the package in `folder`, if its router serves screens.
fn front_end(
    map: &CodeMap,
    text_of: &HashMap<usize, &str>,
    calls: &CallsIn,
    folder: &str,
    manifest: &serde_json::Value,
) -> Option<FrontEnd> {
    let depends = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|key| manifest[key].get(name).is_some())
    };
    let within = |path: &str| folder.is_empty() || path.starts_with(&format!("{folder}/"));
    // The package's files, but those of a package inside it.
    let nested: Vec<String> = map
        .nodes
        .iter()
        .filter(|n| n.path.ends_with("/package.json") && within(&n.path))
        .filter_map(|n| n.path.strip_suffix("/package.json").map(str::to_owned))
        .filter(|f| f != folder)
        .collect();
    let files: Vec<usize> = map
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            matches!(
                n.kind,
                NodeKind::File {
                    language: Language::TypeScript | Language::JavaScript,
                    ..
                }
            ) && within(&n.path)
                && !nested.iter().any(|f| n.path.starts_with(&format!("{f}/")))
        })
        .map(|(i, _)| i)
        .collect();
    let index: HashMap<&str, usize> = map
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.path.as_str(), i))
        .collect();

    let (router, mut screens) = if depends("next") {
        let app = next_app(map, &index, folder, &files);
        if app.is_empty() {
            (Router::NextPages, next_pages(map, folder, &files))
        } else {
            (Router::NextApp, app)
        }
    } else if depends("@tanstack/react-router") {
        (Router::TanStack, tanstack(map, text_of, &files))
    } else if depends("react-router") || depends("react-router-dom") {
        (
            Router::ReactRouter,
            react_router(map, text_of, &imports_of(map), &files),
        )
    } else {
        return None;
    };
    if screens.is_empty() {
        return None;
    }
    screens.sort_by(|a, b| a.route.cmp(&b.route).then(a.file.cmp(&b.file)));
    screens.dedup_by(|a, b| a.route == b.route);

    let dev = manifest["scripts"]["dev"]
        .as_str()
        .or_else(|| manifest["scripts"]["start"].as_str())
        .map(str::to_owned);
    let framework = match dev.as_deref() {
        Some(script) if script.contains("next") => Framework::Next,
        Some(script) if script.contains("vite") => Framework::Vite,
        _ if depends("next") => Framework::Next,
        _ if depends("vite") => Framework::Vite,
        _ => Framework::Other,
    };
    let name = manifest["name"]
        .as_str()
        .map(|n| n.rsplit('/').next().unwrap_or(n).to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            folder
                .rsplit('/')
                .next()
                .filter(|n| !n.is_empty())
                .unwrap_or("app")
                .to_owned()
        });

    let mut front = FrontEnd {
        folder: folder.to_owned(),
        name,
        router,
        framework,
        runner: runner(&index, folder),
        dev,
        api_env: api_env(map, text_of, &files),
        cookies: cookies(map, text_of, folder),
        screens,
        workflows: Vec::new(),
    };
    connect(map, text_of, calls, &index, &files, &mut front);
    front.workflows = workflows(map, &front);
    Some(front)
}

/// The tool that runs a package's scripts, by the lock file beside it or
/// in a folder above.
fn runner(index: &HashMap<&str, usize>, folder: &str) -> String {
    let mut dir = folder;
    loop {
        let at = |name: &str| {
            let path = if dir.is_empty() {
                name.to_owned()
            } else {
                format!("{dir}/{name}")
            };
            index.contains_key(path.as_str())
        };
        for (lock, tool) in [
            ("pnpm-lock.yaml", "pnpm"),
            ("yarn.lock", "yarn"),
            ("bun.lock", "bun"),
            ("bun.lockb", "bun"),
            ("package-lock.json", "npm"),
        ] {
            if at(lock) {
                return tool.to_owned();
            }
        }
        if dir.is_empty() {
            return "npm".to_owned();
        }
        dir = crate::imports::parent(dir);
    }
}

/// The variable of the environment a front end reads its API's address
/// from: the one its code reads most among those that name an API and an
/// address.
fn api_env(map: &CodeMap, text_of: &HashMap<usize, &str>, files: &[usize]) -> Option<String> {
    static READ: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?:process\.env|import\.meta\.env)\.([A-Z][A-Z0-9_]*)").ok());
    let read = READ.as_ref()?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for file in files {
        if crate::map::is_test(Path::new(&map.nodes[*file].path)) {
            continue;
        }
        let Some(text) = text_of.get(file) else {
            continue;
        };
        for c in read.captures_iter(text) {
            let name = &c[1];
            let api = name.contains("API") || name.contains("BACKEND");
            let address = ["URL", "BASE", "HOST", "ENDPOINT", "SERVER", "ORIGIN"]
                .iter()
                .any(|w| name.contains(w));
            if api && address {
                *counts.entry(name.to_owned()).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(name, _)| name)
}

/// The cookies a Next.js middleware reads: those it names, and the names
/// its constants of cookies hold.
fn cookies(map: &CodeMap, text_of: &HashMap<usize, &str>, folder: &str) -> Vec<String> {
    static NAMED: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r#"cookies\??\.(?:get|has)\(\s*["']([^"']+)["']"#).ok());
    static CONSTANT: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r#"(?m)^\s*(?:export\s+)?const\s+\w*COOKIE\w*\s*=[^;]{0,200}?["']([\w.-]+)["']"#)
            .ok()
    });
    let mut found = Vec::new();
    for base in ["", "src/"] {
        for name in ["middleware", "proxy"] {
            for extension in ["ts", "js"] {
                let path = if folder.is_empty() {
                    format!("{base}{name}.{extension}")
                } else {
                    format!("{folder}/{base}{name}.{extension}")
                };
                let Some(text) = map.find(&path).and_then(|n| text_of.get(&n)) else {
                    continue;
                };
                for re in [&NAMED, &CONSTANT] {
                    let Some(re) = re.as_ref() else {
                        continue;
                    };
                    for c in re.captures_iter(text) {
                        if !found.contains(&c[1].to_owned()) {
                            found.push(c[1].to_owned());
                        }
                    }
                }
            }
        }
    }
    found
}

/// The screens of a Next.js `app` folder: each `page` file, its route the
/// folders above it but route groups (`(main)`), slots (`@modal`) and
/// private folders (`_lib`), with the layouts of those folders.
fn next_app(
    map: &CodeMap,
    index: &HashMap<&str, usize>,
    folder: &str,
    files: &[usize],
) -> Vec<Screen> {
    let prefix = |dir: &str| {
        if folder.is_empty() {
            format!("{dir}/")
        } else {
            format!("{folder}/{dir}/")
        }
    };
    let mut screens = Vec::new();
    for root in [prefix("src/app"), prefix("app")] {
        for &file in files {
            let path = &map.nodes[file].path;
            let Some(rest) = path.strip_prefix(&root) else {
                continue;
            };
            let (dir, name) = rest.rsplit_once('/').unwrap_or(("", rest));
            let stem = name.split('.').next().unwrap_or(name);
            if stem != "page" {
                continue;
            }
            let Some(route) = next_app_route(dir) else {
                continue;
            };
            // The layouts of the folders down to the page's, outermost
            // first.
            let mut layouts = Vec::new();
            let mut at = root.trim_end_matches('/').to_owned();
            let parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
            for depth in 0..=parts.len() {
                if depth > 0 {
                    at = format!("{at}/{}", parts[depth - 1]);
                }
                for kind in ["layout", "template"] {
                    for extension in ["tsx", "jsx", "ts", "js"] {
                        let layout = format!("{at}/{kind}.{extension}");
                        if index.contains_key(layout.as_str()) {
                            layouts.push(layout);
                        }
                    }
                }
            }
            screens.push(Screen {
                route,
                file: path.clone(),
                layouts,
                ..Screen::default()
            });
        }
        if !screens.is_empty() {
            break;
        }
    }
    screens
}

/// A route of a Next.js `app` folder from the folders of its page, or
/// `None` for a page no address reaches: one in a private folder, or one
/// that intercepts another's route.
fn next_app_route(dir: &str) -> Option<String> {
    let mut kept = Vec::new();
    for segment in dir.split('/').filter(|s| !s.is_empty()) {
        if segment.starts_with("(.") || segment.starts_with('_') {
            return None;
        }
        if (segment.starts_with('(') && segment.ends_with(')')) || segment.starts_with('@') {
            continue;
        }
        kept.push(segment);
    }
    Some(format!("/{}", kept.join("/")))
}

/// The screens of a Next.js `pages` folder: each file but the app's own
/// (`_app`, `_document`) and the API's routes.
fn next_pages(map: &CodeMap, folder: &str, files: &[usize]) -> Vec<Screen> {
    let mut screens = Vec::new();
    for dir in ["src/pages", "pages"] {
        let root = if folder.is_empty() {
            format!("{dir}/")
        } else {
            format!("{folder}/{dir}/")
        };
        for &file in files {
            let path = &map.nodes[file].path;
            let Some(rest) = path.strip_prefix(&root) else {
                continue;
            };
            let stem = rest.rsplit_once('.').map_or(rest, |(s, _)| s);
            if stem.starts_with("api/")
                || stem == "api"
                || stem.split('/').any(|s| s.starts_with('_'))
            {
                continue;
            }
            let route = stem
                .split('/')
                .filter(|s| *s != "index")
                .collect::<Vec<_>>()
                .join("/");
            screens.push(Screen {
                route: format!("/{route}"),
                file: path.clone(),
                ..Screen::default()
            });
        }
        if !screens.is_empty() {
            break;
        }
    }
    screens
}

/// The screens of TanStack Router's file routes: each
/// `createFileRoute('/posts/$id')`, its parameters in brackets, its
/// layouts without a path (`_auth`) left out.
fn tanstack(map: &CodeMap, text_of: &HashMap<usize, &str>, files: &[usize]) -> Vec<Screen> {
    static ROUTE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r#"createFileRoute\(\s*["'`]([^"'`]+)["'`]\s*\)"#).ok());
    let Some(re) = ROUTE.as_ref() else {
        return Vec::new();
    };
    let mut screens = Vec::new();
    for file in files {
        let Some(text) = text_of.get(file) else {
            continue;
        };
        for c in re.captures_iter(text) {
            let parts: Vec<String> = c[1]
                .split('/')
                .filter(|s| !s.is_empty() && !s.starts_with('_'))
                .map(|s| match s.strip_prefix('$') {
                    Some("") => "[...splat]".to_owned(),
                    Some(name) => format!("[{name}]"),
                    None => s.to_owned(),
                })
                .collect();
            screens.push(Screen {
                route: format!("/{}", parts.join("/")),
                file: map.nodes[*file].path.clone(),
                ..Screen::default()
            });
        }
    }
    screens
}

/// The screens React Router's routes declare, as objects (`{ path:
/// 'settings', element: <Settings /> }`) or as `<Route>` elements, a
/// relative path under the route that holds it; each rendered by the file
/// its element's component is imported from, else the file that declares
/// it.
fn react_router(
    map: &CodeMap,
    text_of: &HashMap<usize, &str>,
    imports: &Imports,
    files: &[usize],
) -> Vec<Screen> {
    static OBJECT: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r#"\bpath\s*:\s*["'`]([^"'`]*)["'`]"#).ok());
    static ELEMENT: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r#"<Route\b[^>]*?\bpath\s*=\s*\{?\s*["'`]([^"'`]*)["'`]"#).ok()
    });
    static COMPONENT: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r"(?:element\s*[:=]\s*\{?\s*<\s*([A-Z]\w*)|Component\s*[:=]\s*\{?\s*([A-Z]\w*))")
            .ok()
    });
    let (Some(object), Some(element), Some(component)) =
        (OBJECT.as_ref(), ELEMENT.as_ref(), COMPONENT.as_ref())
    else {
        return Vec::new();
    };
    let mut screens = Vec::new();
    for &file in files {
        let Some(text) = text_of.get(&file) else {
            continue;
        };
        if !text.contains("element") && !text.contains("Component") {
            continue;
        }
        // Each route's path, where it is, and the span it holds.
        let mut routes: Vec<(String, usize, usize)> = Vec::new();
        for c in object.captures_iter(text) {
            let Some(m) = c.get(0) else { continue };
            let (start, end) = enclosing_object(text, m.start());
            routes.push((c[1].to_owned(), start, end));
        }
        for c in element.captures_iter(text) {
            let Some(m) = c.get(0) else { continue };
            routes.push((
                c[1].to_owned(),
                m.start(),
                route_element_end(text, m.start()),
            ));
        }
        if routes.is_empty() {
            continue;
        }
        let imported = imported_names(text);
        for (i, (path, start, end)) in routes.iter().enumerate() {
            // A relative path is under the innermost route that holds it.
            let mut full = path.clone();
            let mut inner = (*start, *end);
            while !full.starts_with('/') {
                let parent = routes
                    .iter()
                    .enumerate()
                    .filter(|(j, (_, s, e))| {
                        *j != i && *s < inner.0 && *e >= inner.1 && (*s, *e) != inner
                    })
                    .max_by_key(|(_, (_, s, _))| *s);
                match parent {
                    Some((_, (p, s, e))) => {
                        full = format!("{}/{full}", p.trim_end_matches('/'));
                        inner = (*s, *e);
                    }
                    None => full = format!("/{full}"),
                }
            }
            let own = &text[*start..(*end).min(text.len())];
            let name = component
                .captures(own)
                .and_then(|c| c.get(1).or_else(|| c.get(2)))
                .map(|m| m.as_str().to_owned());
            let rendered = name
                .and_then(|n| imported.get(&n).cloned())
                .and_then(|spec| imported_file(map, imports, file, &spec))
                .unwrap_or(file);
            let route = full.replace("//", "/");
            let route: Vec<String> = route
                .split('/')
                .filter(|s| !s.is_empty() && *s != "*")
                .map(|s| match s.strip_prefix(':') {
                    Some(name) => format!("[{}]", name.trim_end_matches('?')),
                    None => s.to_owned(),
                })
                .collect();
            screens.push(Screen {
                route: format!("/{}", route.join("/")),
                file: map.nodes[rendered].path.clone(),
                ..Screen::default()
            });
        }
    }
    screens
}

/// The span of the object literal around `at`: from its `{` to its `}`.
fn enclosing_object(text: &str, at: usize) -> (usize, usize) {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut start = 0;
    for i in (0..at).rev() {
        match bytes[i] {
            b'}' => depth += 1,
            b'{' if depth == 0 => {
                start = i;
                break;
            }
            b'{' => depth -= 1,
            _ => {}
        }
    }
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (start, i + 1);
                }
            }
            _ => {}
        }
    }
    (start, text.len())
}

/// Where the `<Route>` element starting at `at` ends: its closing tag, or
/// the end of the tag when it closes itself.
fn route_element_end(text: &str, at: usize) -> usize {
    let tag_end = text[at..].find('>').map_or(text.len(), |i| at + i + 1);
    if text[..tag_end].ends_with("/>") {
        return tag_end;
    }
    let mut depth = 1;
    let mut i = tag_end;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("</Route") {
            depth -= 1;
            if depth == 0 {
                return i + rest.find('>').map_or(rest.len(), |j| j + 1);
            }
            i += 7;
        } else if rest.starts_with("<Route") {
            let end = rest.find('>').map_or(rest.len(), |j| j + 1);
            if !rest[..end].ends_with("/>") {
                depth += 1;
            }
            i += end;
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    text.len()
}

/// The names a file imports, each with the module it is from: default and
/// named imports, and `lazy(() => import('./x'))` given a name.
fn imported_names(text: &str) -> HashMap<String, String> {
    static IMPORT: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(
            r#"import\s+(?:type\s+)?([\w$]+)?\s*,?\s*(?:\{([^}]*)\})?\s*from\s*["']([^"']+)["']"#,
        )
        .ok()
    });
    static LAZY: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r#"const\s+([A-Z]\w*)\s*=\s*(?:React\.)?lazy\(\s*\(\)\s*=>\s*import\(\s*["']([^"']+)["']"#)
            .ok()
    });
    let mut names = HashMap::new();
    if let Some(re) = IMPORT.as_ref() {
        for c in re.captures_iter(text) {
            let from = c[3].to_owned();
            if let Some(default) = c.get(1) {
                names.insert(default.as_str().to_owned(), from.clone());
            }
            for name in c.get(2).map_or("", |m| m.as_str()).split(',') {
                let mut words = name.split_whitespace();
                let (Some(first), second, third) = (words.next(), words.next(), words.next())
                else {
                    continue;
                };
                let local = if second == Some("as") {
                    third.unwrap_or(first)
                } else {
                    first
                };
                names.insert(local.to_owned(), from.clone());
            }
        }
    }
    if let Some(re) = LAZY.as_ref() {
        for c in re.captures_iter(text) {
            names.insert(c[1].to_owned(), c[2].to_owned());
        }
    }
    names
}

/// What each file imports, by its place in the map.
type Imports = HashMap<usize, Vec<usize>>;

/// The file `from` imports as `spec`: among those it imports, the one
/// whose path ends as the module's does.
fn imported_file(map: &CodeMap, imports: &Imports, from: usize, spec: &str) -> Option<usize> {
    let last = spec.trim_end_matches('/').rsplit('/').next()?;
    imports
        .get(&from)
        .into_iter()
        .flatten()
        .copied()
        .find(|&to| {
            let path = &map.nodes[to].path;
            let stem = path.rsplit_once('.').map_or(path.as_str(), |(s, _)| s);
            stem.ends_with(&format!("/{last}")) || stem.ends_with(&format!("/{last}/index"))
        })
}

/// What each file of the map imports.
fn imports_of(map: &CodeMap) -> Imports {
    let mut imports: Imports = HashMap::new();
    for edge in map.edges.iter().filter(|e| e.kind == EdgeKind::Imports) {
        imports.entry(edge.from).or_default().push(edge.to);
    }
    imports
}

/// Fills in each screen's links and calls, through the files it imports.
/// A file reached through an import of some of its names calls only what
/// those names call: a page that imports `useProducts` from a file of
/// every product's hooks does not create one.
fn connect(
    map: &CodeMap,
    text_of: &HashMap<usize, &str>,
    calls_in: &CallsIn,
    index: &HashMap<&str, usize>,
    files: &[usize],
    front: &mut FrontEnd,
) {
    let in_front: HashSet<usize> = files.iter().copied().collect();
    let imports = imports_of(map);
    let mut importers: Imports = HashMap::new();
    let mut imported_by: HashMap<usize, usize> = HashMap::new();
    for edge in map.edges.iter().filter(|e| e.kind == EdgeKind::Imports) {
        importers.entry(edge.to).or_default().push(edge.from);
        if in_front.contains(&edge.from) {
            *imported_by.entry(edge.to).or_default() += 1;
        }
    }
    // A file imported nearly everywhere (a provider, a barrel of every
    // component) is gone through by no screen: through it, every screen
    // would reach everything.
    let everywhere = (files.len() / 4).max(25);

    // The names a file imports from another, `None` for all of it.
    let mut names_from: HashMap<(usize, usize), Option<HashSet<String>>> = HashMap::new();
    for &file in files {
        let Some(text) = text_of.get(&file) else {
            continue;
        };
        for (spec, names) in named_imports(text) {
            let Some(to) = imported_file(map, &imports, file, &spec) else {
                continue;
            };
            let entry = names_from
                .entry((file, to))
                .or_insert_with(|| Some(HashSet::new()));
            match (entry.as_mut(), names) {
                (Some(kept), Some(names)) => kept.extend(names),
                _ => *entry = None,
            }
        }
    }

    let routes: Vec<Vec<&str>> = front
        .screens
        .iter()
        .map(|s| s.route.split('/').filter(|p| !p.is_empty()).collect())
        .collect();
    let mut links_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for &file in files {
        if let Some(text) = text_of.get(&file) {
            let links = written_links(text, &routes);
            if !links.is_empty() {
                links_of.insert(file, links);
            }
        }
    }

    for (i, screen) in front.screens.iter_mut().enumerate() {
        let starts: Vec<usize> = std::iter::once(&screen.file)
            .chain(&screen.layouts)
            .filter_map(|p| index.get(p.as_str()).copied())
            .collect();
        let mut depth_of: HashMap<usize, usize> = HashMap::new();
        let mut queue: VecDeque<usize> = VecDeque::new();
        for start in starts {
            depth_of.insert(start, 0);
            queue.push_back(start);
        }
        while let Some(file) = queue.pop_front() {
            let depth = depth_of[&file];
            if depth > 0 && imported_by.get(&file).copied().unwrap_or(0) > everywhere {
                continue;
            }
            for &next in imports.get(&file).into_iter().flatten() {
                if let std::collections::hash_map::Entry::Vacant(e) = depth_of.entry(next) {
                    e.insert(depth + 1);
                    queue.push_back(next);
                }
            }
        }
        let mut calls: BTreeMap<usize, usize> = BTreeMap::new();
        let mut links: Vec<usize> = Vec::new();
        for (&file, &depth) in &depth_of {
            if let Some(made) = calls_in.get(&file) {
                // The names it is reached by, from the files it is reached
                // from; all of them from a screen's own file.
                let mut wanted: Option<HashSet<&str>> = (depth > 0).then(HashSet::new);
                for importer in importers.get(&file).into_iter().flatten() {
                    if !depth_of.contains_key(importer) {
                        continue;
                    }
                    match (&mut wanted, names_from.get(&(*importer, file))) {
                        (Some(kept), Some(Some(names))) => {
                            kept.extend(names.iter().map(String::as_str));
                        }
                        _ => wanted = None,
                    }
                }
                for (operation, export) in made {
                    let reached = match (&wanted, export) {
                        (Some(names), Some(export)) => names.contains(export.as_str()),
                        _ => true,
                    };
                    if reached {
                        let entry = calls.entry(*operation).or_insert(depth);
                        *entry = (*entry).min(depth);
                    }
                }
            }
            for &link in links_of.get(&file).into_iter().flatten() {
                if link != i && !links.contains(&link) {
                    links.push(link);
                }
            }
        }
        links.sort_unstable();
        let mut calls: Vec<Call> = calls
            .into_iter()
            .map(|(operation, depth)| Call {
                method: map.operations[operation].method.clone(),
                path: map.operations[operation].path.clone(),
                depth,
            })
            .collect();
        calls.sort_by(|a, b| (a.depth, &a.path, &a.method).cmp(&(b.depth, &b.path, &b.method)));
        screen.links = links;
        screen.calls = calls;
    }
}

/// Each file's calls of the API: the operation, by its place in the map's,
/// and the export of the file that makes it, if one does.
type CallsIn = HashMap<usize, Vec<(usize, Option<String>)>>;

/// The calls of every file of the map that calls the API.
fn calls_in(map: &CodeMap, text_of: &HashMap<usize, &str>) -> CallsIn {
    let mut calls: CallsIn = HashMap::new();
    for (i, operation) in map.operations.iter().enumerate() {
        if operation.callers.is_empty() {
            continue;
        }
        let written = written_pattern(&operation.path);
        for &caller in &operation.callers {
            let export = text_of
                .get(&caller)
                .and_then(|text| export_calling(text, written.as_ref(), operation));
            calls.entry(caller).or_default().push((i, export));
        }
    }
    calls
}

/// How a client writes an operation's path: its parameters put in any
/// way, its version or `api` part perhaps left to a base.
fn written_pattern(path: &str) -> Option<Regex> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let skip = parts
        .iter()
        .take_while(|p| is_version(p) || **p == "api")
        .count();
    let pattern: Vec<String> = parts[skip..]
        .iter()
        .map(|p| {
            if p.starts_with('{') {
                r"[^/'`]+".to_owned()
            } else {
                regex::escape(p)
            }
        })
        .collect();
    if pattern.is_empty() {
        return None;
    }
    Regex::new(&format!(r"/{}/?['`?]", pattern.join("/"))).ok()
}

/// The modules a file imports from, each with the names it imports, or
/// `None` when it takes the module whole: by default, as a namespace, or
/// for what it does.
fn named_imports(text: &str) -> Vec<(String, Option<Vec<String>>)> {
    static IMPORT: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r#"import\s+(?:type\s+)?([\w$]+)?\s*,?\s*(\*\s*as\s+[\w$]+)?\s*(?:\{([^}]*)\})?\s*from\s*["']([^"']+)["']"#)
            .ok()
    });
    let Some(re) = IMPORT.as_ref() else {
        return Vec::new();
    };
    re.captures_iter(text)
        .map(|c| {
            let whole = c.get(1).is_some() || c.get(2).is_some();
            let names: Vec<String> = c
                .get(3)
                .map_or("", |m| m.as_str())
                .split(',')
                .filter_map(|name| {
                    let name = name.trim().trim_start_matches("type ").trim();
                    name.split_whitespace().next().map(str::to_owned)
                })
                .collect();
            (c[4].to_owned(), (!whole).then_some(names))
        })
        .collect()
}

/// The export of a file that makes a call of `operation`: the last one
/// declared before where its path, or a name a client gives it, is first
/// written; `None` when it is made outside any.
fn export_calling(
    text: &str,
    written: Option<&Regex>,
    operation: &crate::api::Operation,
) -> Option<String> {
    static EXPORT: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r"(?m)^export\s+(?:default\s+)?(?:async\s+)?(?:const|let|var|function\*?|class)\s+([\w$]+)")
            .ok()
    });
    let exports: Vec<(usize, &str)> = EXPORT
        .as_ref()?
        .captures_iter(text)
        .filter_map(|c| Some((c.get(0)?.start(), c.get(1)?.as_str())))
        .collect();
    if exports.is_empty() {
        return None;
    }
    let at = written
        .and_then(|re| re.find(text))
        .map(|m| m.start())
        .or_else(|| {
            let pascal = crate::api::pascal(&operation.id);
            let camel = crate::api::lower_first(&pascal);
            [pascal.as_str(), camel.as_str()]
                .iter()
                .filter(|n| n.len() > 3)
                .find_map(|n| text.find(n))
        })?;
    exports
        .iter()
        .rev()
        .find(|(start, _)| *start <= at)
        .map(|(_, name)| (*name).to_owned())
}

/// The screens a file's text links to: each path it writes that a route
/// matches, the best match when several do. A part put in
/// (`${org.slug}`) matches a parameter only.
fn written_links(text: &str, routes: &[Vec<&str>]) -> Vec<usize> {
    static PATH: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r#"["'](/[^"'\s]*)["']|`(/(?:[^`$\s]|\$\{[^}`]*\}|\$)*)`"#).ok()
    });
    static PUT_IN: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\$\{[^}]*\}").ok());
    let (Some(re), Some(put_in)) = (PATH.as_ref(), PUT_IN.as_ref()) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for c in re.captures_iter(text) {
        let Some(written) = c.get(1).or_else(|| c.get(2)) else {
            continue;
        };
        let written = written.as_str();
        if written.starts_with("//") {
            continue;
        }
        // What is put in may hold a `?` of its own (`${org?.slug}`): it is
        // a part, not where the query starts.
        let written = put_in.replace_all(written, "\u{1}");
        let written = written.split(['?', '#']).next().unwrap_or_default();
        let parts: Vec<String> = written
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.contains('\u{1}') {
                    "*".to_owned()
                } else {
                    s.to_owned()
                }
            })
            .collect();
        // A file, not a screen.
        if parts.last().is_some_and(|p| p.contains('.') && p != "*") {
            continue;
        }
        let best = routes
            .iter()
            .enumerate()
            .filter_map(|(i, route)| route_score(route, &parts).map(|score| (score, i)))
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        if let Some((_, i)) = best
            && !found.contains(&i)
        {
            found.push(i);
        }
    }
    found
}

/// How well a written path matches a route: the parts it matches by name,
/// or `None` when it does not match.
fn route_score(route: &[&str], written: &[String]) -> Option<usize> {
    let mut score = 0;
    let mut r = 0;
    let mut w = 0;
    while r < route.len() {
        let part = route[r];
        if let Some(name) = part.strip_prefix('[').and_then(|p| p.strip_suffix(']')) {
            if name.starts_with("[...") {
                // An optional catch-all takes the rest, or nothing.
                return Some(score);
            }
            if name.starts_with("...") {
                return (w < written.len()).then_some(score);
            }
            if w >= written.len() {
                return None;
            }
            r += 1;
            w += 1;
            continue;
        }
        if w >= written.len() || written[w] != part {
            return None;
        }
        score += 1;
        r += 1;
        w += 1;
    }
    (w == written.len() && (score > 0 || route.is_empty())).then_some(score)
}

/// What a person does on a front end: each action of the API its screens
/// call, at the screen that calls it most directly, with the shortest way
/// there from a screen above it.
fn workflows(map: &CodeMap, front: &FrontEnd) -> Vec<Workflow> {
    let screens = &front.screens;
    // Each action at the screen whose route names most of what it acts on
    // (`/products/benefits` for `/v1/benefits/{id}`), then the one that
    // calls it nearest, then the shortest route.
    let mut best: BTreeMap<(String, String), (usize, usize)> = BTreeMap::new();
    let mut reached: HashMap<(String, String), usize> = HashMap::new();
    let rank = |j: usize, depth: usize, path: &str| {
        (
            std::cmp::Reverse(named_in_route(&screens[j].route, path)),
            depth,
            screens[j].route.matches('/').count(),
        )
    };
    for (i, screen) in screens.iter().enumerate() {
        for call in &screen.calls {
            if !matches!(call.method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
                continue;
            }
            let key = (call.method.clone(), call.path.clone());
            *reached.entry(key.clone()).or_default() += 1;
            match best.get(&key) {
                Some(&(j, depth))
                    if rank(j, depth, &call.path) <= rank(i, call.depth, &call.path) => {}
                _ => {
                    best.insert(key, (i, call.depth));
                }
            }
        }
    }
    let mut flows = Vec::new();
    for ((method, path), (screen, _)) in best {
        // An action nearly every screen reaches is the frame's, not a
        // person's: an event sent from every page.
        if screens.len() >= 3 && reached[&(method.clone(), path.clone())] * 5 > screens.len() * 4 {
            continue;
        }
        let operation = map
            .operations
            .iter()
            .find(|o| o.method == method && o.path == path);
        let name = operation
            .map(|o| o.summary.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{method} {path}"));
        let group = operation
            .and_then(|o| o.tags.first().cloned())
            .unwrap_or_else(|| {
                path.split('/')
                    .filter(|s| !s.is_empty())
                    .find(|s| !is_version(s) && *s != "api")
                    .unwrap_or("api")
                    .to_owned()
            });
        flows.push(Workflow {
            name,
            group,
            method,
            path,
            steps: way_to(screens, screen),
        });
    }
    flows.sort_by(|a, b| (&a.group, &a.name).cmp(&(&b.group, &b.name)));
    flows
}

/// How many of the names an operation's path acts on a route names too,
/// one or many alike: `checkout-links` in `/products/checkout-links` for
/// `/v1/checkout-links/{id}`.
fn named_in_route(route: &str, path: &str) -> usize {
    let stem = |part: &str| part.trim_end_matches('s').replace(['-', '_'], "");
    let named: HashSet<String> = route
        .split('/')
        .filter(|p| !p.is_empty() && !p.starts_with('['))
        .map(stem)
        .collect();
    path.split('/')
        .filter(|p| !p.is_empty() && !p.starts_with('{') && !is_version(p) && *p != "api")
        .filter(|p| named.contains(&stem(p)))
        .count()
}

/// Whether a part of a path is a version: `v1`, `v2`.
fn is_version(part: &str) -> bool {
    part.len() > 1 && part.starts_with('v') && part[1..].chars().all(|c| c.is_ascii_digit())
}

/// The shortest way to `target` through the screens' links, from the
/// highest screen above it that has one: the target alone without.
fn way_to(screens: &[Screen], target: usize) -> Vec<usize> {
    let parts = |i: usize| -> Vec<&str> {
        screens[i]
            .route
            .split('/')
            .filter(|p| !p.is_empty())
            .collect()
    };
    let own = parts(target);
    // Not from the root, which a public home holds, but for a screen just
    // under it.
    let mut above: Vec<usize> = (0..screens.len())
        .filter(|&i| {
            let theirs = parts(i);
            i != target
                && theirs.len() < own.len()
                && own.starts_with(&theirs)
                && (!theirs.is_empty() || own.len() == 1)
        })
        .collect();
    above.sort_by_key(|&i| parts(i).len());
    for start in above {
        let mut previous: HashMap<usize, usize> = HashMap::new();
        let mut queue = VecDeque::from([(start, 0)]);
        previous.insert(start, start);
        while let Some((at, steps)) = queue.pop_front() {
            if at == target {
                let mut way = vec![target];
                let mut back = target;
                while back != start {
                    back = previous[&back];
                    way.push(back);
                }
                way.reverse();
                return way;
            }
            if steps >= LONGEST_WAY {
                continue;
            }
            for &next in &screens[at].links {
                if let std::collections::hash_map::Entry::Vacant(e) = previous.entry(next) {
                    e.insert(at);
                    queue.push_back((next, steps + 1));
                }
            }
        }
    }
    vec![target]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::map;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A spec of a shop: its products listed, made and removed.
    fn shop_spec(root: &Path) {
        write(
            root,
            "server/openapi.json",
            r#"{"openapi": "3.1.0", "info": {"title": "Shop"}, "paths": {
                "/v1/products/": {"get": {"operationId": "list_products", "tags": ["products"]},
                                  "post": {"operationId": "create_product", "summary": "Create Product", "tags": ["products"]}},
                "/v1/products/{id}": {"delete": {"operationId": "delete_product", "summary": "Delete Product", "tags": ["products"]}},
                "/v1/users/me": {"get": {"operationId": "me"}},
                "/v1/events/": {"post": {"operationId": "track", "summary": "Track"}}}}"#,
        );
    }

    #[test]
    fn a_next_app_its_screens_links_calls_and_workflows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        shop_spec(root);
        write(root, "web/pnpm-lock.yaml", "lockfileVersion: 9\n");
        write(
            root,
            "web/package.json",
            r#"{"name": "@shop/web", "scripts": {"dev": "next dev --turbopack"}, "dependencies": {"next": "16", "react": "19"}}"#,
        );
        write(
            root,
            "web/src/middleware.ts",
            "const SESSION_COOKIE = process.env.SESSION || 'shop_session'\nexport function middleware(request) {\n  if (!request.cookies.has(SESSION_COOKIE)) return null\n  request.cookies.get('theme')\n}\n",
        );
        write(
            root,
            "web/src/api.ts",
            "export const api = createClient({ baseUrl: process.env.NEXT_PUBLIC_API_URL })\nexport const me = () => api.GET('/v1/users/me')\nexport const track = () => api.POST('/v1/events/')\n",
        );
        // The menu every screen of the dashboard shows, as data.
        write(
            root,
            "web/src/components/Menu.tsx",
            "import { track } from '../api'\nexport const items = (org) => [\n  { title: 'Home', link: `/dashboard/${org.slug}` },\n  { title: 'Products', link: `/dashboard/${org.slug}/products` },\n]\n",
        );
        write(
            root,
            "web/src/app/layout.tsx",
            "import { me } from '../api'\nexport default function Root({ children }) { return children }\n",
        );
        write(
            root,
            "web/src/app/(main)/dashboard/[organization]/layout.tsx",
            "import { items } from '../../../../components/Menu'\nexport default function Layout({ children }) { return children }\n",
        );
        write(
            root,
            "web/src/app/(main)/dashboard/[organization]/page.tsx",
            "import { track } from '../../../../api'\nexport default function Home() { return null }\n",
        );
        write(
            root,
            "web/src/app/(main)/dashboard/[organization]/products/page.tsx",
            "import { useProducts, useDeleteProduct } from '../../../../../hooks/products'\nexport default function Products({ org }) {\n  return <a href={`/dashboard/${org.slug}/products/new`}>New</a>\n}\n",
        );
        write(
            root,
            "web/src/app/(main)/dashboard/[organization]/products/new/page.tsx",
            "import { useCreateProduct } from '../../../../../../hooks/products'\nexport default function NewProduct() { return null }\n",
        );
        write(
            root,
            "web/src/app/(main)/dashboard/[organization]/products/_parts/page.tsx",
            "export default function Private() { return null }\n",
        );
        write(
            root,
            "web/src/app/(main)/dashboard/[organization]/@modal/(.)products/page.tsx",
            "export default function Intercepted() { return null }\n",
        );
        write(
            root,
            "web/src/hooks/products.ts",
            "import { api } from '../api'\nexport const useProducts = () => api.GET('/v1/products/')\nexport const useCreateProduct = () => api.POST('/v1/products/')\nexport const useDeleteProduct = (id) => api.DELETE('/v1/products/{id}', { params: { path: { id } } })\n",
        );
        let map = map(root, 100);
        assert_eq!(map.front_ends.len(), 1);
        let web = &map.front_ends[0];
        assert_eq!(
            (
                web.folder.as_str(),
                web.name.as_str(),
                web.router,
                web.framework
            ),
            ("web", "web", Router::NextApp, Framework::Next)
        );
        assert_eq!(web.runner, "pnpm");
        assert_eq!(web.dev.as_deref(), Some("next dev --turbopack"));
        assert_eq!(web.api_env.as_deref(), Some("NEXT_PUBLIC_API_URL"));
        assert_eq!(web.cookies, ["theme", "shop_session"]);

        let routes: Vec<&str> = web.screens.iter().map(|s| s.route.as_str()).collect();
        assert_eq!(
            routes,
            [
                "/dashboard/[organization]",
                "/dashboard/[organization]/products",
                "/dashboard/[organization]/products/new",
            ]
        );
        let home = &web.screens[0];
        assert_eq!(
            home.layouts,
            [
                "web/src/app/layout.tsx",
                "web/src/app/(main)/dashboard/[organization]/layout.tsx",
            ]
        );
        // The menu's links, from every screen under its layout.
        assert_eq!(home.links, [1]);
        assert_eq!(web.screens[1].links, [0, 2]);
        let calls = |i: usize| -> Vec<(String, usize)> {
            web.screens[i]
                .calls
                .iter()
                .map(|c| (format!("{} {}", c.method, c.path), c.depth))
                .collect()
        };
        assert!(calls(0).contains(&("GET /v1/users/me".to_owned(), 1)));
        assert!(calls(1).contains(&("GET /v1/products/".to_owned(), 1)));

        let flows: Vec<(&str, &str, Vec<&str>)> = web
            .workflows
            .iter()
            .map(|w| {
                (
                    w.group.as_str(),
                    w.name.as_str(),
                    w.steps
                        .iter()
                        .map(|&s| web.screens[s].route.as_str())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            flows,
            [
                (
                    "products",
                    "Create Product",
                    vec![
                        "/dashboard/[organization]",
                        "/dashboard/[organization]/products",
                        "/dashboard/[organization]/products/new",
                    ]
                ),
                (
                    "products",
                    "Delete Product",
                    vec![
                        "/dashboard/[organization]",
                        "/dashboard/[organization]/products",
                    ]
                ),
            ]
        );
    }

    #[test]
    fn react_router_routes_nested_and_tanstack_file_routes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "app/yarn.lock", "");
        write(
            root,
            "app/package.json",
            r#"{"name": "app", "scripts": {"dev": "vite"}, "dependencies": {"react-router-dom": "7"}}"#,
        );
        write(
            root,
            "app/src/routes.tsx",
            "import Settings from './pages/Settings'\nimport { Users } from './pages/Users'\nconst Billing = lazy(() => import('./pages/Billing'))\nexport const routes = [\n  {\n    path: '/settings',\n    element: <Settings />,\n    children: [\n      { path: 'billing', element: <Billing /> },\n      { path: 'users/:userId', element: <Users /> },\n    ],\n  },\n]\nexport const legacy = <Route path=\"/old\" element={<Settings />} />\n",
        );
        write(
            root,
            "app/src/pages/Settings.tsx",
            "export default function Settings() {}\n",
        );
        write(
            root,
            "app/src/pages/Users.tsx",
            "export function Users() {}\n",
        );
        write(
            root,
            "app/src/pages/Billing.tsx",
            "export default function Billing() {}\n",
        );
        write(
            root,
            "admin/package.json",
            r#"{"name": "admin", "dependencies": {"@tanstack/react-router": "1"}}"#,
        );
        write(
            root,
            "admin/src/routes/posts.$postId.tsx",
            "export const Route = createFileRoute('/_auth/posts/$postId')({})\n",
        );
        let map = map(root, 100);
        let found: Vec<String> = map
            .front_ends
            .iter()
            .map(|f| {
                let screens: Vec<String> = f
                    .screens
                    .iter()
                    .map(|s| format!("{} {}", s.route, s.file))
                    .collect();
                format!(
                    "{} {:?} {}: {}",
                    f.name,
                    f.router,
                    f.runner,
                    screens.join(", ")
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                "admin TanStack npm: /posts/[postId] admin/src/routes/posts.$postId.tsx",
                "app ReactRouter yarn: /old app/src/pages/Settings.tsx, /settings app/src/pages/Settings.tsx, /settings/billing app/src/pages/Billing.tsx, /settings/users/[userId] app/src/pages/Users.tsx",
            ]
        );
        assert_eq!(map.front_ends[1].framework, Framework::Vite);
    }

    #[test]
    fn written_paths_match_the_route_they_name_best() {
        let routes: Vec<Vec<&str>> = [
            "/",
            "/dashboard/[org]",
            "/dashboard/[org]/settings",
            "/docs/[...slug]",
            "/shop/[[...rest]]",
        ]
        .iter()
        .map(|r| r.split('/').filter(|p| !p.is_empty()).collect())
        .collect();
        let links = |text: &str| written_links(text, &routes);
        assert_eq!(links("href='/'"), [0]);
        assert_eq!(links("push(`/dashboard/${org}/settings?tab=1`)"), [2]);
        assert_eq!(links("link: `/dashboard/${org?.slug}/settings`"), [2]);
        assert_eq!(links("`/dashboard/${org ? org.slug : ''}/settings`"), [2]);
        assert_eq!(links("'/dashboard/acme'"), [1]);
        assert_eq!(links("'/docs/a/b'"), [3]);
        assert_eq!(links("'/docs'"), Vec::<usize>::new());
        assert_eq!(links("'/shop'"), [4]);
        assert_eq!(
            links("'/logo.png' '//cdn.example.com/x'"),
            Vec::<usize>::new()
        );
        assert_eq!(links("'/v1/products/'"), Vec::<usize>::new());
    }
}
