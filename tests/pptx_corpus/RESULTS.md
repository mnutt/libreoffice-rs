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

## Remaining work, in priority order

1. SmartArt layout and cached drawing fallbacks. Several SmartArt fixtures still
   produce blank or incomplete diagrams.
2. Preset/custom vector geometry, connectors, line dashes/arrowheads, gradient
   clipping, background-fill inheritance, and shape/group rotation. Most preset
   shapes currently fall back to rectangles; group transforms cover scale and
   translation, not rotation/flips.
3. Text columns, hanging bullets, automatic fit calculation, vertical text,
   per-character fallback/shaping, superscripts and more exact font baselines.
4. Full chart axis/legend/layout semantics, hierarchical category placement in the
   shared engine, and advanced chart types. The remaining text regressions are
   mostly chart ticks/labels, plus complex text and SmartArt cases; they are
   preserved in the report rather than excluded from the score.
5. Exact table border styles, theme line-reference widths and corner-cell styling.
6. Inspect later slides: all-page text/page counts are measured, but raster review
   covers only the first two pages. Conversion success does not imply that later
   slides or every image, object, animation or embedded media item is supported.

The new rich text and image transform behavior is aimed at PDF export. Other
exporters do not yet reproduce all of these properties. Keep the corpus and its
known mismatches as the starting point for the next fidelity pass.
