#!/usr/bin/env python3
"""One repeatable PPTX build/test/evaluate/audit cycle, with internal log files.

Approve the prefix `python3 tests/pptx_corpus/loop.py` once. No shell commands,
downloads, commits or pushes are performed by this harness. Byte-identical
completed candidate artifacts are hardlinked, preserving their paths and data.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import hashlib
import fcntl
import os
from pathlib import Path
import shutil
import subprocess
import sys
import uuid

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def compact_completed_candidates(runs):
    """Share identical immutable outputs; preserve all bytes and paths."""
    with (runs/'.compact.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        return _compact_completed_candidates(runs)


def _compact_completed_candidates(runs):
    seen, inodes = {}, {}
    saved = 0
    for path in sorted(runs.glob('*/**/candidate/*')):
        if path.suffix not in ('.pdf', '.png', '.txt') or not path.is_file():
            continue
        # Never touch an in-progress conversion or rasterization.
        if not (path.parent.parent.parent/'results.json').is_file():
            continue
        stat = path.stat()
        identity = (stat.st_dev,stat.st_ino)
        key = inodes.get(identity)
        if key is None:
            key = (stat.st_size,hashlib.sha256(path.read_bytes()).digest())
            inodes[identity] = key
        previous = seen.get(key)
        if previous is None:
            seen[key] = path
            continue
        if previous.stat().st_ino == stat.st_ino:
            continue
        temporary = path.with_name(path.name+'.dedup-'+uuid.uuid4().hex)
        try:
            os.link(previous,temporary)
            os.replace(temporary,path)
        finally:
            temporary.unlink(missing_ok=True)
        if stat.st_nlink == 1:
            saved += stat.st_size
    return saved


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ids', nargs='+', help='Optional focused fixture IDs, automatically assigned to shards')
    parser.add_argument('--baseline', default='sixth-all', help='Run prefix within runs/')
    parser.add_argument('--baseline-audit', default='sixth-visual-audit.json', help='Audit filename within this corpus directory')
    parser.add_argument('--resume', help='Resume audit/report/gate for an existing loop directory; retain its immutable candidate')
    args = parser.parse_args()
    for name in (args.baseline,args.baseline_audit,args.resume or 'unused'):
        if Path(name).name != name or name in ('.','..'):
            parser.error('Baseline names must be simple filenames')
    manifests = [json.loads((HERE/f'shard-{i}.json').read_text()) for i in range(4)]
    known = {f['id'] for m in manifests for f in m['fixtures']}
    if args.ids and set(args.ids)-known:
        parser.error('Unknown fixture IDs: '+', '.join(sorted(set(args.ids)-known)))
    saved = compact_completed_candidates(HERE/'runs')
    if saved:
        print(f'Shared identical completed candidates: {saved/1024**2:.1f} MiB reclaimed',flush=True)
    if shutil.disk_usage(HERE).free < 1024**3:
        parser.error('Need at least 1 GiB free space for an all-page cycle')
    stamp = datetime.now(timezone.utc).strftime('loop-%Y%m%dT%H%M%S%f')
    root = HERE/'runs'/(args.resume or stamp)
    if args.resume:
        if not args.resume.startswith('loop-') or not (root/'pptx-preview').is_file():
            parser.error('Resume requires an existing loop directory and immutable candidate')
    else:
        root.mkdir(parents=True,exist_ok=False)
    print(f'Cycle logs: {root}',flush=True)
    env = dict(os.environ,CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_DEV_INCREMENTAL='false')
    def command(argv, log):
        with (root/log).open('wb') as stream:
            subprocess.run(argv,cwd=ROOT,env=env,stdout=stream,stderr=subprocess.STDOUT,check=True)
    candidate = root/'pptx-preview'
    if not args.resume:
        command(['cargo','test','--offline','--workspace','--all-features'],'tests.log')
        command(['cargo','build','--offline','-p','lo_impress','--example','pptx-preview'],'build.log')
        shutil.copyfile(ROOT/'target/debug/examples/pptx-preview',candidate)
        candidate.chmod(0o755)
    print(f'Immutable candidate and logs: {root}',flush=True)
    def shard(i):
        ids = [f['id'] for f in manifests[i]['fixtures'] if not args.ids or f['id'] in args.ids]
        if not ids:return
        argv=[sys.executable,str(HERE/'run.py'),'evaluate',
              '--manifest',str(HERE/f'shard-{i}.json'),'--candidate',str(candidate),
              '--bundle','/tmp/davros-office2pdf-baseline',
              '--references',str(HERE/f'runs/{args.baseline}-{i}'),
              '--output',str(root/f'candidate-{i}'),'--preview-pages','0','--timeout','20']
        if args.ids:argv+=['--ids',*ids]
        command(argv,f'shard-{i}.log')
        print(f'Shard {i} passed feature checks',flush=True)
    if not args.resume:
        with ThreadPoolExecutor(max_workers=4) as pool:
            list(pool.map(shard,range(4)))
    if args.ids:
        # Apply the same paired gate to focused runs; a successful conversion
        # alone must not be mistaken for a fidelity pass.
        from regression_gate import assess
        from visual_audit import compare
        selected = set(args.ids)
        before, after, work = [], [], []
        for i in range(4):
            baseline = json.loads((HERE/f'runs/{args.baseline}-{i}/results.json').read_text())
            before.extend(c for c in baseline['cases'] if c['id'] in selected)
            result = root/f'candidate-{i}/results.json'
            if result.exists():
                cases = json.loads(result.read_text())['cases']
                after.extend(cases)
                work.extend((c,result.parent) for c in cases)
        old_audit = json.loads((HERE/args.baseline_audit).read_text())
        old_audit['cases'] = [c for c in old_audit['cases'] if c['id'] in selected]
        with ThreadPoolExecutor(max_workers=4) as pool:
            audit = {'cases': list(pool.map(lambda item: compare(*item),work))}
        (root/'visual-audit.json').write_text(json.dumps(audit,indent=2)+'\n')
        gate = assess({'cases': before},{'cases': after},old_audit,audit)
        (root/'regressions.json').write_text(json.dumps(gate,indent=2)+'\n')
        print(f"Focused comparison: {gate['paired_pages']} paired pages, {len(gate['failures'])} regressions. Evidence: {root}",flush=True)
        raise SystemExit(bool(gate['failures']))
    command([sys.executable,str(HERE/'visual_audit.py'),str(root/'candidate'),'--output',str(root/'visual-audit.json')],'audit.log')
    command([sys.executable,str(HERE/'summarize.py'),'--baseline',str(HERE/'runs'/args.baseline),
             '--candidate',str(root/'candidate'),'--output',str(root/'comparison.json')],'summary.log')
    # Produce the review artifact even when the regression gate subsequently fails.
    command([sys.executable,str(HERE/'standalone_report.py'),str(root/'candidate'),
             '--output',str(root/'report.html')],'report.log')
    command([sys.executable,str(HERE/'regression_gate.py'),'--baseline',str(HERE/'runs'/args.baseline),
             '--candidate',str(root/'candidate'),'--baseline-audit',str(HERE/args.baseline_audit),
             '--candidate-audit',str(root/'visual-audit.json'),'--output',str(root/'regressions.json')],'gate.log')
    print(f'Full cycle passed. Report: {root / "report.html"}',flush=True)


if __name__ == '__main__':main()
