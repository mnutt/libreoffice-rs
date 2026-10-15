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

## Reproduce

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
