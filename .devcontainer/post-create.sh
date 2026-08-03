#!/usr/bin/env bash
# Container setup that cannot be expressed as an image or a devcontainer feature.
set -euo pipefail

# kata is the project's issue ledger; the database itself is bind-mounted from
# the host at ${KATA_HOME}, but the binary has to be installed in the container.
# The installer picks /usr/local/bin when writable and ~/.local/bin otherwise,
# which for the unprivileged container user means ~/.local/bin.
curl -fsSL https://katatracker.com/install.sh | bash

kata version

# PSScriptAnalyzer lints docs/install.ps1, the Windows installer, under the same
# rules the Installer workflow runs. PowerShell itself comes from a feature.
pwsh -NoProfile -Command "Install-Module -Name PSScriptAnalyzer -Force -Scope CurrentUser -SkipPublisherCheck"
