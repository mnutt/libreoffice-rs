#!/usr/bin/env python3
"""Compare existing previews, allowing only sub-point PDF page-size rounding.

No images are resized: PNG dimensions must still match exactly. This separate
report does not alter the stricter historical corpus scores.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import re
import statistics
import struct
import subprocess


def png_size(path):
    with path.open('rb') as stream:
        header = stream.read(24)
    if len(header) != 24 or header[:8] != b'\x89PNG\r\n\x1a\n' or header[12:16] != b'IHDR':
        raise ValueError(f'Invalid PNG header: {path}')
    return struct.unpack('>II', header[16:24])


def compatible(a, b):
    return len(a) == len(b) == 2 and all(abs(x-y) <= 0.1 for x,y in zip(a,b))


def compare(case, root):
    row = dict(id=case['id'], pages=[])
    stages = [case['stages'][n] for n in ('candidate', 'libreoffice')]
    if any(s['status'] != 'ok' for s in stages):
        return row
    a,b = [s['pdf'] for s in stages]
    for i,(an,bn) in enumerate(zip(a['previews'],b['previews'])):
        if not compatible(a['page_sizes_pt'][i], b['page_sizes_pt'][i]):
            continue
        ap = root/case['id']/'candidate'/an
        bp = root/case['id']/'libreoffice'/bn
        if png_size(ap) != png_size(bp):
            continue
        result = subprocess.run(['magick','compare','-metric','RMSE',str(ap),str(bp),'null:'],
                                capture_output=True,text=True,timeout=30)
        match = re.search(r'\(([\d.eE+-]+)\)',result.stderr)
        if result.returncode not in (0,1) or not match:
            raise RuntimeError(f'Comparison failed for {case["id"]}: {result.stderr}')
        row['pages'].append(dict(page=i+1,rmse=float(match[1])))
    if row['pages']:
        row['mean_rmse'] = statistics.mean(p['rmse'] for p in row['pages'])
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('prefix',type=Path)
    parser.add_argument('--output',type=Path,required=True)
    args = parser.parse_args()
    work = []
    metadata = []
    for i in range(4):
        root = Path(f'{args.prefix}-{i}')
        result = json.loads((root/'results.json').read_text())
        work.extend((c,root) for c in result['cases'])
        metadata.append({k:v for k,v in result.items() if k not in ('cases','summary')})
    with ThreadPoolExecutor(max_workers=4) as pool:
        rows = list(pool.map(lambda job: compare(*job),work))
    comparable = [r for r in rows if 'mean_rmse' in r]
    summary = dict(decks=len(rows),comparable_decks=len(comparable),
                   comparable_pages=sum(len(r['pages']) for r in rows),
                   mean_deck_rmse=statistics.mean(r['mean_rmse'] for r in comparable) if comparable else None)
    args.output.write_text(json.dumps(dict(pdf_tolerance_pt=0.1,png_policy='identical dimensions; no resampling',
                                         summary=summary,shards=metadata,cases=rows),indent=2)+'\n')
    print(json.dumps(summary,indent=2))


if __name__ == '__main__':
    main()
