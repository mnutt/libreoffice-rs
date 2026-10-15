#!/usr/bin/env python3
"""Fail on paired per-page fidelity regressions; never average away a bad deck."""
import argparse
import json
from pathlib import Path
from summarize import load


def assess(before, after, before_audit, after_audit, pixel_delta=0.01, text_delta=0.005):
    old = {c['id']: c for c in before['cases']}
    new = {c['id']: c for c in after['cases']}
    failures = []
    if old.keys() != new.keys():
        failures.append(dict(kind='coverage', missing=sorted(old.keys()-new.keys()), added=sorted(new.keys()-old.keys())))
    for ident in sorted(old.keys() & new.keys()):
        a,b = old[ident],new[ident]
        x,y = a['stages']['candidate'],b['stages']['candidate']
        if x['status'] != y['status'] and y['status'] != 'ok':
            failures.append(dict(id=ident, kind='status', before=x['status'], after=y['status']))
        if b.get('expectation_failures'):
            failures.append(dict(id=ident, kind='expectations', details=b['expectation_failures']))
        if x['status'] == y['status'] == 'ok':
            if x['pdf']['pages'] != y['pdf']['pages']:
                failures.append(dict(id=ident, kind='page_count', before=x['pdf']['pages'], after=y['pdf']['pages']))
            if x['pdf']['page_sizes_pt'] != y['pdf']['page_sizes_pt']:
                failures.append(dict(id=ident, kind='page_geometry'))
        for metric in ('recall','precision'):
            x = a.get('comparisons',{}).get('candidate_vs_libreoffice',{}).get(metric)
            y = b.get('comparisons',{}).get('candidate_vs_libreoffice',{}).get(metric)
            if x is not None and y is not None and x-y > text_delta:
                failures.append(dict(id=ident, kind=metric, before=x, after=y))
    pixels = lambda audit: {(c['id'],p['page']):p['rmse'] for c in audit['cases'] for p in c['pages']}
    a,b = pixels(before_audit), pixels(after_audit)
    for ident,page in sorted(a.keys()-b.keys()):
        failures.append(dict(id=ident,page=page,kind='lost_visual_comparison'))
    for ident,page in sorted(a.keys() & b.keys()):
        if b[ident,page]-a[ident,page] > pixel_delta:
            failures.append(dict(id=ident,page=page,kind='pixels',before=a[ident,page],after=b[ident,page]))
    return dict(paired_pages=len(a.keys() & b.keys()), newly_compared_pages=len(b.keys()-a.keys()),
                pixel_delta=pixel_delta,text_delta=text_delta,failures=failures)


if __name__ == '__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--baseline',type=Path,required=True)
    p.add_argument('--candidate',type=Path,required=True)
    p.add_argument('--baseline-audit',type=Path,required=True)
    p.add_argument('--candidate-audit',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    args=p.parse_args()
    report=assess(load(args.baseline),load(args.candidate),json.loads(args.baseline_audit.read_text()),json.loads(args.candidate_audit.read_text()))
    args.output.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))
    raise SystemExit(bool(report['failures']))
