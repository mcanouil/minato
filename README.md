# Minato

Overview and sync of Git repositories across hosting providers.

Minato shows what you have remotely, what you have cloned locally, and where the two have drifted apart.
It then offers safe, scriptable actions to close the gap: clone what is missing, fetch and report, and fast-forward clones and forks that are strictly behind.

GitHub is the only supported provider for now.

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

Groups nest as the tree does: `~/Projects/perso/apps/minato` is in `perso/apps`, a group whose parent is `perso`.
Naming a group takes everything beneath it, so `--group perso` keeps `perso/apps` and `perso/data` as well, while `--group perso/apps` keeps only that bucket.
Matching runs from the root down, one directory at a time, so `--group apps` does not stand for `perso/apps`.

Moving between groups moves the directory: `minato move imdb-ratings --to-group demo`, or `--to-group perso/apps` for a nested one.
A clone does not care where it sits, so this is a plain rename, but it is still a change to your filesystem.
It is therefore one repository at a time, never a side effect of another command, refuses anything it would have to overwrite, and refuses an ambiguous name rather than guessing which repository you meant.
The directory keeps its own name, which is not always the repository name.

`--group` and `--into-group` are deliberately different.
The first selects by where a clone already sits; the second says where a new one should go.
A repository that has not been cloned is in no group, so filtering `clone` by one would match nothing.

`layout` only decides the name of a _new_ clone beneath the directory it is placed in.
It defaults to a flat name, because where a repository belongs is a judgement its identity does not carry.
Use `minato clone --into <directory>` to say where.

A token is never stored here.
It is read from `MINATO_GITHUB_TOKEN`, then `GITHUB_TOKEN`, then the `gh` CLI, so it stays wherever you already keep it.

## Commands

Every command takes `--json` for scripts and agents, and `--refresh` to ignore cached data.

Commands can be narrowed with `--owner`, `--group`, and `--state`, each repeatable, and combining rather than accumulating: naming an owner and a state requires both.
`--state drifted` is shorthand for anything not in sync, which is usually what wants attention.

A group and a state describe a local clone, so only the commands that scan for one act on them: `status`, `clone`, `fetch`, `update`, and `tui`.
`list` and `sync-fork` work from what GitHub reports and never scan, so they take `--owner` alone, and `move` names its repository outright.
A command asked to narrow by a condition it cannot act on says so and stops, rather than answering the question you did not ask.

| Command                                 | What it does                                                                                                                                                                   |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `minato list`                           | Every repository the provider reports, with stars, issues, pull requests, licence, and when it was last pushed.                                                                |
| `minato status`                         | How local clones stand against the provider: not cloned, in sync, ahead, behind, diverged, or local only, with the reason.                                                     |
| `minato clone`                          | Clones repositories that have no local copy. `--into <directory>` chooses where they land, defaulting to the first configured root. Skips any destination that already exists. |
| `minato fetch`                          | Fetches every clone. Updates remote-tracking refs only, so it never touches a working tree and is always safe to run.                                                          |
| `minato update`                         | Fast-forwards clones that are strictly behind and have no modified tracked files. Everything else is reported with the reason it was left alone.                               |
| `minato move <repo> --to-group <group>` | Moves one repository into another group, which means moving its directory.                                                                                                     |
| `minato refresh`                        | Discards cached data so the next run asks the provider again.                                                                                                                  |
| `minato auth status`                    | Whether a token was found and where it came from, never the token itself.                                                                                                      |
| `minato doctor`                         | Checks git, the token, configuration, roots, and the cache, reporting all of them rather than stopping at the first problem.                                                   |
| `minato tui`                            | Browses the same comparison interactively.                                                                                                                                     |

### Interactive browser

`minato tui` opens a keyboard-driven table over exactly the comparison the commands produce.

| Key             | Does                                                                                                                |
| --------------- | ------------------------------------------------------------------------------------------------------------------- |
| `j` `k`, arrows | Move. `g` and `G` jump to the ends.                                                                                 |
| `/`             | Search by repository, group, or path. `Enter` keeps it, `Esc` clears it.                                            |
| `s`             | Cycle the ordering: name, state, group. Sorting by state puts what needs attention first.                           |
| `f` `u`         | Fetch or update the highlighted repository.                                                                         |
| `r`             | Rescan the disk. It does not refetch, since a keystroke should not spend your rate limit; use `--refresh` for that. |
| `q`             | Leave.                                                                                                              |

Every action it offers calls the same function the matching command calls, so nothing can be done here that cannot be scripted.
It needs a terminal, and says so plainly when there is not one rather than failing obscurely.

`clone`, `fetch`, and `update` all take `--dry-run`, which reports what would happen and changes nothing.
One repository failing does not stop the others; every repository is reported and the process exits non-zero if any of them failed.

Nothing force-pushes, rebases, or discards a change.
An update is only ever a fast-forward, so if the situation has been misjudged, git refuses rather than improvising.

Provider responses are cached for fifteen minutes.
Cached output says how old it is, so stale data never passes for fresh, and it stays readable with no network at all.

## Completion

`minato completions <shell>` prints a completion script for `bash`, `zsh`, `fish`, `powershell`, or `elvish`.

```sh
minato completions zsh > ~/.zfunc/_minato
```

It goes to standard output rather than to a path Minato picks, because where a completion script belongs is the shell's business.
The script is generated from the command definitions, so it offers exactly the commands and flags the binary has.
Where each shell looks for it is in [Get started](https://m.canouil.dev/minato/get-started/#enable-completion).

## Documentation

The documentation website is a Quarto project under [`docs/`](docs), covering the design note now and the command reference as commands land.

```sh
quarto render docs
```

CI renders it on every pull request and uploads the result as an artifact.
Deployment to GitHub Pages is deferred until the repository is public, since Pages on a private repository requires a paid plan.

## Install

### Quick install (script)

On macOS and Linux:

```sh
curl -fsSL https://m.canouil.dev/minato/install.sh | bash
```

On Windows:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://m.canouil.dev/minato/install.ps1 | iex"
```

Either script detects your platform, verifies the download against the release `SHA256SUMS`, and installs into `/usr/local/bin` when writable and otherwise `~/.local/bin`, or `%LOCALAPPDATA%\Programs\minato` on Windows, which it adds to your user `PATH`.
Set `MINATO_VERSION` to pin a release, `MINATO_INSTALL_DIR` to choose where it lands, `MINATO_SKIP_CHECKSUM=1` to skip verification, or `MINATO_NO_MODIFY_PATH=1` to leave `PATH` alone.

### With cargo

```sh
cargo install --git https://github.com/mcanouil/minato --tag <version>
```

### From a released binary

Every [release](https://github.com/mcanouil/minato/releases) carries a binary for macOS (Apple silicon and Intel), Linux (x86-64 and ARM64), and Windows.

```sh
curl -fsSLO https://github.com/mcanouil/minato/releases/download/<version>/minato-<version>-<target>.tar.gz
curl -fsSLO https://github.com/mcanouil/minato/releases/download/<version>/SHA256SUMS
sha256sum --ignore-missing --check SHA256SUMS
tar -xzf minato-<version>-<target>.tar.gz
install -m 0755 minato /usr/local/bin/minato
```

On macOS, `shasum -a 256 --ignore-missing --check SHA256SUMS` does the same job.
On Windows, unzip the `.zip` and put `minato.exe` on your `PATH`.

### From source

```sh
git clone https://github.com/mcanouil/minato && cd minato
cargo install --path .
```

### In a devcontainer

No host toolchain is needed beyond Docker; see [`CONTRIBUTING.md`](CONTRIBUTING.md) for the full setup.

```sh
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . cargo install --path .
```

## Requirements

- Rust 1.88 or later (edition 2024).
- A `git` binary on `PATH`, which Minato shells out to so that your existing SSH agent and credential helpers work unchanged.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for bug reporting, development setup, and commit conventions.

## Citation

If you use _minato_ in your work, please cite it.
Citation metadata is in [`CITATION.cff`](CITATION.cff).
GitHub renders it via the "Cite this repository" widget on the repository sidebar.

## Licence

Released under the MIT Licence.
See the [LICENSE](LICENSE) file for details.
