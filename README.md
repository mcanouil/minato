# minato

Overview and sync of Git repositories across hosting providers.

`minato` shows what you have remotely, what you have cloned locally, and where the two have drifted apart.
It then offers safe, scriptable actions to close the gap: clone what is missing, fetch and report, and fast-forward clones and forks that are strictly behind.

GitHub is the only supported provider for now.

## Status

Early development.
The read-only commands below work; nothing yet clones, fetches, or updates anything.
Progress is tracked in the local [kata](https://github.com/kenn-io/kata) ledger (`kata list`).

## Configuration

Configuration is TOML at `~/.config/minato/minato.toml`, or wherever `MINATO_CONFIG` points.
Running `minato doctor` before it exists prints a sample to start from.

```toml
[providers.github]
users = ["your-username"]
orgs = []

[local]
roots = ["~/Projects"]
layout = "{repo}"
protocol = "ssh"
```

`roots` are searched for existing clones.
A clone is matched to a repository by its remote URL rather than by where it sits, so any directory structure works: one root covering categories such as `~/Projects/perso` and `~/Projects/work` is found without listing them.

`layout` only decides the name of a *new* clone beneath the directory it is placed in.
It defaults to a flat name, because where a repository belongs is a judgement its identity does not carry.
Use `minato clone --into <directory>` to say where.

A token is never stored here.
It is read from `MINATO_GITHUB_TOKEN`, then `GITHUB_TOKEN`, then the `gh` CLI, so it stays wherever you already keep it.

## Commands

Every command takes `--json` for scripts and agents, and `--refresh` to ignore cached data.

| Command | What it does |
| --- | --- |
| `minato list` | Every repository the provider reports, with stars, issues, pull requests, licence, and when it was last pushed. |
| `minato status` | How local clones stand against the provider: not cloned, in sync, ahead, behind, diverged, or local only, with the reason. |
| `minato clone` | Clones repositories that have no local copy. `--into <directory>` chooses where they land, defaulting to the first configured root. Skips any destination that already exists. |
| `minato fetch` | Fetches every clone. Updates remote-tracking refs only, so it never touches a working tree and is always safe to run. |
| `minato update` | Fast-forwards clones that are strictly behind and have no modified tracked files. Everything else is reported with the reason it was left alone. |
| `minato refresh` | Discards cached data so the next run asks the provider again. |
| `minato auth status` | Whether a token was found and where it came from, never the token itself. |
| `minato doctor` | Checks git, the token, configuration, roots, and the cache, reporting all of them rather than stopping at the first problem. |

`clone`, `fetch`, and `update` all take `--dry-run`, which reports what would happen and changes nothing.
One repository failing does not stop the others; every repository is reported and the process exits non-zero if any of them failed.

Nothing force-pushes, rebases, or discards a change.
An update is only ever a fast-forward, so if the situation has been misjudged, git refuses rather than improvising.

Provider responses are cached for fifteen minutes.
Cached output says how old it is, so stale data never passes for fresh, and it stays readable with no network at all.

## Documentation

The documentation website is a Quarto project under [`docs/`](docs), covering the design note now and the command reference as commands land.

```sh
quarto render docs
```

CI renders it on every pull request and uploads the result as an artifact.
Deployment to GitHub Pages is deferred until the repository is public, since Pages on a private repository requires a paid plan.

## Requirements

- Rust 1.85 or later (edition 2024).
- A `git` binary on `PATH`, which `minato` shells out to so that your existing SSH agent and credential helpers work unchanged.

## Development

The three commands below are the whole loop, whether run on the host with a local Rust toolchain or inside the devcontainer.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets
cargo test --locked
```

Tests that reach the network are marked `#[ignore]`, so the default run stays offline and deterministic.
Run them with `cargo test -- --ignored` after `gh auth login`, which checks the GraphQL query against the real API rather than only against a mock.

A devcontainer is provided in [`.devcontainer/`](.devcontainer), intended to remove the need for a host toolchain.
It has not been exercised yet, so treat it as unverified until it has been: development so far has run on the host.
It mounts `~/Projects` read-only, which means manual runs can see real clones without the container being able to mutate them, but also that anything writing to a clone, `git fetch` included, cannot run inside it as currently configured.

```sh
devcontainer up --workspace-folder .
```

CI runs these same three commands, with tests on Linux, macOS, and Windows.
Lint levels live in `Cargo.toml` under `[lints]`, so a local run and a CI run agree; there are no extra lint flags in the workflow.

## Licence

MIT.
See [LICENSE](LICENSE).
