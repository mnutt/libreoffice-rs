#!/usr/bin/env python3
"""Embed all three renderers' original PNG previews in an offline HTML report."""
import argparse
import base64
from html import escape
import json
from pathlib import Path

from summarize import load


ENGINES = [('candidate', 'libreoffice-rs'), ('libreoffice', 'LibreOffice'),
           ('office2pdf', 'office2pdf')]
STYLE = """
body { font: 15px system-ui,sans-serif; margin: 24px; color: #20252b; background: #f5f6f8 }
header, article { background: white; padding: 20px; margin-bottom: 24px; border: 1px solid #ddd }
h1 { margin-top: 0 } h2 { font-size: 18px; overflow-wrap: anywhere }
.row { display:flex; justify-content:space-between; align-items:flex-start; gap: 0; margin: 12px 0 }
.row > * { width:30%; max-width:30%; box-sizing:border-box; margin:0 }
.labels { font-weight:600; text-align:center }
.row img { height:auto; border:1px solid #aaa; display:block; cursor:zoom-in }
.missing { padding:24px 8px; background:#eee; text-align:center; overflow-wrap:anywhere }
details { margin:16px 0 0 } summary { cursor:pointer } pre { white-space:pre-wrap; overflow-wrap:anywhere }
table { width:100%; border-collapse:collapse; margin-top:12px; table-layout:fixed }
td,th { text-align:left; padding:8px; border-bottom:1px solid #ddd; overflow-wrap:anywhere }
input { padding:10px; width:min(600px,80%); font:inherit }
.toolbar { position:sticky; top:0; background:#f5f6f8; padding:12px 0; z-index:1 }
[hidden] { display:none!important }
dialog { max-width:95vw; max-height:95vh; padding:8px; border:0 }
dialog img { display:block; max-width:90vw; max-height:88vh; object-fit:contain }
dialog::backdrop { background:#000b }
"""


def write_report(prefix, output):
    dataset = load(prefix)  # Reject incomplete or duplicate shards.
    roots = {}
    for shard in range(4):
        root = Path(f'{prefix}-{shard}')
        for case in json.loads((root / 'results.json').read_text())['cases']:
            roots[case['id']] = root
    count = len(dataset['cases'])
    images = pages = 0
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open('w') as out:
        out.write('<!doctype html><html lang="en"><meta charset="utf-8">'
                  '<meta name="viewport" content="width=device-width,initial-scale=1">'
                  '<title>PPTX: three-renderer comparison</title><style>' + STYLE + '</style><body>')
        out.write(f'<header><h1>PPTX: three-renderer comparison</h1><p>{count} distinct presentations. '
                  'LibreOffice is the primary fidelity reference; office2pdf is the existing baseline. '
                  'Each row shows the same PDF page in all three renderers, in that order.</p>'
                  '<p>Up to the first two pages are shown per deck; page counts and text comparisons cover '
                  'the full PDFs. These include deliberately malformed upstream regression fixtures. '
                  'All previews are embedded original PNGs, without resampling. Click an image to enlarge it. '
                  'This file works offline.</p><details><summary>Run provenance</summary><pre>' +
                  escape(json.dumps(dataset['shards'], indent=2)) + '</pre></details></header>'
                  '<div class="toolbar"><label>Filter decks <input id="filter" type="search" '
                  'placeholder="Filename or fixture purpose"></label> <span id="count"></span></div>')
        for case in sorted(dataset['cases'], key=lambda c: c['id']):
            ident = case['id']
            title = ident + ' ' + case.get('purpose', '')
            out.write(f'<article data-search="{escape(title.lower(), quote=True)}">'
                      f'<h2>{escape(ident)}</h2><p>{escape(case.get("purpose", ""))}</p>')
            n = max((len(s.get('pdf', {}).get('previews', [])) for s in case['stages'].values()), default=0)
            for page in range(max(1, n)):
                pages += 1
                out.write(f'<p>Page {page+1}</p><div class="row labels">' +
                          ''.join(f'<span>{label}</span>' for _, label in ENGINES) + '</div><div class="row">')
                for engine, label in ENGINES:
                    stage = case['stages'][engine]
                    previews = stage.get('pdf', {}).get('previews', [])
                    if page < len(previews):
                        path = roots[ident] / ident / engine / previews[page]
                        data = base64.b64encode(path.read_bytes()).decode('ascii')
                        alt = escape(f'{ident}, page {page+1}, {label}', quote=True)
                        out.write(f'<img loading="lazy" decoding="async" alt="{alt}" src="data:image/png;base64,{data}">')
                        images += 1
                    else:
                        reason = stage['status'] if stage['status'] != 'ok' else 'No preview for this page'
                        out.write(f'<div class="missing">{escape(reason)}</div>')
                out.write('</div>')
            out.write('<details><summary>Metrics and diagnostics</summary><table><tr>'
                      '<th>Renderer</th><th>Status</th><th>PDF pages</th><th>PDF bytes</th></tr>')
            for engine, label in ENGINES:
                stage = case['stages'][engine]
                pdf = stage.get('pdf', {})
                out.write(f'<tr><td>{label}</td><td>{escape(stage["status"])}</td>'
                          f'<td>{pdf.get("pages", "—")}</td><td>{pdf.get("bytes", "—")}</td></tr>')
            out.write('</table><p>Raster RMSE: lower is better; null means not comparable under the '
                      'strict page-dimension check, not a perfect match. Recall and precision measure '
                      'extracted text, not visual fidelity.</p><pre>' + escape(json.dumps(case, indent=2)) +
                      '</pre></details></article>')
        out.write('''<dialog id="zoom"><button id="close">Close</button><img alt="Enlarged preview"></dialog>
<script>
const cards = [...document.querySelectorAll('article')];
const filter = document.querySelector('#filter');
function update() {
  const query = filter.value.toLowerCase().trim();
  let shown = 0;
  for (const card of cards) { card.hidden = !card.dataset.search.includes(query); shown += !card.hidden; }
  document.querySelector('#count').textContent = `${shown} / ${cards.length} decks`;
}
filter.addEventListener('input', update); update();
const zoom = document.querySelector('#zoom');
document.addEventListener('click', event => {
  if (event.target.matches('article img')) {
    zoom.querySelector('img').src = event.target.src;
    zoom.querySelector('img').alt = event.target.alt;
    zoom.showModal();
  }
});
document.querySelector('#close').onclick = () => zoom.close();
zoom.addEventListener('close', () => zoom.querySelector('img').removeAttribute('src'));
</script></body></html>''')
    return dict(decks=count, page_rows=pages, embedded_images=images, bytes=output.stat().st_size)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('prefix', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(write_report(args.prefix, args.output), indent=2))
