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

### Groups

A group is simply a directory beneath a root, so `~/Projects/demo/some-repo` is in the `demo` group with nothing to configure.
The tree is the source of truth, and the relationship runs both ways: `--group demo` selects what is already there, and `minato clone --into-group demo` puts new clones where that group already lives.

Moving between groups moves the directory: `minato move imdb-ratings --to-group demo`.
A clone does not care where it sits, so this is a plain rename, but it is still a change to your filesystem.
It is therefore one repository at a time, never a side effect of another command, refuses anything it would have to overwrite, and refuses an ambiguous name rather than guessing which repository you meant.
The directory keeps its own name, which is not always the repository name.

`--group` and `--into-group` are deliberately different.
The first selects by where a clone already sits; the second says where a new one should go.
A repository that has not been cloned is in no group, so filtering `clone` by one would match nothing.

`layout` only decides the name of a *new* clone beneath the directory it is placed in.
It defaults to a flat name, because where a repository belongs is a judgement its identity does not carry.
Use `minato clone --into <directory>` to say where.

A token is never stored here.
It is read from `MINATO_GITHUB_TOKEN`, then `GITHUB_TOKEN`, then the `gh` CLI, so it stays wherever you already keep it.

## Commands

Every command takes `--json` for scripts and agents, and `--refresh` to ignore cached data.

Commands can be narrowed with `--owner`, `--group`, and `--state`, each repeatable, and combining rather than accumulating: naming an owner and a state requires both.
`--state drifted` is shorthand for anything not in sync, which is usually what wants attention.

| Command | What it does |
| --- | --- |
| `minato list` | Every repository the provider reports, with stars, issues, pull requests, licence, and when it was last pushed. |
| `minato status` | How local clones stand against the provider: not cloned, in sync, ahead, behind, diverged, or local only, with the reason. |
| `minato clone` | Clones repositories that have no local copy. `--into <directory>` chooses where they land, defaulting to the first configured root. Skips any destination that already exists. |
| `minato fetch` | Fetches every clone. Updates remote-tracking refs only, so it never touches a working tree and is always safe to run. |
| `minato update` | Fast-forwards clones that are strictly behind and have no modified tracked files. Everything else is reported with the reason it was left alone. |
| `minato move <repo> --to-group <group>` | Moves one repository into another group, which means moving its directory. |
| `minato refresh` | Discards cached data so the next run asks the provider again. |
| `minato auth status` | Whether a token was found and where it came from, never the token itself. |
| `minato doctor` | Checks git, the token, configuration, roots, and the cache, reporting all of them rather than stopping at the first problem. |
| `minato tui` | Browses the same comparison interactively. |

### Interactive browser

`minato tui` opens a keyboard-driven table over exactly the comparison the commands produce.

| Key | Does |
| --- | --- |
| `j` `k`, arrows | Move. `g` and `G` jump to the ends. |
| `/` | Search by repository, group, or path. `Enter` keeps it, `Esc` clears it. |
| `s` | Cycle the ordering: name, state, group. Sorting by state puts what needs attention first. |
| `f` `u` | Fetch or update the highlighted repository. |
| `r` | Rescan the disk. It does not refetch, since a keystroke should not spend your rate limit; use `--refresh` for that. |
| `q` | Leave. |

Every action it offers calls the same function the matching command calls, so nothing can be done here that cannot be scripted.
It needs a terminal, and says so plainly when there is not one rather than failing obscurely.

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

- Rust 1.88 or later (edition 2024).
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

A devcontainer is provided in [`.devcontainer/`](.devcontainer), so no host toolchain is needed beyond Docker.

```sh
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . cargo test --locked
```

It has been verified end to end: the container builds, all tests pass inside it, and `minato` itself runs there.
It carries Rust, `git`, `gh`, `jq`, and `kata`, with the kata ledger bind-mounted from the host so issue state is shared.

Two things worth knowing.

The devcontainer CLI must be reasonably current.
Version 0.76 starts the container and then never returns, which looks like a hang but is not: the container is running fine.
Version 0.87 completes in seconds.

Authentication is not inherited.
`gh` on macOS keeps its token in the keychain, which cannot be bind-mounted, so mounting its configuration directory would only produce a broken login.
Export a token on the host before starting the container instead, which the network tests and any real run inside will pick up.

```sh
export MINATO_GITHUB_TOKEN=$(gh auth token)
devcontainer up --workspace-folder .
```

`~/Projects` is mounted read-only at `/host/Projects`, so a run inside can see real clones and cannot alter them.
That also means anything writing to a clone, `git fetch` included, cannot run against that mount.

CI runs these same three commands, with tests on Linux, macOS, and Windows.
Lint levels live in `Cargo.toml` under `[lints]`, so a local run and a CI run agree; there are no extra lint flags in the workflow.

## Licence

MIT.
See [LICENSE](LICENSE).
