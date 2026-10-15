# Initial XLSX grid milestone

Historical snapshot. See [FEATURES.md](FEATURES.md) for the subsequent drawing,
chart, conditional-format and pagination milestone.

Implementation: `616bc522bf6ab1d5e770ca22055b423f28d48d26`. Source files match the final report’s recorded source hash.

All 24 documents convert through all three renderers. The candidate has 100%
reference-word recall against real LibreOffice on 18/24 cases. This is text
accounting, not 18 visual passes: pictures, charts and conditional formatting
are still missing. No production switch from office2pdf has been made.

The local full visual report is [verified-grid/report.html](runs/verified-grid/report.html).
The pre-change report is [before/report.html](runs/before/report.html).
These generated artifacts are ignored; `baseline.json` preserves the numerical
comparison and provenance in Git. Follow the README to regenerate the corpus.

| Fixture | Before recall vs LO | After recall vs LO | After recall vs office2pdf | Candidate / office2pdf / LO pages |
| --- | ---: | ---: | ---: | --- |
| poi-Formatting | 67.2% | 98.4% | 98.4% | 1 / 1 / 1 |
| poi-WidthsAndHeights | 100.0% | 100.0% | 100.0% | 1 / 1 / 1 |
| poi-NumberFormatTests | 74.8% | 97.9% | 99.6% | 9 / 9 / 8 |
| poi-WithVariousData | 86.6% | 100.0% | 100.0% | 1 / 1 / 1 |
| poi-WithMoreVariousData | 89.3% | 100.0% | 100.0% | 1 / 1 / 1 |
| merge_cells | 100.0% | 100.0% | 100.0% | 1 / 1 / 1 |
| SH008-Table-With-Tall-Row | 54.5% | 100.0% | 100.0% | 1 / 1 / 1 |
| SH109-CellWithBorder | 100.0% | 100.0% | 100.0% | 1 / 1 / 1 |
| date | 33.3% | 100.0% | 93.3% | 1 / 1 / 1 |
| date_1904 | 33.3% | 100.0% | 66.7% | 1 / 1 / 1 |
| shared_formulas | 100.0% | 100.0% | 100.0% | 1 / 1 / 1 |
| Booleans | 87.5% | 100.0% | 100.0% | 1 / 1 / 1 |
| InlineString | 100.0% | 100.0% | 100.0% | 1 / 1 / 1 |
| GroupTest | 100.0% | 100.0% | 48.0% | 1 / 1 / 1 |
| issue_1065_hidden_sheet_probe | 100.0% | 100.0% | 100.0% | 1 / 1 / 1 |
| Themes | 100.0% | 100.0% | 100.0% | 1 / 1 / 1 |
| WithChart | 37.5% | 37.5% | 37.5% | 1 / 2 / 2 |
| SimpleScatterChart | 11.8% | 11.8% | 28.6% | 1 / 2 / 2 |
| picture | 100.0% | 100.0% | 100.0% | 1 / 1 / 4 |
| poi-WithConditionalFormatting | 94.1% | 100.0% | 88.9% | 1 / 1 / 1 |
| headerFooterTest | 14.3% | 100.0% | 100.0% | 1 / 1 / 1 |
| poi-RepeatingRowsCols | 100.0% | 100.0% | 100.0% | 4 / 4 / 4 |
| simple-monthly-budget | 39.6% | 56.6% | 70.5% | 1 / 1 / 2 |
| 100-customers | 88.2% | 85.9% | 99.8% | 3 / 3 / 3 |

## Findings and next work

1. **Drawing anchors, pictures and chart sheets.** `picture` has one image in
   office2pdf and four image placements in LibreOffice, but none in the candidate,
   despite perfect word recall. `WithChart` and `SimpleScatterChart` lose their
   chart pages. Implement OOXML drawing relationships and one-/two-cell anchors
   before evaluating chart layout against the existing ODF chart model.
2. **Charts and conditional formatting.** The budget template now has recognizable
   layout, styles and formatted values, but still lacks its chart and data bar.
   Conditional-format fixtures remain in the corpus even when their ordinary
   text scores pass. Table styles, text boxes and rich runs also need coverage.
3. **Print settings and pagination.** Print areas, repeated title bands, manual
   breaks, fit-to-page scaling, horizontal centering and precise header/footer
   placement remain incomplete. The candidate produces nine number-format pages
   versus LibreOffice’s eight. Natural-width preview intentionally differs from
   print output; use both reference renderers when diagnosing this.
4. **Fonts, number formats and text metrics.** Default dimensions now follow
   substituted Normal-font metrics. Exact original-font metrics, rich text,
   rotations, underline, shrink-to-fit, fractions, locale-specific formats,
   accounting layout and negative-section colors remain incomplete.

`100-customers` is the one case whose word recall against LibreOffice falls
(88.2% → 85.9%). Its old plain-text output ignored cell geometry; the new grid
is visually closer to office2pdf and reaches 99.8% word recall against it.
LibreOffice clips narrow columns differently, and extraction concatenates or
splits adjacent cell text. The reduction is retained in the results rather than
hidden or treated as proof of a visual regression. Font/width differences remain.

The two date-system controls match LibreOffice’s extracted text while disagreeing
with office2pdf. `GroupTest` also agrees with LibreOffice after hidden dimensions
are honored. These disagreements support retaining both reference engines.

## Validation

- All workspace library tests passed before the final malformed-input additions.
- The final Calc suite passes: 20 unit tests and six XLSX integration regressions.
- `cargo check --workspace --all-targets --all-features` passed.
- Four corpus-runner tests cover manifest safety, checksums and multiset metrics.
- The final candidate rendered all 24 fixtures; its source hash matches the
  committed implementation. Reused reference stages are labeled in the full report.
- No new runtime dependencies were added. Build size and production performance
  have not been benchmarked; the candidate corpus binary is a debug build.
