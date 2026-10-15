# PPTX fidelity results — 2026-10-15

The first broad pass covers 704 upstream fixture paths, deduplicated into 627
presentations. The fork renders 614. The other 13 are malformed/encrypted packages
with explicit expected parser errors. There are no candidate crashes or timeouts
in the final run, and all current feature checks pass.

This is substantial groundwork, **not LibreOffice parity**.

| Measure | Original importer | Updated importer |
| --- | ---: | ---: |
| Successful PDFs | 614 | 614 |
| Mean LibreOffice text recall, 424 paired nonempty references | 52.9% | 65.7% |
| Exact page-dimension/page-count agreement | 0 | 239 |
| Feature-check failures under current expectations | 10 | 0 |

151 decks improve in text recall by more than one percentage point; 11 regress
by more than one point. The complete per-document deltas, failures, provenance,
font hashes and tool versions are in `results.json`. Pixel scores are absent when
page dimensions differ, including tiny rounding differences. At 0.1-point page
size tolerance, 601 final decks agree with LibreOffice in size and page count.
The original importer's page geometry prevents a meaningful paired aggregate
pixel-error comparison. Text recall cannot establish visual fidelity.

Desktop LibreOffice produced 611 PDFs, with 15 errors and one timeout; office2pdf
produced 613, with 14 errors. Reference errors remain visible. References were
captured with the earlier 128 MiB file cap; the final candidate uses 256 MiB.
No reference error was an output-size-limit signal. These are reproducible local
results, not a general reliability ranking of the three converters.

## Implemented

- Source slide order and dimensions; actual shape/group coordinates.
- Master/layout placeholder frames, text properties, theme fonts/colors, and
  non-placeholder master artwork. Speaker notes are not painted onto slide PDFs.
- Rich text sizes, fonts, weight, italics, underline, colors, alignment and basic
  paragraph spacing/bullets; inherited insets/vertical anchors; saved autofit scale.
- Tight line spacing no longer clips the last line. Slide-number fields resolve.
- PNG/JPEG/WebP/SVG import plus bounded GIF/BMP decoding. PDF pictures preserve
  source crops, rotation and flips. SVG rendering requires the existing `svg` feature.
- Background pictures and linear-gradient backgrounds; solid theme fill references.
- Table row/column geometry, merged-cell spans, theme fills (including `fillRef`),
  header text styling, margins, and basic per-edge borders.
- Positioned cached charts through the shared renderer; radial radar plots and
  date-formatted categories. Existing chart rendering remains the fallback for
  hierarchical categories, area/doughnut/combination charts and other cases the
  shared renderer cannot yet interpret.
- Shared image resources across slides: a 24-slide master-background deck now
  produces a 9.6 MiB PDF instead of hitting the old 128 MiB file cap.

Examples reviewed against LibreOffice include the master-slide fixture (text
recall 32.1% → 100%), themed table headers and run colors, radar chart labels,
merged tables, and the black GIF-background fixture. The latter's pixel error
fell from 0.994 in the intermediate importer to 0.021 after GIF support.
The table-header comparison exposed and fixed theme-fill inheritance that text
recall alone could not detect.

## Validation

- 247 workspace tests passed with all features; the final table-fill regression
  test and all 24 Impress tests also pass after the last importer correction.
- 13 shared Python runner tests pass.
- All 627 current feature checks pass (`feature-checks.json`). Ten failures across
  six original-renderer decks confirm that the visual/text probes detect missing
  content (`negative-controls.json`). Expected invalid inputs must fail normally,
  rather than panic, time out or produce a different error.
- All 54 existing XLSX fixtures pass; their final PDFs are byte-identical to the
  prior hardened renderer (`xlsx-regression.json`).
- The corpus defaults to a 256 MiB per-file limit. Davros's separate PDF output
  check and process limit were updated to the same value in its working tree,
  without committing Davros.

## Second fidelity pass

Commit `a9605d6` adds cached SmartArt drawings, bounded custom DrawingML paths and
common preset geometry, theme line widths, group rotation/flipping, vertical text,
and long-word wrapping. Background-filled shapes now also cover preceding content
when the slide uses its implicit white background.

The complete 627-deck rerun retains 614 successful conversions and 13 expected
parser rejections. Mean LibreOffice text recall increases from 65.705% to 72.602%
(424 paired nonempty references). On 239 decks with exactly matching page sizes,
mean raster RMSE decreases from 0.119912 to 0.105821. There are 45 text improvements
and five regressions above one percentage point; 33 visual improvements and three
regressions above 0.01 RMSE. These are separate metrics, not a parity score.
`second-pass-results.json` preserves every comparison, including regressions.

All 627 updated feature checks pass. The prior renderer fails the new SmartArt
and background-coverage checks (`second-negative-controls.json`). All 54 XLSX
PDFs remain byte-identical (`xlsx-second-regression.json`). Workspace all-feature
tests pass, followed by all 30 Impress tests including the added regression tests.

Remaining visual regressions include a binary-digit text slide (line breaks/font
metrics), a SmartArt slide whose newly visible text improves recall but increases
pixel error, and a rotated-shape fixture. Word-level recall also penalizes some
changed line breaks; do not interpret it as a count of missing rendered glyphs.

## Follow-up fixes

Explicitly hidden shapes, pictures, charts and groups are now skipped. A hidden
child does not suppress visible siblings. All five affected corpus decks were
checked; negative controls catch the previously visible group and chart labels
(`hidden-object-results.json`).

PDF export now paints text highlights, resolves preset dash patterns (including
the distinct dot/system-dot spacing), and scales bounded custom dash patterns by
stroke width. All 32 decks containing highlight or non-solid dash properties pass
the focused comparison and feature checks (`line-text-style-results.json`). New
color-count gates detect missing highlights and solid lines replacing dashes.
These styles remain approximations: line caps, joins and arrowheads need more
work, as do exact glyph highlight ascent/descent bounds. The raw comparisons
retain the small pixel regression on the line-properties fixture.

## PNG and color-map pass

The shared PNG decoder now supports Adam7 interlacing, packed grayscale/palette
samples, 16-bit channels, and grayscale/RGB transparency keys. It retains bounded
inflation and pixel limits and adds no dependencies. Odd-sized/empty Adam7 passes,
per-pass filter history, truncation, palette transparency and 16-bit samples have
unit coverage. PPTX layout/slide color-map overrides also resolve accent aliases
against an unmodified theme palette, avoiding errors when two accents swap.

All 627 checks pass and all 54 XLSX PDFs remain byte-identical
(`third-pass-results.json`, `third-feature-checks.json`,
`xlsx-third-regression.json`). Exact-size mean pixel error falls from 0.105821 to
0.102016; text recall stays at 72.602%. The 579-deck rounding-tolerant audit falls
from 0.097809 to 0.092620 (`third-visual-audit.json`). It also exposes a background
inheritance regression on `lo-chart_pt_color_bg1-be2773`, which the exact-size
cohort misses. The next pass corrects the background's color-map scope; this
intermediate report deliberately preserves the regression.

## Remaining work, in priority order

1. SmartArt layout for files without usable cached drawing shapes. Those diagrams
   still require a layout engine; cached drawing fallbacks are now supported.
2. Remaining preset geometry, connector routing, line caps/arrowheads, gradient
   clipping, and exact group scaling of fonts. Common presets and custom paths,
   plus group rotation/flips, are now supported.
3. Text columns, hanging bullets, automatic fit calculation, East Asian vertical text,
   per-character fallback/shaping, superscripts and more exact font baselines.
4. Full chart axis/legend/layout semantics, hierarchical category placement in the
   shared engine, and advanced chart types. The remaining text regressions are
   mostly chart ticks/labels, plus complex text and SmartArt cases; they are
   preserved in the report rather than excluded from the score.
5. Exact table border styles and corner-cell styling.
6. Inspect later slides: all-page text/page counts are measured, but raster review
   covers only the first two pages. Conversion success does not imply that later
   slides or every image, object, animation or embedded media item is supported.

The new rich text and image transform behavior is aimed at PDF export. Other
exporters do not yet reproduce all of these properties. Keep the corpus and its
known mismatches as the starting point for the next fidelity pass.
