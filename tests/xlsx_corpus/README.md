# XLSX preview development corpus

This opt-in corpus compares three independent paths using the same 24 inputs:

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
and tiles horizontally. Neither yet implements full Excel print scaling/areas.
Real LibreOffice follows its own print settings and font substitutions; it is an
independent comparison, not an authoritative Microsoft Excel export. Differences
between the two reference renderers are useful evidence, not automatic failures.

## Initial implementation

The XLSX importer now populates the shared grid layout: per-sheet paper geometry,
independent margins, explicit/default row and column dimensions, hidden dimensions
and sheets, merges, source font sizes/styles, solid fills, theme/RGB colors,
per-edge borders, alignment, wrapping, printed headings/gridlines, and basic
header/footer fields. Missing `sheetFormatPr` uses Normal-font metrics instead of
fixed dimensions. Widths and font substitution remain approximate.

Formula expressions remain in the workbook model, while previews use the file's
saved results. This includes shared formula cells, string results and errors.
Missing caches render blank with a diagnostic rather than recalculating through
the limited formula evaluator. Saving a workbook with updated caches requires an
actual spreadsheet application; stale saved results remain stale.

The formatter supports common decimals, grouping, percentages, currency literals,
scientific notation, positive/negative/zero sections, common date/time formats,
elapsed hours, and both Excel date systems (including serial 60). Fractions and
value-conditional number formats fall back to unformatted values. Locale calendars,
accounting alignment, per-section colors and fractional seconds remain incomplete.
Built-in date format 14 uses `m/d/yy`; locale-dependent reference exports may use a
four-digit year.

Regressions are in `crates/lo_calc/tests/xlsx_preview.rs` and the formatter's unit
tests. Corpus failures remain diagnostic; they are not replaced with broad
"conversion succeeds" assertions. See `BASELINE.md` for measurements and the next
work items.
