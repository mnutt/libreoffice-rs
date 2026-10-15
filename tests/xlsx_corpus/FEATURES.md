# XLSX drawings and formatting milestone

Implementation: `e45b651` on `davros-fixes`. This is a development milestone,
not a production switch or a claim of complete Excel fidelity.

The [all-page comparison report](runs/chart-layout-verified/report.html) contains the
candidate, office2pdf and real LibreOffice PDFs. Generated binaries and images
remain ignored. `features.json` records the final measurements, source/binary
hashes, font configuration, reference provenance and expectation results.
The original `baseline.json` remains the grid-only comparison.

## Changes driven by this corpus

- Imported internal drawing relationships and absolute/one-cell/two-cell anchors.
  PNG/JPEG pictures, chart sheets and drawing text now contribute to page bounds.
  Drawings are clipped across paper tiles; natural-width preview includes their
  full width. External image relationships are ignored.
- Added cached line, clustered bar and scatter charts to the existing PDF chart
  renderer. Numeric X coordinates, cache indexes/gaps, no-line series, workbook
  theme and individual point colors, currency labels, titles, legends, axis
  bounds, and circle/square marker sizes/colors are retained.
- Chart layout now respects source font sizes, title weight, grid visibility and
  bar gap width. Plot margins use measured labels, and top/bottom legends wrap
  into centered rows. Legend symbols retain scatter marker shape and color.
  Chart-only sheets fit their source paper size even in natural-width preview.
- Automatic wrapped row heights use the rendered cell font and indentation,
  preventing bold or indented text from being clipped by underestimated heights.
- Applied scalar conditional comparisons, relative and absolute references,
  priority/stop rules, differential fonts/fills, and cached numeric/named data
  bars. The budget's hidden bar value stays hidden.
- Preserved drawing text, repeated print-title rows/columns, horizontal centering,
  explicit print scaling and fit-to-page output. Natural-width preview continues
  to use source scale with vertical pagination for ordinary worksheets.
- Added indexed cell colors, negative number-section colors, underline,
  indentation, shrink-to-fit, rotation, and Typewriter font-family mapping.
- Added PDF feature expectations. A missing image or chart page now fails the
  runner even if word recall is perfect. Synthetic tests exercise multi-page
  print titles because the existing repeating-title fixture has short sheets.

No new runtime dependency was added. Formula previews still consume saved
results rather than recalculating spreadsheets.

## Results

All 24 documents convert with all three engines, and all candidate feature
expectations pass. Twenty cases have 100% word recall against LibreOffice,
up from eighteen. This measures retained text, not twenty visual passes.

| Case | Grid-only recall vs LO | Current recall vs LO | Current recall vs office2pdf | Current candidate / office2pdf / LO pages |
| --- | ---: | ---: | ---: | --- |
| WithChart | 37.5% | 100% | 100% | 2 / 2 / 2 |
| SimpleScatterChart | 11.8% | 100% | 64.3% | 2 / 2 / 2 |
| picture | 100% | 100% | 100% | 1 / 1 / 4 |
| simple-monthly-budget | 56.6% | 71.7% | 99.2% | 2 / 1 / 2 |

The picture now appears; the initial grid renderer had zero PDF image objects.
The one-page preview shows the complete picture, while office2pdf's one-page
reference clips it. LibreOffice tiles the picture across four paper pages.

The scatter fixture illustrates why both references matter: LibreOffice has the
numeric chart and its labels, while office2pdf's output differs substantially.
The candidate now follows the numeric coordinates and retains both chart pages.

The budget's only missing office2pdf word is the data-bar value `2336`, which the
source explicitly hides with `showValue=0`. Its remaining deficit against
LibreOffice includes text extraction splitting the reference's narrow text into
fragments. These metrics do not prove equivalent typography or chart geometry.

The other three imperfect LibreOffice text comparisons remain:
`poi-Formatting` (98.4%, locale-dependent year formatting),
`poi-NumberFormatTests` (97.9%, font/layout and extraction differences), and
`100-customers` (85.9% vs LibreOffice, 99.8% vs office2pdf, narrow-cell clipping).
These cases remain visible; thresholds were not loosened to conceal them.

## Remaining fidelity work

The current examples now exercise pictures, chart sheets, conditional styles,
data bars, drawing text, rotated text and repeated titles successfully. They are
still insufficient to establish replacement readiness for arbitrary XLSX input.

- Font substitution, exact line metrics and chart plot/legend geometry still
  differ. Chart sheets now fit paper, but source defaults and chart styling are
  not identical to LibreOffice. Very large legends can still exhaust the space
  available inside small chart frames.
- The default paper size differs when a workbook omits it: this implementation
  uses Letter, while the installed LibreOffice uses A4. Number-format test output
  consequently has nine pages versus LibreOffice's eight; office2pdf also has nine.
- The corpus has no meaningful coverage for print areas, manual breaks,
  combination/stacked/log charts, advanced marker shapes, cropped/rotated images,
  complex conditional formulas, color scales/icon sets, or full rich text.
  These remain unsupported or partial; broader fixtures are needed before making
  wider compatibility claims.
- Cached values can be stale or missing. Named data-bar bounds are resolved only
  from same-sheet formula cells that already cache the named value. Conditional
  work is bounded and produces a diagnostic when its limit is reached.
- No release-size, memory or throughput benchmark was performed. Debug corpus
  timings are not performance comparisons.

## Validation

All affected tests pass (23 Calc unit, ten XLSX integration and 30 core unit
tests), including chart-sheet page fitting, horizontal legend wrapping and
indented bold row-height regressions. Workspace all-target/all-feature checking
and five corpus-runner tests pass. The earlier workspace all-feature test suite
also passed before these refinements. Final corpus runs render every page (up
to ten per fixture) and preserve the exact implementation hash.

Davros production dispatch and dependency pins remain unchanged. The fork's
commits are local until explicitly published.
