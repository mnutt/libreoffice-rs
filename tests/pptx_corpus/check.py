#!/usr/bin/env python3
"""Check current visual/text expectations against existing PDFs without reconversion."""
import argparse
import hashlib
import json
from pathlib import Path
from run import runner


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('prefix',type=Path,help='Four-shard run prefix, e.g. runs/verified')
    p.add_argument('--output',type=Path,required=True)
    args=p.parse_args()
    path=Path(__file__).parent/'expectations.json'
    expected=json.loads(path.read_text())
    cases=[]
    for i in range(4):
        root=Path(f'{args.prefix}-{i}')
        results=json.loads((root/'results.json').read_text())
        for c in results['cases']:
            exp=expected.get(c['id'],{})
            stage=c['stages']['candidate']
            pdf=root/c['id']/'candidate/output.pdf'
            text=pdf.with_suffix('.txt')
            if stage['status']=='ok':
                if exp.get('min_colors') or exp.get('max_colors'):
                    stage['pdf']['colors']=runner.raster_color_counts(pdf)
                if exp.get('regions'):
                    stage['pdf']['regions']=[runner.raster_color_counts(pdf,r['box']) for r in exp['regions']]
            cases.append(dict(id=c['id'],failures=runner.check_expectations(stage,text.read_text() if text.exists() else '',exp)))
    report=dict(expectations_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),checked=len(cases),failures=sum(len(c['failures']) for c in cases),cases=cases)
    args.output.write_text(json.dumps(report,indent=2)+'\n')
    print(f"{report['checked']} cases; {report['failures']} failures")
    if report['failures']:raise SystemExit(1)


if __name__=='__main__':main()
