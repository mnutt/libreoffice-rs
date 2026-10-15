# XLSX preview development corpus

This opt-in corpus compares three independent paths using 24 original upstream inputs, 20 expanded inputs, and ten authored probes:

- **candidate:** this fork's `lo_calc::from_xlsx_bytes` and `to_pdf_preview`;
- **office2pdf:** Davros's existing packaged converter (currently office2pdf 0.8.1 with Davros patches);
- **libreoffice:** the installed real LibreOffice application.

The manifest pins documents from office2pdf's public fixture suite at a full
Git revision, with SHA-256 and size checks. Cases cover styles, dimensions,
merges, number formats, 1900/1904 dates, cached formulas, hidden sheets, headers,
charts, images, conditional formatting, print titles, and multi-page tables.
Selection precedes the fixes; low-scoring fixtures remain in the corpus.
Downloaded inputs and generated artifacts are ignored. Upstream fixture notices
are retained under `licenses/`; source attribution is recorded by the manifest.
The suite is a deliberately selected development sample, not a representative
measure of all spreadsheets.

## Run

Requirements: Python 3, Rust, Bubblewrap, Poppler (`pdfinfo`, `pdftotext`,
`pdfimages`, `pdftoppm`), ImageMagick (`magick`), and real LibreOffice (`soffice`).
Build Davros's normal runtime bundle first; it supplies the office2pdf binary,
Liberation/Noto fonts, and fontconfig configuration.

From this fork's root:

```sh
cargo build -p lo_calc --example xlsx-preview
python3 tests/xlsx_corpus/run.py fetch
python3 tests/xlsx_corpus/test_runner.py
python3 tests/xlsx_corpus/run.py evaluate \
  --bundle /path/to/davros-preview/output/native-preview \
  --output tests/xlsx_corpus/runs/initial
```

Output directories must be new. `--ids ID ...` selects cases, `--timeout` sets a
per-converter deadline (default 60 seconds), and `--preview-pages` controls the
number of raster previews (default two). All pages are inspected for dimensions
and extracted text. `--candidate PATH` selects another compiled candidate.

After changing the candidate, rebuild and reuse the unchanged reference stages:

```sh
cargo build -p lo_calc --example xlsx-preview
python3 tests/xlsx_corpus/run.py evaluate \
  --bundle /path/to/davros-preview/output/native-preview \
  --references tests/xlsx_corpus/runs/initial \
  --output tests/xlsx_corpus/runs/next
```

Reuse requires matching manifest, baseline executable and font hashes. Reused
stages are labeled with their original directory; their timings describe the
original run. Omit `--references` to regenerate references after upgrading real
LibreOffice. The report records executable/source hashes and tool versions.
Debug/release builds and sandbox startup affect timing, so these single runs are
not comparative performance benchmarks.

Each conversion runs in a fresh network-isolated Bubblewrap namespace with a
read-only input, executable and font bundle, private writable scratch, 4 GiB
address-space and 128 MiB file limits. LibreOffice also uses a private profile
with macros disabled. The runner fails if user namespaces are unavailable; it
does not silently run reference tools without isolation. The example executable
itself is only a corpus adapter and does not enforce production sandboxing.

## Read the results

`report.html` contains three-column previews, full PDFs, logs, and per-case
metrics. `results.json` is machine-readable. Inspect the full PDFs when a case
has more than two pages.

Word-multiset recall/precision detect missing and added text without depending on
reading order. They are not visual-fidelity scores: a picture-only failure may
still score 100%, and PDF text extraction can include clipped text or concatenate
adjacent cells. Image-object counts and input drawing/chart/media counts help
identify those gaps, but don't measure vector-chart correctness.

Raster RMSE is provided only when paired pages have identical reported geometry,
only for previewed pages, and without resizing one document to match the other.
Lower is closer, but font rasterization and whitespace dominate this metric. A
missing score means incomparable geometry, not a perfect match.

`to_pdf_preview` widens the sheet at natural column widths and retains vertical
pagination, matching Davros's preview policy. `to_pdf` retains source paper size
and tiles horizontally, honoring explicit scale and fit-to-page settings. Both
paths honor repeated print titles and horizontal centering. Single rectangular print areas and manual page breaks are honored.
Disjoint print areas remain unsupported.
Real LibreOffice follows its own print settings and font substitutions; it is an
independent comparison, not an authoritative Microsoft Excel export. Differences
between the two reference renderers are useful evidence, not automatic failures.

## Initial implementation

The XLSX importer now populates the shared grid layout: per-sheet paper geometry,
independent margins, explicit/default row and column dimensions, hidden dimensions
and sheets, merges, source font sizes/styles, solid fills, theme/RGB colors,
per-edge borders, alignment, wrapping, printed headings/gridlines, and basic
header/footer fields. Missing `sheetFormatPr` uses Normal-font metrics instead of
fixed dimensions. Declared widths use SpreadsheetML pixel rounding with the Normal font’s maximum
digit advance. Font substitution still changes metrics when the requested face
is unavailable.

Formula expressions remain in the workbook model, while previews use the file's
saved results. This includes shared formula cells, string results and errors.
Missing caches render blank with a diagnostic rather than recalculating through
the limited formula evaluator. Saving a workbook with updated caches requires an
actual spreadsheet application; stale saved results remain stale.

The formatter supports common decimals, grouping, percentages, currency literals,
scientific notation, positive/negative/zero sections, common date/time formats,
elapsed hours, and both Excel date systems (including serial 60). Fractions and
value-conditional number formats fall back to unformatted values. Locale calendars,
accounting alignment, indexed number-format colors and fractional seconds remain
incomplete. Named section colors, such as negative `[Red]` amounts, are honored.
Built-in date format 14 uses `m/d/yy`; locale-dependent reference exports may use a
four-digit year.

Regressions are in `crates/lo_calc/tests/xlsx_preview.rs` and the formatter's unit
tests in `lo_core`. `expectations.json` adds feature-specific PDF checks: image
presence, retained chart pages and labels, budget drawing text, and date-system
controls. Evaluation returns nonzero on a failed expectation or candidate
conversion and preserves the report. These checks supplement visual review;
they do not certify chart geometry or font fidelity.

See `FEATURES.md` for the current milestone and remaining limitations.
`BASELINE.md` and `baseline.json` preserve the historical grid-only milestone.

## Drawing and conditional-format coverage

The importer follows internal drawing relationships and resolves absolute,
one-cell and two-cell anchors using visible row/column dimensions and EMU offsets.
It imports embedded PNG/JPEG pictures and source crops, drawing text, cached bar/line/scatter charts,
and drawing-only chart sheets. Drawing bounds participate in pagination; preview
width includes drawings, avoiding the cropped picture in the office2pdf reference.
No new runtime dependency was added.
For charts without caches, local references retain only requested values and known
blanks. Per-workbook limits allow 100,000 requested/output positions and 2,000,000
scanned cells; interrupted scans do not publish partial caches. Worksheet XML is
still parsed as a whole, so this is not streaming XML import.

Charts use cached indexed points (including gaps), numeric scatter coordinates,
series/point colors from the workbook theme, clustered and stacked bars (including
separate positive/negative accumulators), basic 2-D pies, named series, legends, data labels,
number formats, axis titles and explicit value-axis bounds. Linear bar gradients
retain source stops through bounded vector subdivision. Pie legends use category
labels; slice proportions and the source start angle are preserved. Scatter series with no line remain
unconnected. These are preview charts: combination/percent-stacked/logarithmic charts,
exploded/negative-value pies, radial/per-point gradients, full source plot layout, marker families beyond circle/square, smoothing, and chart style extensions are not
complete. Unsupported chart families are not approximated as a different type.

Conditional rules support scalar `cellIs` and simple comparison expressions,
relative/absolute cell references, priority and `stopIfTrue`, differential font
(including size), borders, number formats and solid-fill styles. Data bars support numeric, automatic min/max, percentage, percentile and saved
scalar-reference bounds, and honor minimum/maximum lengths (defaults 10%/90%).
Constant automatic ranges retain their minimum bar length. Conditional number
formats use saved numeric values and the workbook date system. Automatic rows
are remeasured after conditional styling; explicit and hidden row heights remain
authoritative. Checked references and overflow-safe interpolation cover malformed
addresses and extreme finite thresholds.
No formulas are recalculated. The
budget's `showValue=0` is honored (office2pdf displays the hidden value).
Two- and three-color scales support numeric, min/max, percentage and percentile
thresholds, plus saved scalar reference thresholds. Three-arrow (color and gray),
three-traffic-light and three-flag icon sets render as vectors and honor threshold
inclusivity, reversal, priority and hidden values. Text reserves room for the icon.
Other icon families, custom icons, complex expressions and enhanced negative
data-bar axes remain unsupported. Rule processing is bounded.

Cell styling also includes the complete indexed palette, underline, indentation,
shrink-to-fit, and rotation through 90 degrees in either direction. Bundled source fonts are measured and subset-embedded from the same face. Inline
and shared-string rich runs retain family, size, bold, italic, underline and color,
with word wrapping across run boundaries, automatic row heights, and uniform
shrink-to-fit when wrapping is disabled. Advanced
shaping, vertical stacked text, rotated rich text and table-style
overrides still need broader coverage.

## Authored probes and release measurements

`generated.json` pins ten deterministic probes from `generate.py`: font variants,
print areas, manual breaks, inline/shared and shrink-to-fit rich text, stacked bars, image crops, constant-range data bars, and conditional number formats/borders.
They are authored in this repository and carry its MIT license. The generator hash
and ZIP hashes are pinned; the ZIP timestamps are fixed. Generate and compare them:

```sh
python3 tests/xlsx_corpus/run.py fetch --manifest tests/xlsx_corpus/generated.json
python3 tests/xlsx_corpus/run.py evaluate \
  --manifest tests/xlsx_corpus/generated.json \
  --bundle /path/to/preserved-baseline-bundle \
  --output tests/xlsx_corpus/runs/authored --preview-pages 10
```

Visual feature checks count color pixels at a fixed raster size and check PDF font
variants, in addition to text, page count and image-object checks. For example, the
crop probe must retain blue and remove red; the stacked chart must contain both
series colors. These are semantic feature checks, not pixel-equivalence claims.

Build and measure a release candidate with the same profile used by Davros:

```sh
CARGO_PROFILE_RELEASE_OPT_LEVEL=s CARGO_PROFILE_RELEASE_LTO=thin \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_STRIP=symbols \
CARGO_PROFILE_RELEASE_PANIC=abort cargo build --release -p lo_calc --example xlsx-preview
python3 tests/xlsx_corpus/benchmark.py \
  --candidate target/release/examples/xlsx-preview \
  --bundle /path/to/preserved-baseline-bundle \
  --output tests/xlsx_corpus/runs/benchmark --repetitions 5
```

Each engine gets one warmup, followed by alternating measured runs per document.
A `wait4` supervisor inside Bubblewrap records elapsed time and converter peak RSS.
The candidate is an XLSX-only adapter; the baseline is the complete Davros converter
and additionally installs its own hardening. Binary sizes therefore cover different
capabilities. Preserve the old bundle before rebuilding Davros, so its binary/fonts
remain an immutable baseline. `benchmark.json` records raw samples and hashes.

Run the bundled-font regression explicitly (ordinary unit tests do not require fonts):

```sh
DAVROS_PREVIEW_FONTS=/path/to/bundle/fonts cargo test -p lo_core source_fonts
```

For low-disk development, `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_INCREMENTAL=false`
keeps the workspace debug artifacts much smaller; it does not change release builds.

## Expanded corpus

`expanded.json` adds 20 inputs pinned to the same upstream revision: tables,
pivot reports, drawings, chart and print-layout regressions, conditional-format
collections, rich strings, right-to-left sheets, and six upstream-authored business
examples. These are regression/example workbooks, not independently sampled
production documents. The existing fixture license notices apply.

```sh
python3 tests/xlsx_corpus/run.py fetch --manifest tests/xlsx_corpus/expanded.json
python3 tests/xlsx_corpus/run.py evaluate \
  --manifest tests/xlsx_corpus/expanded.json \
  --bundle /path/to/preserved-baseline-bundle \
  --output tests/xlsx_corpus/runs/expanded --preview-pages 10
```

`EXPANDED.md` records findings, fixes, and remaining gaps. `expanded-baseline.json`
and `expanded-results.json` preserve before/after measurements, including failed
reference conversions. The large conditional-format collection has more than ten
pages: text and page geometry cover all pages, but raster review is limited to the
first ten. Use the full PDFs for later pages.

Chart visual regressions can inspect normalized page regions using `regions` in
`expectations.json`. Each region specifies `box: [left, top, right, bottom]` in
0–1 page coordinates plus `min_colors`/`max_colors`. This prevents another chart
elsewhere on the page from satisfying a missing pie's color check. Regions use the
same fixed 600-pixel page raster and are recorded in the raw result JSON.

## Allocation limits and ad hoc overrides

The fork reads these trusted process settings when importing or decoding. Values
must be positive decimal integers without unit suffixes; invalid, zero, negative,
non-Unicode or overflowing values return a configuration error. There is no
unlimited sentinel.

| Environment variable | Default | Meaning |
| --- | ---: | --- |
| `LIBREOFFICE_RS_MAX_ZIP_ENTRY_BYTES` | 67108864 (64 MiB) | Maximum expanded size of one ZIP entry |
| `LIBREOFFICE_RS_MAX_ZIP_TOTAL_BYTES` | 268435456 (256 MiB) | Maximum sum of central-directory expanded sizes |
| `LIBREOFFICE_RS_MAX_INFLATE_BYTES` | 67108864 (64 MiB) | Maximum generic Flate output and PNG scanline bytes |
| `LIBREOFFICE_RS_MAX_PNG_PIXELS` | 16000000 | Maximum decoded PNG pixel count |

ZIP metadata is checked before entry allocation. Inflation cannot exceed the
central-directory size, even when data descriptors are used. PNG inflation cannot
exceed the scanline size calculated from its dimensions, even if the configured
ceiling is higher. The XLSX renderer also honors the configurable pixel ceiling
in its early image check. Invalid or oversized images are omitted by the preview
renderer; the decoding API itself returns an error. Oversized ZIPs fail import.

For example, temporarily permit a larger workbook entry and package:

```sh
LIBREOFFICE_RS_MAX_ZIP_ENTRY_BYTES=134217728 \
LIBREOFFICE_RS_MAX_ZIP_TOTAL_BYTES=536870912 \
target/debug/examples/xlsx-preview input.xlsx output.pdf
```

For larger PNGs, both pixel and scanline limits may need increasing. The corpus
runner explicitly forwards all four settings through its cleared sandbox
environment and records `preview_limit_overrides` in results. An integrating
application must likewise pass them to the renderer. Existing external memory,
CPU and time limits remain independent; these settings do not replace sandboxing
or guarantee a bound on total process memory.

All workspace crates now inherit `unsafe_code = "forbid"` from the root manifest.
This policy covers their library, binary, test and example targets, not third-party
dependencies. Shared A1 parsing rejects column overflow, coordinate serialization
handles maximal `u32` values, and non-ASCII hex colors return the fallback color.
