# universe

[![CI](https://github.com/ngodzik/universe/actions/workflows/ci.yml/badge.svg)](https://github.com/ngodzik/universe/actions/workflows/ci.yml) [![Security](https://github.com/ngodzik/universe/actions/workflows/security.yml/badge.svg)](https://github.com/ngodzik/universe/actions/workflows/security.yml)

**An experimental coding agent. It is not stable, it comes with no guarantee of any kind, and it changes all the time.**

## Read this first

This is a personal experiment, not a product, and it is not ready for production use.

- **It is not stable.** Commands, keys, views, settings and file formats change from one commit to the next, without notice and without a way back. Features appear, get rewritten or disappear: whatever is described here may be changed or removed at any time, as the terminal interface was.
- **It is barely tested in real use.** There are tests and CI, but few real sessions, on Linux mostly. macOS is only built and tested by CI; Windows is not supported.
- **It changes your files without asking.** Work in a git repository, commit before you start, and read every diff.
- **It spends money.** Requests go to a paid API, within a budget you set.

**If you want a coding agent to rely on, use [OpenCode](https://opencode.ai), [Pi](https://pi.dev) or [Claude Code](https://code.claude.com/docs/en/overview) instead.** This one overlaps with them on purpose: writing one is how I learn what makes them work, and where I try ideas of my own.

## The idea

Deterministic tools before models: compilers, tests, linters, language servers and git do what they can, and a model is called for the rest. Every request shows what it cost. The keys follow Vim's habits.

## What is there today

Roughly, and all of it experimental:

- a conversation with a model in a window, its changes shown as diffs, with Claude Code and Codex as possible sub-agents;
- a safety net for commands, which is not a sandbox;
- costs and context in view;
- views of the codebase, of where it runs, and of what a branch changed;
- tools of MCP servers, and rules kept outside a repository.

`/help` lists the commands and Ctrl-S every shortcut. They follow the code; this page does not.

## Usage

```bash
export REQUESTY_API_KEY=...         # without one, only Claude Code and Codex answer
cd your-project
universe                           # opens the window
```

Settings live in `~/.universe/config.toml`. Their names and meaning change often; the code's documentation of `Defaults` is the reference.

## Building

On Linux the window needs Wayland's client library: `sudo apt install libwayland-dev` on Debian and Ubuntu.

```bash
cargo install --path .              # builds the universe binary into ~/.cargo/bin
scripts/check.sh                    # what CI runs: formatting, lints, docs, tests, dependency audit
git config core.hooksPath .githooks # run those checks before every commit
```

Optional language servers: `npm install -g pyright typescript@5 typescript-language-server`, and `rustup component add rust-analyzer`.

## Data from others

None of this data ships with the program: each install fetches it for its own use and says where it comes from.

- **Model prices** for Claude Code's and Codex's calls come from [LiteLLM](https://github.com/BerriAI/litellm)'s public list, [`model_prices_and_context_window.json`](https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json), under the MIT licence. They are list prices: an estimate, not a bill.
- **Model scores** come from [Artificial Analysis](https://artificialanalysis.ai/)' free API, with your own key in `ARTIFICIAL_ANALYSIS_API_KEY`, under their terms, which ask that they be credited; the model picker does so wherever scores show.
- **Model lists, prices and context windows** for the provider's models come from the provider itself.

## License

MIT
