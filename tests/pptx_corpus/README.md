# PPTX comparison corpus

This corpus compares the fork's native PPTX PDF exporter with desktop LibreOffice
and the preserved Davros office2pdf renderer. It contains **704 pinned upstream
fixtures, representing 627 byte-distinct presentations**. The 77 aliases remain
in the inventory but are evaluated only once.

Sources:

| Source | Fixtures | Upstream directory |
| --- | ---: | --- |
| [LibreOffice](https://github.com/LibreOffice/core) | 433 | `sd/qa/unit/data` |
| [Apache POI](https://github.com/apache/poi) | 95 | `test-data/slideshow` |
| [python-pptx](https://github.com/scanny/python-pptx) | 67 | repository fixtures |
| [office2pdf](https://github.com/developer0hye/office2pdf) | 109 | `tests/fixtures/pptx` |

`sources.json` pins source commits, paths, byte sizes and Git blob hashes.
`manifest.json` adds download SHA-256 hashes and source-license records.
`collect.py` downloads all PPTX fixtures at those pins up to 32 MiB per file,
verifies each blob, deduplicates by content, and writes four deterministic shards.
Original source license notices are in `licenses/`; the original repositories
retain fixture provenance. Files and generated render directories are ignored.
These are regression fixtures, including intentionally damaged packages, rather
than a representative sample of everyday presentations.

## Eighth pass

The all-page pass at renderer commit `cb3300a` checks all 627 decks, with 614
successful conversions and 13 expected parser errors. The unchanged regression
gate reports no findings across 1,272 comparable pages in 579 decks. Mean
per-deck image RMSE decreases from 0.0845693 to 0.0755636 against `sixth-all`.
This is a regression result, not a claim of rendering parity.

`eighth-pass-results.json` retains the per-document comparisons and engine/font
provenance; `eighth-visual-audit.json` contains per-page scores.
`eighth-regressions.json`, `eighth-negative-controls.json`, and
`eighth-validation.json` record the gate, old-output controls, test counts,
candidate hash, and release size. All 54 XLSX regression PDFs remain byte-identical.
The standalone report is generated locally, with all three engines aligned and
metadata below the images. `picture-bullet-inventory.json` identifies the next
eight distinct presentations to examine; remaining areas are in `fidelity-plan.json`.

## Reproduce

The repeatable build/test/comparison cycle is:

```sh
python3 tests/pptx_corpus/loop.py
python3 tests/pptx_corpus/loop.py --ids lo-3columns-f28ebf
```

The environment has a saved approval prefix for this exact Python script. It
uses no shell command strings, performs no downloads or artifact deletion, builds an
immutable candidate, and runs pinned inputs inside Bubblewrap with networking
disabled and resource limits. Fresh timestamped run directories contain logs,
results and PDFs. A full cycle writes the self-contained report and fails on
individual per-page pixel regressions, text regressions, changed page geometry,
lost visual comparability, or feature failures. The thresholds are recorded in
the output; they are review triggers, not proof of parity. Focused runs apply the same gate to the selected fixtures and write their
visual audit and regression findings alongside the PDFs.

Completed byte-identical candidate artifacts are shared through hard links under
an exclusive lock. In-progress conversions are never compacted, and all paths and bytes
are retained. `--resume loop-TIMESTAMP` repeats audit/report/gate stages using the
saved immutable candidate, so quota interruptions do not require conversion again.
For a focused resume, supply the original `--ids` as well.
`--reuse-build loop-TIMESTAMP` starts a fresh evaluation using that loop's frozen
executable and original test/build logs. Use it to broaden a successful focused
run without rebuilding the same code. It does **not** build the current workspace;
`reused-build.json` records the original directory and executable hash.

The default regression baseline is `sixth-all`, with all 1,272 comparable pages
recorded in `sixth-visual-audit.json`. Baseline render artifacts must be available
under `runs/sixth-all-0` through `runs/sixth-all-3` on the comparison machine.

`--preview-pages 0` renders every page. All-page audits retain the existing
0.1-point PDF rounding allowance and require identical raster dimensions.
`fidelity-plan.json` tracks the remaining milestones; `feature-inventory.json`
lists the pinned slide fixtures exercising common DrawingML features.

To create an offline, self-contained three-renderer report from four completed
shards:

```sh
python3 tests/pptx_corpus/standalone_report.py tests/pptx_corpus/runs/fifth-final \
  --output /tmp/pptx-three-renderers.html
```

The report embeds original PNG previews in aligned rows, ordered libreoffice-rs,
LibreOffice, office2pdf, each capped at 30% width. Metrics and diagnostics appear
below the images in expandable sections. Filtering and image enlargement work
offline. The complete corpus report is approximately 186 MB; generated reports
should not be committed.

```sh
python3 tests/pptx_corpus/collect.py
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_INCREMENTAL=false \
  cargo build --offline -p lo_impress --example pptx-preview
cp target/debug/examples/pptx-preview /tmp/pptx-candidate
python3 tests/pptx_corpus/run.py evaluate \
  --manifest tests/pptx_corpus/shard-0.json \
  --candidate /tmp/pptx-candidate \
  --bundle /path/to/preserved-office2pdf-bundle \
  --output tests/pptx_corpus/runs/current-0 \
  --preview-pages 2 --timeout 20
```

Repeat for shards 1–3. The bundle must contain `davros-preview`, `fonts/`, and
`fonts.conf`. Install desktop LibreOffice, Bubblewrap, Poppler utilities, and
ImageMagick. Keep the candidate executable immutable while a run is active.
Use `--references runs/previous-0` to reuse a completed reference run with the
same manifest, fonts and office2pdf binary. A fresh output directory is required.

The shared runner uses process/network isolation, a 4 GiB address-space limit,
a **256 MiB per-file limit**, and a per-conversion timeout. It records conversion
errors and timeouts separately. The four `LIBREOFFICE_RS_MAX_*` allocation settings
documented in `../xlsx_corpus/README.md` are forwarded into the sandbox. GIF/BMP
decoding also uses the existing pixel and inflated-byte ceilings; animated GIFs
render their first frame. These limits are separate from the file-size limit.

```sh
python3 tests/pptx_corpus/summarize.py \
  --baseline tests/pptx_corpus/runs/initial \
  --candidate tests/pptx_corpus/runs/current \
  --output tests/pptx_corpus/results.json
python3 tests/pptx_corpus/check.py tests/pptx_corpus/runs/current \
  --output tests/pptx_corpus/feature-checks.json
```

`check.py` can re-evaluate text and color gates on existing PDFs without repeating
conversion. The main evaluator also applies these gates. Explicit expected parser
errors require exit status 1 and the recorded diagnostic; a panic, signal, timeout,
or different error cannot satisfy an expected rejection.

## Interpreting the comparison

Text recall counts matching normalized words across **all pages**. It does not
measure placement, typography or drawing fidelity. PDFs and HTML contact sheets
are retained for visual review. Raster comparisons cover **only the first two
pages**, at up to 1600 pixels, and are computed only when page dimensions match
exactly. Blank reference text has no recall score. Missing/failed references are
not treated as successful matches. Do not interpret conversion success as parity.

The old importer used different page dimensions from every LibreOffice reference,
so a corpus-wide before/after pixel-error average is unavailable. Text recall,
page geometry, selected visual probes, and direct inspection provide complementary
evidence. `RESULTS.md` documents improvements and remaining limitations.

For broader raster review without changing the historical exact-page-size scores:

```sh
python3 tests/pptx_corpus/visual_audit.py tests/pptx_corpus/runs/second-final \
  --output tests/pptx_corpus/second-visual-audit.json
```

This accepts PDF dimension differences of at most 0.1 point, but still requires
identical PNG dimensions and never resizes images. The second-pass audit covers
579 decks / 710 pages instead of the 239 decks with exactly equal PDF dimensions.
Its averages use a different cohort and must not be compared directly with the
historical exact-size averages. It exposed missing interlaced PNGs and ignored
layout color-map overrides among the largest remaining differences.
