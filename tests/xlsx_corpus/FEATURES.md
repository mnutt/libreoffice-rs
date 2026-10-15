# XLSX fidelity milestone

Implementation: `a4b396d` on `davros-fixes`. This is progress toward a replacement,
not a claim of complete Excel fidelity or a production renderer switch.

`features.json` records the 24 upstream fixtures; `authored-results.json` records
eight deterministic authored probes. `expanded-results.json` adds 20 upstream
regression/example workbooks; see [EXPANDED.md](EXPANDED.md). Reports include source and binary hashes,
font configuration, reference provenance, and feature expectations. PDFs, PNGs,
and HTML comparisons remain local ignored artifacts under `runs/`. The latest
[original](runs/rich-shrink-originals/report.html) and
[authored](runs/rich-shrink-final/report.html) reports show all three engines.
`baseline.json` preserves the original grid-only measurements.

## Supported and tested improvements

- Bundled source fonts are measured and subset-embedded from the same face,
  including real bold/italic variants. Distinct Unicode characters that share a
  glyph retain distinct PDF text mappings. No host font discovery is required.
- Column widths use SpreadsheetML pixel rounding and the Normal font's maximum
  digit advance. Wrapped row heights use actual font and indentation metrics.
- Inline and shared rich text preserve size, family, bold, italic, underline,
  color, words spanning style boundaries, and automatic row height. Shrink-to-fit
  scales all runs uniformly when wrapping is disabled. A visible blue tail in the
  new probe catches clipping that a text-extraction check would miss; the previous
  renderer fails this check and the current one passes.
- Single rectangular print areas, manual breaks, repeated print titles, source
  scale, fit-to-page, and horizontal centering are retained. Ordinary previews
  widen naturally rather than printing horizontal tiles; chart sheets fit paper.
- Internal PNG/JPEG pictures, drawing text, and chart sheets contribute to bounds.
  Source image crops clip the expanded image to its destination frame. The crop
  probe requires visible blue pixels and zero red pixels. External images are not
  fetched.
- Absent chart caches can be filled from bounded local worksheet references,
  using saved values even on hidden sheets. Existing caches remain authoritative;
  external and expression references are not evaluated. Relationship lookup
  matches ASCII case and rejects ambiguous aliases.
- Cached line, clustered/stacked bar, scatter, and basic 2-D pie charts retain indexed gaps,
  numeric scatter coordinates, colors, titles, labels, legends, axis bounds,
  selected markers, axis titles, and source gap width. Pie legends use categories
  and slices retain source proportions, start angles and point colors. Linear bar
  gradients preserve their stops through bounded vector subdivision. Stacks accumulate positive and negative
  values separately. Automatic value axes leave headroom at exact tick maxima.
- Scalar conditional comparisons, relative references, priorities/stop rules,
  differential fonts/fills, and cached numeric/named data bars are supported.
  Two- and three-color scales support min/max, percentage and percentile bounds.
  Data bars use automatic min/max, percentage/percentile thresholds and saved
  minimum/maximum lengths. Three-arrow (color/gray), traffic-light and flag icons
  honor reversal, inclusive/exclusive thresholds, priority and hidden values.
  Hidden bar values stay hidden. Indexed colors, negative number-section colors,
  indentation, underline, plain-text rotation and shrink-to-fit are retained.

No new runtime dependency was added. Formulas use saved results without
recalculation; missing or stale caches remain a limitation.

## Comparison results

All 52 inputs convert with the candidate and real LibreOffice; office2pdf fails
on three expanded inputs. All candidate feature expectations pass. Twenty of the original 24 cases have 100%
word recall against LibreOffice; that is not twenty visual-fidelity passes.

| Case | Candidate recall vs LO | Candidate recall vs office2pdf | Candidate / office2pdf / LO pages |
| --- | ---: | ---: | --- |
| WithChart | 100% | 100% | 2 / 2 / 2 |
| SimpleScatterChart | 100% | 64.3% | 2 / 2 / 2 |
| picture | 100% | 100% | 1 / 1 / 4 |
| simple-monthly-budget | 71.7% | 99.2% | 2 / 1 / 2 |
| 100-customers | 86.0% | 97.8% | 3 / 3 / 3 |

The candidate shows the complete picture; office2pdf clips it and LibreOffice
prints four tiles. The budget's missing office2pdf value, `2336`, is explicitly
hidden by the source. Scatter labels and coordinates agree better with
LibreOffice than with office2pdf. Authored cropped-image and rich-text cases
also expose reference-renderer defects; neither reference is an oracle.

The customer-sheet word score against office2pdf fell from 99.8% to 97.8% after
pixel width rounding. Before/after extracted non-whitespace character counts are
identical: different spacing joins email suffixes and adjacent ages into different
tokens. This does not establish visual equivalence. Narrow cells visibly clip,
and PDF extraction still includes clipped characters. Font substitution and
layout differences remain visible in the images; the raw scores are retained.
Other imperfect LO comparisons are `poi-Formatting` (98.4%, locale-dependent
year formatting), `poi-NumberFormatTests` (97.9%, font/layout/extraction), and
the budget. Pixel comparisons improve for some cases and worsen for others.

## Release benchmark

`benchmark.json` preserves measurements of implementation `581f7ae`, immediately
before rich-text shrink-to-fit, across the original 24 fixtures. Each engine gets
one warmup and five measured runs per document, alternating order under identical
Bubblewrap limits and using the same font bundle. The release profile is `opt-level=s`,
thin LTO, one codegen unit, stripped symbols, and abort-on-panic.

| Measurement | XLSX-only candidate | Full Davros office2pdf baseline |
| --- | ---: | ---: |
| Executable bytes | 1,261,096 | 40,932,816 |
| Sum of per-document median elapsed time | 0.212 s | 1.114 s |
| Highest per-document median peak RSS | 13.8 MiB | 158.5 MiB |

The aggregate time is about 5.3 times lower in this sample. This is not a universal
speed claim: the picture case is slower (46.8 ms versus 35.8 ms). Executable sizes
cover different capabilities; the baseline supports other document formats and
also installs its own hardening. Shared font assets are excluded from both
executable sizes. These measurements do not establish full-production bundle
size or arbitrary-document throughput.

## Remaining parity work

- Font substitution, line metrics, chart plot/legend geometry and locale defaults
  still differ. Missing source fonts cannot be reconstructed from a workbook.
- Omitted paper size defaults to Letter here and A4 in the installed LibreOffice.
  Number-format output has nine pages versus LO's eight; office2pdf also has nine.
- Disjoint print areas, rotated images, rotated rich text, superscript/subscript,
  strike-through, advanced shaping, and table-style overrides need work.
- Combination/percent-stacked/logarithmic charts, more marker shapes, smoothing,
  complex conditional formulas, other/custom icon sets, and negative data-bar axes
  remain unsupported or partial.
- The 52 cases are a development sample, not representative coverage of arbitrary
  Excel files. Feature gates complement manual inspection; they do not certify
  equivalent pixels, pagination, or typography.

## Validation

The affected Calc tests pass (30 unit and fourteen XLSX integration tests), as do ten
corpus-runner tests. The full workspace all-feature suite also passes after the icon/data-bar
changes (36 core tests). The source-font embedding regression was
also run with the bundled fonts. All pages are inspected for text and geometry; the first ten are rasterized per
fixture. Provenance stays attached to each measurement.

The fork changes are published on `davros-fixes`. Davros integration changes remain
uncommitted, and its optional renderer pin is an earlier fork milestone.
