# universe coding rules

## Language

**All code, comments, doc-comments, variable names, error messages, and commit messages must be in English.**

## Prose and Markdown

**Never use em dashes or en dashes in Markdown files (README, CLAUDE.md, docs).** Use a comma, a colon, parentheses, or a new sentence instead.

**Never hard-wrap prose at a fixed column.** One paragraph is one line, one bullet is one line. Fenced blocks and table rows keep their own line structure.

## Principle

**Deterministic tools read the project, and no model is called.** Parsers, patterns, the LSP, git, `kustomize` and `helm` read what the project is; universe draws it. What cannot be read is left out rather than guessed or drawn wrong. universe never edits the project's files; the one thing it changes, when the person asks, is a linked repository fast-forwarded where nothing can be lost.

## Architecture

Separation of concerns is **enforced by the crate dependency graph**, not by convention. The compiler refuses inverted dependencies.

```
      crates/codemap  (no internal dependencies, no rendering)
      crates/tools    (no internal dependencies)
                ↑
      crates/ui       (tools, never a window library)
                ↑
      crates/gui      (tools + ui + codemap)
                ↑
      universe        (bin, depends on codemap, ui and gui)
```

| Crate             | Single responsibility                                                      |
|-------------------|----------------------------------------------------------------------------|
| `universe-codemap` | A project, or a system of several repositories read as one (declared in `~/.universe/systems/<name>.toml`, its own repositories read as code, each named by its folder, and those linked to it read for where it runs; found from any of them; a folder no system holds read alone, with the settings an older version kept for it), as a graph (folders, files, imports read by patterns, Python packages, TypeScript aliases, Dart packages and Go modules (each `go.mod`, a module's `vendor` folder) resolved, routes read from FastAPI's decorators when there is no spec and calls from the paths clients write, a front end's calls to its back end through an OpenAPI spec (the latest of an API's versions, each operation served by the function its id names, else by the route of its method and path)), its design one level at a time (parts, packages found by their manifests, what uses what, layers, loops), its files grouped by module, language, layer or role, its services (those of its Compose files, the folders that serve an API or MCP tools and the front ends that call one) and who reaches whom, through an OpenAPI spec, over MCP or by a URL naming a service, each link with where it was read, the tables its SQLAlchemy and SQLModel models define (columns, keys, indexes, foreign keys), where it runs as its deployment and infrastructure say (Helm charts, Kustomize trees and Flux objects, also declared in Terraform, Terragrunt units, AWS, Google Cloud and DigitalOcean with every setting's layers, each environment rendered by `kustomize` and `helm` with origins kept and secrets hidden, Terraform read with modules at their pinned ref, cloud identifiers matched to the blocks that configure them, an architecture model, every environment assembled at once (each release's images, its objects with what they say of themselves and how they act on each other, its settings told by the kind of layer that set them), a cache keyed by the repositories' state, and linked repositories fast-forwarded where nothing can be lost), and a force-directed layout that gathers groups into galaxies. Draws nothing |
| `universe-tools` | Deterministic tools for the file open: where a name is defined or used, as the project's language servers say or as `git grep` reads it, and what git says of each file. Writes nothing |
| `universe-ui`    | The interface's state: the view shown, the files, the file open read only, modes, keys, commands, how the window looks. Draws nothing |
| `universe-gui`   | Draws `universe-ui`'s state in a window, with egui on Bevy, the project's views in 2D and 3D, and runs its loop |
| `universe`       | CLI entry point: opens the window on the system that holds a folder, on several folders together, and declares a system of them |

In `universe-ui`, keys become actions only in `keymap.rs` (and, inside the file open, in `reader.rs`, which follows Vim), and `App` turns actions into state changes and returns an `Effect` for the loop instead of doing I/O, but reading the files it opens. It reads keys and clicks in its own types (`input.rs`) and colours as RGB (`style.rs`), never the window's: `universe-gui` translates its events into them, so that the state is tested without a window. Effects are carried out by `universe-ui`'s `Host`, never by the window itself; a lookup runs on a thread of its own. `universe-gui` draws the state without changing anything but what only the drawing knows (what was clicked, how far a pane scrolls), and drives the `Host` once a frame without ever waiting on it. The window sleeps between keys: nothing in it may ask egui to repaint unless something moves, since bevy_egui takes any request, even one for later, as one for now; only the universe and the views in space (the plan, the services and the database), while shown, draw without rest. A new key binding touches `keymap.rs` only; a new `:` command touches `command.rs` and `App::run_command`.

A new crate is added when its first real code lands, not before, and this graph is updated in the same commit.

## Idiomatic Rust patterns

1. **Traits** are the only abstraction mechanism. `impl Trait` for static dispatch, `dyn Trait` only for heterogeneous collections.
2. **Newtypes** for domain primitives. Never a bare `u64` or `f64` where a domain type makes sense.
3. **Typestate** for lifecycles where calling a method in the wrong state is a bug the compiler can catch.
4. **Builders** for complex configuration, validated at `build()`, returning `Result`.
5. **Iterators** over indexed loops.

## Non-negotiable rules

### Error handling
- `thiserror` in library crates: explicit error types
- `anyhow` in the binary
- Never `.unwrap()` or `.expect("...")` in library code
- An error keeps its source: wrap with `#[source]`, never stringify the cause away

### Secrets
- A type that holds a key or a token implements `Debug` by hand and redacts it
- What deployments hold as secrets is hidden when they are rendered, never shown
- universe's own folder, `~/.universe`, and all it holds are readable by their owner only: `Defaults::keep_home_private` shuts it when universe starts, so what writes there needs nothing more

### Unsafe
- `#![deny(unsafe_code)]` in every crate, the binary included

### Visibility
- `pub` only for items that are part of the crate's public API
- `pub(crate)` to share between modules within a crate
- No `pub use *`: re-exports are explicit

### Testing
- Unit tests `#[cfg(test)]` in the relevant module
- Integration tests in `crate/tests/`
- Formats read from files are tested against fixed texts, never against a live service

### Documentation
- Every `pub` item: `///` doc-comment required
- `# Examples` for non-trivial functions
- Comments explain WHY, not what the name already says

## Commands

```bash
scripts/check.sh    # everything CI runs, locally
```
