# fleet

Overview and sync of Git repositories across hosting providers.

`fleet` shows what you have remotely, what you have cloned locally, and where the two have drifted apart.
It then offers safe, scriptable actions to close the gap: clone what is missing, fetch and report, and fast-forward clones and forks that are strictly behind.

GitHub is the only supported provider for now.

## Status

Early development.
The command surface described in the design note is not yet implemented; this repository currently contains the project scaffold only.
Progress is tracked in the local [kata](https://github.com/kenn-io/kata) ledger (`kata list`).

## Documentation

The documentation website is a Quarto project under [`docs/`](docs), covering the design note now and the command reference as commands land.

```sh
quarto render docs
```

CI renders it on every pull request and uploads the result as an artifact.
Deployment to GitHub Pages is deferred until the repository is public, since Pages on a private repository requires a paid plan.

## Requirements

- Rust 1.85 or later (edition 2024).
- A `git` binary on `PATH`, which `fleet` shells out to so that your existing SSH agent and credential helpers work unchanged.

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
