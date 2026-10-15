#!/usr/bin/env python3
"""Merge four complete comparison shards and summarize paired fidelity changes."""
import argparse
import collections
import json
import statistics
from pathlib import Path


def load(prefix):
    shards = [json.loads(Path(f'{prefix}-{i}/results.json').read_text()) for i in range(4)]
    cases = [c for shard in shards for c in shard['cases']]
    expected = sum(len(json.loads((Path(__file__).parent / f'shard-{i}.json').read_text())['fixtures']) for i in range(4))
    if len(cases) != expected or len({c['id'] for c in cases}) != expected:
        raise ValueError('All four complete, distinct shards are required')
    return dict(shards=[{k:v for k,v in s.items() if k not in ('cases','summary')} for s in shards], cases=cases)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--baseline', type=Path, required=True)
    p.add_argument('--candidate', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    baseline, candidate = load(args.baseline), load(args.candidate)
    before = {c['id']:c for c in baseline['cases']}
    summary = {'unique_decks':len(candidate['cases'])}
    summary['statuses'] = {engine:dict(collections.Counter(c['stages'][engine]['status'] for c in candidate['cases'])) for engine in ('candidate','office2pdf','libreoffice')}
    summary['expectation_failures'] = sum(len(c['expectation_failures']) for c in candidate['cases'])
    summary['matching_page_dimensions'] = {}
    for name, dataset in [('baseline',baseline),('candidate',candidate)]:
        summary['matching_page_dimensions'][name] = sum(c['stages']['candidate'].get('pdf',{}).get('page_sizes_pt') == c['stages']['libreoffice'].get('pdf',{}).get('page_sizes_pt') for c in dataset['cases'] if c['stages']['candidate']['status']=='ok' and c['stages']['libreoffice']['status']=='ok')
    deltas=[]
    for c in candidate['cases']:
        old=before[c['id']]['comparisons'].get('candidate_vs_libreoffice',{})
        new=c['comparisons'].get('candidate_vs_libreoffice',{})
        row={'id':c['id']}
        if old.get('recall') is not None and new.get('recall') is not None:
            row.update(recall_before=old['recall'],recall_after=new['recall'])
        pairs=[(a,b) for a,b in zip(old.get('raster_rmse',[]),new.get('raster_rmse',[])) if a is not None and b is not None]
        if pairs:
            row.update(rmse_before=statistics.mean(a for a,b in pairs),rmse_after=statistics.mean(b for a,b in pairs))
        deltas.append(row)
    for metric in ('recall','rmse'):
        rows=[r for r in deltas if metric+'_before' in r]
        summary[metric]={'paired_decks':len(rows),'mean_before':statistics.mean(r[metric+'_before'] for r in rows) if rows else None,'mean_after':statistics.mean(r[metric+'_after'] for r in rows) if rows else None}
        sign=1 if metric=='recall' else -1
        summary[metric]['improved_over_0_01']=sum(sign*(r[metric+'_after']-r[metric+'_before'])>0.01 for r in rows)
        summary[metric]['regressed_over_0_01']=sum(sign*(r[metric+'_after']-r[metric+'_before']) < -0.01 for r in rows)
    args.output.write_text(json.dumps(dict(summary=summary,baseline=baseline,candidate=candidate,deltas=deltas),indent=2)+'\n')
    print(json.dumps(summary,indent=2))


if __name__ == '__main__':
    main()
