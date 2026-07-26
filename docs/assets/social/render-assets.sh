#!/usr/bin/env bash
# @license MIT
# @copyright 2026 Mickaël Canouil
# @author Mickaël Canouil
#
# Regenerate every published icon and the social card from their two sources,
# assets/images/icon.svg and assets/social/_og-image.typ.
#
#   docs/assets/social/render-assets.sh
#
# Needs librsvg (rsvg-convert), ImageMagick 7 (magick), and typst, plus the
# brand fonts Outfit and Inter for the card. The outputs are committed, so this
# runs on a workstation; nothing in CI regenerates them.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docs_dir="$(cd "${script_dir}/../.." && pwd)"
images_dir="${docs_dir}/assets/images"
icon="${images_dir}/icon.svg"

work_dir="$(mktemp -d)"
trap 'rm -rf "${work_dir}"' EXIT

# The mark at `mark` pixels, centred on an opaque `size` pixel square of the
# brand light background. Launchers and home screens composite these against
# unknown wallpaper, so they cannot be transparent, and they must not round
# their own corners: every platform applies its own mask.
render_padded() {
	local size="$1" mark="$2" output="$3"
	rsvg-convert -w "${mark}" -h "${mark}" "${icon}" -o "${work_dir}/mark-${mark}.png"
	magick "${work_dir}/mark-${mark}.png" \
		-background '#F7F9FB' -gravity center -extent "${size}x${size}" \
		-alpha remove -alpha off \
		"${output}"
}

# The .ico keeps its transparency: it is the legacy tab icon, drawn against
# browser chrome that is light or dark depending on the platform theme.
rsvg-convert -w 32 -h 32 "${icon}" -o "${work_dir}/mark-32.png"
magick "${work_dir}/mark-32.png" -define icon:auto-resize=32 "${docs_dir}/favicon.ico"

render_padded 180 144 "${images_dir}/apple-touch-icon.png"
render_padded 192 154 "${images_dir}/icon-192.png"
render_padded 512 410 "${images_dir}/icon-512.png"

# --root lets the template read the icon by its project-relative path, and
# --ppi 72 makes one typst point one pixel, so the page size is the pixel size.
typst compile \
	--root "${docs_dir}" \
	--ppi 72 \
	"${script_dir}/_og-image.typ" \
	"${images_dir}/og-image.png"

magick identify \
	"${docs_dir}/favicon.ico" \
	"${images_dir}/apple-touch-icon.png" \
	"${images_dir}/icon-192.png" \
	"${images_dir}/icon-512.png" \
	"${images_dir}/og-image.png"
