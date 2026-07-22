## Install

### Quick install (script)

```bash
curl -fsSL https://m.canouil.dev/minato/install.sh | bash

# or pin this exact release
curl -fsSL https://m.canouil.dev/minato/install.sh | bash -s -- --version %%VERSION%%
```

The script picks the archive for your machine, verifies it against `SHA256SUMS`, and installs into `/usr/local/bin` when writable, otherwise `~/.local/bin`.
It needs `bash` and `curl`; on a minimal distribution such as Alpine, install them first with `apk add bash curl`.

### A prebuilt binary

Pick the archive for your machine from the table below, then:

```bash
VERSION=%%VERSION%%
TARGET=x86_64-unknown-linux-musl   # or whichever row matches

curl -fsSLO "https://github.com/mcanouil/minato/releases/download/${VERSION}/minato-${VERSION}-${TARGET}.tar.gz"
curl -fsSLO "https://github.com/mcanouil/minato/releases/download/${VERSION}/SHA256SUMS"

# Check it is what was published.
sha256sum --ignore-missing --check SHA256SUMS

tar -xzf "minato-${VERSION}-${TARGET}.tar.gz"
install -m 0755 minato /usr/local/bin/minato
```

On macOS, `shasum -a 256 --ignore-missing --check SHA256SUMS` does the same job.
On Windows, unzip the `.zip` and put `minato.exe` somewhere on `PATH`.

### With Rust already installed

```bash
cargo install --git https://github.com/mcanouil/minato --tag %%VERSION%%
```

### From source

```bash
git clone https://github.com/mcanouil/minato && cd minato
cargo install --path .
```

### In a devcontainer

No host toolchain is needed beyond Docker.
The bundled `.devcontainer/` carries Rust, `git`, `gh`, `jq`, and `kata`.

```bash
git clone https://github.com/mcanouil/minato && cd minato
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . cargo install --path .
```

## Verify what you downloaded

Beyond the checksum, every archive carries build provenance, so you can confirm it came from this repository's workflow and not from somewhere else:

```bash
gh attestation verify "minato-%%VERSION%%-x86_64-unknown-linux-musl.tar.gz" \
  --repo mcanouil/minato
```

## Which archive is which

| Archive | For |
| --- | --- |
| `minato-%%VERSION%%-x86_64-unknown-linux-musl.tar.gz` | Linux on Intel or AMD, any distribution. |
| `minato-%%VERSION%%-aarch64-unknown-linux-musl.tar.gz` | Linux on ARM, including most cloud instances. |
| `minato-%%VERSION%%-aarch64-apple-darwin.tar.gz` | macOS on Apple silicon. |
| `minato-%%VERSION%%-x86_64-apple-darwin.tar.gz` | macOS on Intel. |
| `minato-%%VERSION%%-x86_64-pc-windows-msvc.zip` | Windows on Intel or AMD. |

## Documentation

<https://m.canouil.dev/minato/>

---

## Changes
