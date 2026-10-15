# Expanded XLSX corpus: findings and fixes

Twenty additional upstream workbooks bring the total to 54 (24 original, ten
local authored probes, twenty expanded). Selection and checksums are in
`expanded.json`, pinned to office2pdf revision
`2e0ae824ac1b7f49a20e8fedcd019a87941d80c0`. Fourteen are upstream regression/example
files and six are upstream-authored English business examples. They broaden
feature coverage, but are not a representative production sample. The same
checked-in upstream license notices apply.

Renderer fixes: `54aeb62` and `a4b396d`; corpus expansion: `91feb89`.
The [before report](runs/expanded-initial/report.html) and
[after report](runs/nearby-expanded-final/report.html) contain PDFs and previews.
`expanded-baseline.json` and `expanded-results.json` preserve the measurements.

## Defects fixed

**Missing worksheet-backed charts.** `123233_charts` initially produced a nearly
blank page with only footer cells. Two independent problems caused this:

- Its worksheet part is `Sheet1.xml` but its relationship file is
  `sheet1.xml.rels`. XLSX relationship lookup now uses ASCII case equivalence,
  rejecting ambiguous aliases. This follows [OPC part-name equivalence](https://learn.microsoft.com/en-us/dotnet/core/compatibility/core-libraries/8.0/system-io-packaging-case-insensitive-uri).
- Chart series have local worksheet references without embedded caches. Missing
  caches now use saved cells on the referenced sheets, including hidden sheets,
  shared-string labels, and saved formula results. Existing chart caches are
  preserved. Missing values stay gaps. External references, unions, expressions,
  and oversized ranges are not evaluated. Work is bounded across the workbook.

The first fix restored three charts and raised word recall against LibreOffice
from 3.8% to 89.1%. The follow-up adds the fourth (2-D pie), linear gradient bar
fills, and axis titles, raising recall to **97.3%**. Pie slice proportions, start
angle and point colors follow the source; pie legends use category labels and the
saved font size. Axis titles reserve space beside the ticks. Linear gradients
use up to 16 source stops and 64 clipped vector strips per bar, adding no runtime
dependency. Gradient fills previously fell through to the black outline color.

PDF checks now inspect specific normalized page regions: the pie region must
contain blue pixels, and the gradient bars must not be black. The earlier output
has zero blue pixels in the pie region and 6,198 dark pixels in the gradient
region; the corrected output passes both checks. Text checks also require
Revenue/Year titles. This is not full chart parity: axis tick selection, borders,
spacing and source print pagination still differ. Exploded or negative-value pies,
radial gradients, per-point gradients and gradient outlines remain unsupported.
Only ordinary one-series 2-D pies are enabled; other pie families are not
silently converted to this type.

**Missing conditional heatmap.** The regional sales dashboard rendered its
numeric cells white despite a source red/yellow/green scale. Two- and three-color
scales now interpolate RGB colors using min/max, numeric, percentage, percentile,
and supported saved scalar thresholds. The distribution excludes text, booleans,
errors, and missing formula results. Higher-priority fills and stop rules retain
precedence. The dashboard now visually matches the reference heatmap. Word recall
was 100% both before and after, illustrating why text-only checks missed this.
New visible green/rose pixel requirements fail on the original PDF and pass now.
The chart test uses its actual blue/orange palette; its dark green bars do not
meet the bright-green heatmap classifier.

## Conversion and comparison results

Candidate and LibreOffice convert all 54 inputs. Office2pdf converts 51 and fails
on three expanded files. All candidate feature expectations pass. The original
32 cases retain identical text-comparison and raster-RMSE results.

| Expanded case | Candidate word recall vs LO | Candidate / office2pdf / LO pages |
| --- | ---: | --- |
| ExcelTables | 100.0% | 1 / 1 / 2 |
| ExcelPivotTableSample | 84.6% | 3 / 3 / 3 |
| WithDrawing | 90.0% | 1 / 2 / 2 |
| WithTextBox | 100.0% | 1 / 1 / 1 |
| 123233_charts | 97.3% | 1 / 2 / 2 |
| chart_sheet | 100.0% | 2 / 2 / 2 |
| issue_1603_gift_budget | 59.9% | 2 / 2 / 3 |
| issue_1181_fit_to_height | 77.0% | 3 / 3 / 2 |
| richtext-namespaced | 100.0% | 1 / 1 / 2 |
| right-to-left | 88.0% | 2 / 2 / 2 |
| table-multiple | 100.0% | 3 / 3 / 3 |
| ConditionalFormattingSamples | 92.3% | 23 / error / 36 |
| NewStyleConditionalFormattings | 100.0% | 1 / error / 3 |
| conditional_formatting_cell_is | 81.8% | 2 / 2 / 2 |
| 02_financial_model_en | 100.0% | 2 / 2 / 2 |
| 03_inventory_en | 100.0% | 2 / 2 / 4 |
| 05_project_schedule_en | 100.0% | 1 / 1 / 1 |
| 06_sales_dashboard_en | 100.0% | 1 / 1 / 2 |
| 09_expense_report_en | 100.0% | 1 / 1 / 2 |
| 10_kpi_tracker_en | 100.0% | 1 / error / 1 |

The office2pdf failures are retained: `ConditionalFormattingSamples` panics on a
missing archive entry, while `NewStyleConditionalFormattings` and
`10_kpi_tracker_en` panic while parsing icon sets. The candidate converting these
files does not mean it renders all their conditional-format features.

Natural-width preview deliberately differs from LibreOffice paper tiling. For
example, the sales dashboard retains the Total column on one wide page while
LibreOffice puts it on a second page. All pages are inspected for text and
geometry; rasterization covers the first ten pages per fixture. The largest
conditional-format collection has 23 candidate pages and 36 LibreOffice pages,
so its later pages have not received raster review in this batch.

## Remaining findings and next priorities

1. Refine chart tick bounds, borders, legend spacing and source layout. Extend
   coverage beyond basic pies and linear gradients. Fit-to-height and gift-budget
   examples retain chart/layout differences.
2. Extend conditional-format coverage beyond the supported three-arrow, traffic-light
   and flag sets: other/custom icons, enhanced negative data-bar axes, and formula
   rules beyond simple scalar comparisons. Automatic bar bounds are now supported.
3. Investigate gift-budget (59.9% LO recall), conditional cell comparisons (81.8%),
   and pivot-report (84.6%) differences with cell-level and visual checks. Raw
   recall alone does not distinguish layout, clipped text and missing content.
4. Right-to-left sheet layout remains unimplemented. Table-style application also
   remains unsupported; the basic table samples look plain in both candidate and
   LibreOffice, so they do not establish style fidelity.

No cases were removed or existing thresholds relaxed to hide defects. The full workspace all-feature test suite passes, including 36 core tests,
33 Calc unit tests and eighteen XLSX integration tests. Ten runner tests pass,
including saved formula/gap handling, package case ambiguity, numeric percentile
selection, conditional priority, pie proportions, gradient interpolation and region-specific visible-color checks. It does not certify
arbitrary-XLSX replacement readiness. Davros remains uncommitted.

## Conditional icon and automatic-bar follow-up

The KPI tracker now renders its green/up, yellow/right and red/down arrows. A
region-specific raster gate requires all three colors in the icon column; the
previous PDF has zero qualifying pixels there and fails that gate. Automatic
min/max bars now render in the conditional-format samples, with percentage and
percentile bounds also supported. Saved bar lengths and first-match priority are
retained; text and booleans do not contaminate numeric distributions.

Bar lengths follow the [OOXML data-bar definition](https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.spreadsheet.databar?view=openxml-3.0.1),
including its default 10%/90% endpoints. This does not implement enhanced x14
negative axes/colors. Icon shapes are small vector approximations; other families
and custom sets remain unsupported. No runtime dependencies were added.

All 52 candidate conversions and feature gates pass in `icons-bars-original`,
`icons-bars-expanded` and `icons-bars-authored`. Original and authored text
scores are unchanged. The large conditional-format collection changes from
92.32% to 92.15% LO recall and 94.32% to 96.33% precision after hidden values and
icon text insets take effect; these extraction scores do not establish visual
parity. The underlying measurements are retained in the tracked result files.

## Adjacent edge cases and differential styles

`c456ceb` fixes checked conditional references, finite-endpoint interpolation,
constant-range bars, and differential borders, font sizes and number formats.
The new constant-bar fixture was compared with LibreOffice: single and repeated
values use the configured minimum length (10% by default, 20% in the custom case).
Negative-bar colors and gradients still differ from LibreOffice; this change does
not claim enhanced negative-bar fidelity. Its visual gate fails on the prior
candidate, which has no blue bar pixels. A second authored fixture checks visible
red borders and percentage, date and currency output. Saved formula values,
1904 dates, explicit border clearing, rule priority, and automatic versus hidden
row heights have focused Rust regressions.

`dbb53af` retains only requested chart cells, with separate request/output and scan
budgets. A regression locates chart values after 100,001 unrelated cells and then
loads a second chart range; another ensures interrupted scans cannot become
partial caches. XML parsing remains whole-document, and later uncached ranges can
require rescanning a sheet. This reduces retained values and avoids the previous
100,000-cell cutoff, without promising unbounded large-sheet support.

The final `nearby-original-final`, `nearby-expanded-final` and
`nearby-authored-final` runs convert all 54 inputs with zero candidate feature
failures. Existing page counts and text recall are unchanged. Conditional-format
collection precision against LibreOffice rises from 96.33% to 96.79%; other
existing text scores are unchanged. Full workspace all-feature tests and all ten
runner tests pass. No runtime dependencies were added; Davros remains untouched.

## Allocation hardening follow-up

`hardened-env-original`, `hardened-env-expanded` and `hardened-env-authored` record
all 54 conversions with zero feature failures after bounded ZIP/PNG inflation and
shared-parser fixes. Page counts and every recorded comparison metric are
unchanged from the adjacent-edge-case runs. Eleven runner tests pass, including
explicit forwarding of the four configurable allocation limits through the
sandbox. See [README.md](README.md#allocation-limits-and-ad-hoc-overrides).
