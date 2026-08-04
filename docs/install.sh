#!/usr/bin/env bash
# @license MIT
# @copyright 2026 Mickaël Canouil
# @author Mickaël Canouil
#
# minato installer.
#
#   curl -fsSL https://m.canouil.dev/minato/install.sh | bash
#
# Downloads the prebuilt release binary for this machine, verifies it against
# the release SHA256SUMS, and installs it onto PATH.
#
# Environment variables:
#   MINATO_VERSION             Install this version instead of the latest.
#   MINATO_INSTALL_DIR         Install here instead of the resolved default.
#   MINATO_SKIP_CHECKSUM=1     Skip SHA256 verification (not recommended).
#   MINATO_VERIFY_PROVENANCE=1 Also verify build provenance with the gh CLI.
#
# This installer needs bash. On a minimal distribution such as Alpine, which
# ships only busybox, install it first: `apk add bash curl`.

# POSIX-syntax guard so `sh install.sh` fails clearly rather than mis-parsing
# the bash below. It runs before `set -o pipefail`, which dash rejects.
if [ -z "${BASH_VERSION:-}" ]; then
	echo "This installer needs bash. Run: bash install.sh (or: curl -fsSL https://m.canouil.dev/minato/install.sh | bash)" >&2
	exit 1
fi

set -euo pipefail

REPO="mcanouil/minato"
BINARY_NAME="minato"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}$1${NC}"; }
warn() { echo -e "${YELLOW}$1${NC}"; }
error() {
	echo -e "${RED}$1${NC}" >&2
	exit 1
}

# Where a completion script installed by `minato completions --install` would
# be.
#
# Mirrors `known_locations` in src/cli/completions.rs, which `minato doctor`
# uses; keep the two in step, which a test compares them on. $fpath is a zsh
# variable and this is bash, so the zsh entries are the conventional
# directories rather than a real search of it, and $ZSH_CUSTOM is read for an
# oh-my-zsh that has been moved even though a `curl | bash` pipe will rarely
# have it exported.
#
# PowerShell is absent for a reason of its own: the documented setup evaluates
# the script from $PROFILE at every session, so it regenerates itself and can
# never be stale.
completion_locations() {
	local zsh_custom="${ZSH_CUSTOM:-${HOME}/.oh-my-zsh/custom}"
	local data="${XDG_DATA_HOME:-${HOME}/.local/share}"
	local config="${XDG_CONFIG_HOME:-${HOME}/.config}"
	local brew="${HOMEBREW_PREFIX:-}"

	printf '%s\t%s\n' \
		bash "${data}/bash-completion/completions/${BINARY_NAME}" \
		bash "${data}/${BINARY_NAME}-completions/${BINARY_NAME}.bash" \
		zsh "${zsh_custom}/completions/_${BINARY_NAME}" \
		zsh "${HOME}/.zfunc/_${BINARY_NAME}" \
		zsh "${data}/zsh/site-functions/_${BINARY_NAME}" \
		fish "${config}/fish/completions/${BINARY_NAME}.fish" \
		elvish "${config}/elvish/lib/${BINARY_NAME}.elv"

	# Homebrew's own share/zsh/site-functions, which its shell setup puts on
	# $fpath. Only $HOMEBREW_PREFIX is consulted here: probing /opt/homebrew and
	# /usr/local, as the binary does, would report on a prefix this installer
	# never wrote to.
	if [ -n "${brew}" ]; then
		printf '%s\t%s\n' zsh "${brew}/share/zsh/site-functions/_${BINARY_NAME}"
	fi
}

# Names the completion scripts the version just installed would generate
# differently, with the command that rewrites each.
#
# An install is the one moment that knows the command surface may have changed,
# and a stale script keeps working while quietly offering the commands of an
# older release. Comparing against the binary just installed, rather than
# assuming every script is now stale, keeps a reinstall of the same version
# silent.
report_stale_completions() {
	local binary="$1"
	local shell path generated announced=0 announced_shells=""

	# No HOME means no conventional location to look in, and `set -u` would
	# trip on the lookups below.
	[ -n "${HOME:-}" ] || return 0

	while IFS=$'\t' read -r shell path; do
		[ -f "${path}" ] || continue
		# A binary that cannot run here says nothing about the script; the
		# install already reported what it could, and `doctor` will say more.
		generated=$("${binary}" completions "${shell}" 2>/dev/null) || continue
		[ "${generated}" = "$(cat "${path}")" ] && continue

		if [ "${announced}" -eq 0 ]; then
			warn "Completion scripts do not update themselves. Regenerate:"
			announced=1
		fi
		# One command per shell rather than per file: --install finds the file
		# itself, so two stale copies of one shell's script are one command.
		case " ${announced_shells} " in
		*" ${shell} "*) ;;
		*)
			announced_shells="${announced_shells} ${shell}"
			echo "  ${BINARY_NAME} completions ${shell} --install"
			;;
		esac
	done < <(completion_locations)

	if [ "${announced}" -eq 1 ]; then
		echo
	fi
}

# mktemp output lives in a global so the EXIT trap, which runs after main()
# returns and its locals are gone, can still see and remove it. Guarded so an
# exit before mktemp (empty tmpdir) does not trip `set -u`.
tmpdir=""
cleanup() {
	[ -n "${tmpdir}" ] && rm -rf "${tmpdir}"
	return 0
}
trap cleanup EXIT

usage() {
	cat <<EOF
minato installer

Usage:
  curl -fsSL https://m.canouil.dev/minato/install.sh | bash
  ./install.sh [--version <version>] [--dir <path>] [--help]

Options:
  --version <version>  Install this version instead of the latest.
  --dir <path>         Install into this directory.
  --help               Show this help and exit.

Environment variables:
  MINATO_VERSION, MINATO_INSTALL_DIR, MINATO_SKIP_CHECKSUM,
  MINATO_VERIFY_PROVENANCE. See the script header for details.
EOF
}

# minato publishes one archive per Rust target triple. Map the running machine
# onto the triple the release job built, so the filename lines up exactly.
detect_target() {
	local os arch
	case "$(uname -s)" in
	Darwin) os="apple-darwin" ;;
	Linux) os="unknown-linux-musl" ;;
	*) error "Unsupported OS: $(uname -s). minato ships binaries for macOS and Linux; on Windows use the .zip from the releases page." ;;
	esac
	case "$(uname -m)" in
	x86_64 | amd64) arch="x86_64" ;;
	aarch64 | arm64) arch="aarch64" ;;
	*) error "Unsupported architecture: $(uname -m)." ;;
	esac
	echo "${arch}-${os}"
}

find_install_dir() {
	if [ -n "${MINATO_INSTALL_DIR:-}" ]; then
		# Creating it is left to the install step, which can fall back to sudo
		# for a root-owned path such as /opt/minato; an eager mkdir here would
		# abort under `set -e` before that fallback is reached.
		echo "${MINATO_INSTALL_DIR}"
	elif [ -w "/usr/local/bin" ]; then
		echo "/usr/local/bin"
	else
		mkdir -p "${HOME}/.local/bin"
		echo "${HOME}/.local/bin"
	fi
}

download() {
	local url="$1" output="$2"
	if command -v curl &>/dev/null; then
		curl -fsSL "${url}" -o "${output}"
	elif command -v wget &>/dev/null; then
		wget -q "${url}" -O "${output}"
	else
		error "Neither curl nor wget is available."
	fi
}

get_latest_version() {
	# Follow the redirect from the HTML /releases/latest to /releases/tag/<tag>.
	# Unlike api.github.com this is not rate-limited to 60 requests per hour per
	# IP, so users behind a shared address are not turned away with a 403.
	local url="https://github.com/${REPO}/releases/latest"
	local final_url=""
	if command -v curl &>/dev/null; then
		final_url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "${url}") || return 1
	elif command -v wget &>/dev/null; then
		final_url=$(wget --spider -S "${url}" 2>&1 |
			awk 'tolower($1)=="location:" {print $2}' |
			tail -1 |
			tr -d '\r\n') || return 1
	else
		return 1
	fi
	case "${final_url}" in
	*/releases/tag/*) echo "${final_url##*/releases/tag/}" ;;
	*) return 1 ;;
	esac
}

verify_checksum() {
	local file="$1" checksums_file="$2" filename="$3"

	if [ ! -f "${checksums_file}" ]; then
		error "SHA256SUMS is not available. Set MINATO_SKIP_CHECKSUM=1 to bypass."
	fi

	local expected
	expected=$(awk -v f="${filename}" '{gsub(/^\*/, "", $2); if ($2==f) {print $1; exit}}' "${checksums_file}")
	if [ -z "${expected}" ]; then
		error "No checksum for ${filename} in SHA256SUMS."
	fi

	local actual
	if command -v sha256sum &>/dev/null; then
		actual=$(sha256sum "${file}" | cut -d' ' -f1)
	elif command -v shasum &>/dev/null; then
		actual=$(shasum -a 256 "${file}" | cut -d' ' -f1)
	else
		error "No sha256 tool found. Install coreutils or set MINATO_SKIP_CHECKSUM=1 to bypass."
	fi

	if [ "${expected}" != "${actual}" ]; then
		error "Checksum verification failed.\n  Expected: ${expected}\n  Actual:   ${actual}"
	fi
	info "Checksum verified."
}

verify_provenance() {
	local file="$1"
	if ! command -v gh &>/dev/null; then
		error "MINATO_VERIFY_PROVENANCE=1 needs the gh CLI, which is not installed."
	fi
	info "Verifying build provenance..."
	if ! gh attestation verify "${file}" --repo "${REPO}"; then
		error "Build provenance verification failed."
	fi
}

main() {
	local version="${MINATO_VERSION:-}"
	local install_dir_override=""

	while [ "$#" -gt 0 ]; do
		case "$1" in
		--version)
			[ "$#" -ge 2 ] || error "--version needs a value."
			version="$2"
			shift 2
			;;
		--dir)
			[ "$#" -ge 2 ] || error "--dir needs a value."
			install_dir_override="$2"
			shift 2
			;;
		--help | -h)
			usage
			exit 0
			;;
		*) error "Unknown argument: $1 (try --help)." ;;
		esac
	done

	info "Installing ${BINARY_NAME}..."
	echo

	local target
	target=$(detect_target)

	if [ -z "${version}" ]; then
		info "Resolving the latest release..."
		version=$(get_latest_version) ||
			error "Could not resolve the latest version. Pass --version or see https://github.com/${REPO}/releases."
	fi
	# The tags carry no leading v; accept one anyway so a pasted v0.1.0 works.
	version="${version#v}"

	local install_dir
	install_dir=$(MINATO_INSTALL_DIR="${install_dir_override:-${MINATO_INSTALL_DIR:-}}" find_install_dir)

	info "Version:           ${version}"
	info "Target:            ${target}"
	info "Install directory: ${install_dir}"
	echo

	local filename="${BINARY_NAME}-${version}-${target}.tar.gz"
	local base_url="https://github.com/${REPO}/releases/download/${version}"

	tmpdir=$(mktemp -d)

	info "Downloading ${filename}..."
	download "${base_url}/${filename}" "${tmpdir}/${filename}" ||
		error "Download failed. See https://github.com/${REPO}/releases for available builds."

	if [ "${MINATO_SKIP_CHECKSUM:-0}" = "1" ]; then
		warn "Checksum verification skipped (MINATO_SKIP_CHECKSUM=1)."
	else
		download "${base_url}/SHA256SUMS" "${tmpdir}/SHA256SUMS" ||
			error "Could not download SHA256SUMS. Set MINATO_SKIP_CHECKSUM=1 to bypass."
		verify_checksum "${tmpdir}/${filename}" "${tmpdir}/SHA256SUMS" "${filename}"
	fi

	if [ "${MINATO_VERIFY_PROVENANCE:-0}" = "1" ]; then
		verify_provenance "${tmpdir}/${filename}"
	fi

	info "Extracting..."
	tar -xzf "${tmpdir}/${filename}" -C "${tmpdir}"
	[ -f "${tmpdir}/${BINARY_NAME}" ] || error "The archive did not contain a ${BINARY_NAME} binary."

	# A root-owned directory such as /usr/local/bin needs sudo for every write,
	# the signing included, so resolve the prefix once. Left empty when the
	# directory is writable, so nothing runs under sudo needlessly.
	local sudo_cmd=""
	if [ ! -w "${install_dir}" ]; then
		warn "${install_dir} is not writable; using sudo."
		sudo_cmd="sudo"
	fi
	# Create the directory now, with sudo when it or its parent is root-owned,
	# so a custom MINATO_INSTALL_DIR that does not yet exist is handled here.
	# shellcheck disable=SC2086
	${sudo_cmd} mkdir -p "${install_dir}"
	# shellcheck disable=SC2086
	${sudo_cmd} mv "${tmpdir}/${BINARY_NAME}" "${install_dir}/"
	# shellcheck disable=SC2086
	${sudo_cmd} chmod +x "${install_dir}/${BINARY_NAME}"

	# An unsigned Mach-O binary is killed by Gatekeeper on first run; an ad-hoc
	# signature is enough to let it start. Non-fatal, since the binary still runs
	# once the user clears it manually.
	if [ "$(uname -s)" = "Darwin" ]; then
		# shellcheck disable=SC2086
		${sudo_cmd} codesign -s - "${install_dir}/${BINARY_NAME}" 2>/dev/null || true
	fi

	echo
	info "Installed ${BINARY_NAME} ${version} to ${install_dir}/${BINARY_NAME}."
	echo

	case ":${PATH}:" in
	*":${install_dir}:"*) ;;
	*)
		warn "${install_dir} is not on your PATH. Add this to your shell profile:"
		echo "  export PATH=\"${install_dir}:\$PATH\""
		echo
		;;
	esac

	report_stale_completions "${install_dir}/${BINARY_NAME}"

	echo "Next steps:"
	echo "  ${BINARY_NAME} doctor   # Check configuration and tooling are usable"
	echo "  ${BINARY_NAME} --help   # List the commands"
	echo
	echo "See https://m.canouil.dev/minato/get-started/ to configure it."
}

# Guard: run main only when executed directly, not when sourced. A curl | bash
# pipe leaves BASH_SOURCE[0] empty, which we treat as direct execution.
if [[ "${BASH_SOURCE[0]-}" == "${0}" || -z "${BASH_SOURCE[0]-}" ]]; then
	main "$@"
fi
