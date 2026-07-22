# Contributing to minato

Thanks for taking the time to help.

## Reporting a bug

Open an [issue](https://github.com/mcanouil/minato/issues) describing what you did, what you expected, and what happened instead.
Include the output of `minato --version`, your operating system, and a minimal set of steps to reproduce it.
Since `minato` shells out to `git`, the `git --version` and how your remotes are configured are often relevant too.

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

No host clones are mounted.
The integration tests, and any manual run inside, use disposable git repositories created in a temporary directory, so `git fetch` and the ahead, behind, and sync paths can be exercised.
The read-only guarantee lives where the design note puts it, in the action layer: an update is only ever a fast-forward, and `--dry-run` rehearses first.

CI runs these same three commands, with tests on Linux, macOS, and Windows.
Lint levels live in `Cargo.toml` under `[lints]`, so a local run and a CI run agree; there are no extra lint flags in the workflow.

## Commit conventions

Commits follow [Conventional Commits](https://www.conventionalcommits.org): a type such as `feat`, `fix`, `docs`, `refactor`, `test`, or `chore`, then an imperative summary, for example `fix: skip a remoteless clone when fetching`.
Keep one logical change per commit.
The changelog and the release notes are built from these, so a clear subject is what a reader eventually sees.

Update [`CHANGELOG.md`](CHANGELOG.md) under `[Unreleased]` for any user-facing change, grouped under `### Features` or `### Bug Fixes`.
