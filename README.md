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

Development happens inside the devcontainer, so no toolchain is required on the host beyond Docker and an editor that supports devcontainers.

```sh
devcontainer up --workspace-folder .
```

The container mounts `~/Projects` read-only, so manual runs can see real clones without any risk of the container mutating them.
Destructive manual checks, such as clone and fast-forward, are done by running the built binary on the host.

Inside the container:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features
cargo test --all-features
```

CI runs the same three commands, with tests on Linux, macOS, and Windows.

## Licence

MIT.
See [LICENSE](LICENSE).
