// @license MIT
// @copyright 2026 Mickaël Canouil
// @author Mickaël Canouil
//
// The 1200x630 social card, rendered by render-assets.sh. Compiled at 72 ppi
// from a page measured in points, so one point is one pixel.
//
// The mark is the same icon.svg the favicons come from. It carries the light
// scheme colours, so it sits on a brand-light plaque rather than straight on
// the deep-water background, which keeps the teal at full contrast without a
// second copy of the artwork.

#set page(width: 1200pt, height: 630pt, margin: 0pt, fill: rgb("#0C1620"))
#set text(font: "Inter", fill: rgb("#E6EDF3"))

#let plaque = box(
  width: 220pt,
  height: 220pt,
  radius: 48pt,
  fill: rgb("#F7F9FB"),
  align(center + horizon, image("/assets/images/icon.svg", width: 140pt)),
)

// Centred on the tagline, which is the wider of the two lines, so the title
// sits over its own descriptive line rather than flush against the mark.
#let wordmark = {
  set align(center)
  text(font: "Outfit", weight: 600, size: 112pt, [Minato])
  v(14pt, weak: true)
  text(size: 40pt, fill: rgb("#8FA9B8"), [A harbour for your repositories.])
}

// The lockup is centred and sized to its contents, which leaves far more than
// the 96pt of clear space a cropped or shrunken card needs on any side.
#align(
  center + horizon,
  pad(
    x: 96pt,
    grid(
      columns: (220pt, auto),
      column-gutter: 56pt,
      align: horizon,
      plaque,
      wordmark,
    ),
  ),
)
