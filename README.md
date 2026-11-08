# universe

[![CI](https://github.com/ngodzik/universe/actions/workflows/ci.yml/badge.svg)](https://github.com/ngodzik/universe/actions/workflows/ci.yml) [![Security](https://github.com/ngodzik/universe/actions/workflows/security.yml/badge.svg)](https://github.com/ngodzik/universe/actions/workflows/security.yml)

**An experimental way to see a software project, or a system of several repositories, in space. It is not stable, it comes with no guarantee of any kind, and it changes all the time.**

## Read this first

This is a personal experiment, not a product, and it is not ready for production use.

- **It is not stable.** Commands, keys, views, settings and file formats change from one commit to the next, without notice and without a way back. Features appear, get rewritten or disappear: whatever is described here may be changed or removed at any time, as the terminal interface and the coding agent were.
- **It is barely tested in real use.** There are tests and CI, but few real sessions, on Linux mostly. macOS is only built and tested by CI; Windows is not supported.

## The idea

A large project is hard to hold in one's head, more so when it spans several repositories. universe reads it, its repositories together, with deterministic tools only and no model, and draws it: its files as stars gathered into galaxies, its design, its services, its API, its database and where it is deployed, in 3D where depth helps and flat where it reads better. It never edits the project's files.

## What is there today

Roughly, and all of it experimental:

- the project's files as a universe of stars, grouped by module, language, layer or role;
- its plan, its services and who reaches whom, its API routes with what serves and calls each, its database tables and their keys;
- its deployments, every environment at once;
- its files, opened read only, with their names followed to where they are defined.

`:help` and Ctrl-S list the keys. They follow the code; this page does not.

## Usage

```bash
universe                           # opens the window on the current folder, or the system that holds it
universe api web --save shop        # declares a system of several repositories, read as one
```

## Building

Each [release](https://github.com/ngodzik/universe/releases) carries the binary built for Linux (x86_64) and macOS (Apple Silicon); its notes say what it needs to run. To build it from the source instead:

On Linux the window needs Wayland's client library: `sudo apt install libwayland-dev` on Debian and Ubuntu.

```bash
cargo install --path .              # builds the universe binary into ~/.cargo/bin
scripts/check.sh                    # what CI runs: formatting, lints, docs, tests, dependency audit
git config core.hooksPath .githooks # run those checks before every commit
```

Optional language servers, to follow names in files: `npm install -g pyright typescript@5 typescript-language-server`, and `rustup component add rust-analyzer`.

## License

MIT
