#!/usr/bin/env python3
"""Measure release converters with identical corpus/fonts/Bubblewrap limits.

A wait4 supervisor reports converter peak RSS and wall time inside the sandbox. This is
warm-cache, process-per-document work; it is not an in-process throughput test.
"""
import argparse
import json
from pathlib import Path
import statistics
import subprocess
import run

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--candidate', type=Path, required=True)
    p.add_argument('--bundle', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--repetitions', type=int, default=5)
    args = p.parse_args()
    if not 3 <= args.repetitions <= 20: p.error('use 3–20 measured repetitions')
    output = args.output.resolve(); output.mkdir(parents=True, exist_ok=False)
    bundle = args.bundle.resolve(); candidate = args.candidate.resolve()
    manifest = json.loads((run.HERE/'manifest.json').read_text())
    run.validate_manifest(manifest)
    result = dict(candidate_sha256=run.digest(candidate), baseline_sha256=run.digest(bundle/'davros-preview'),
                  manifest_sha256=run.digest(run.HERE/'manifest.json'),
                  fonts={f.name:run.digest(f) for f in sorted((bundle/'fonts').glob('*'))},
                  rustc=subprocess.check_output(['rustc','--version'],text=True).strip(),
                  repetitions=args.repetitions, methodology='One unmeasured warmup per engine/document, alternating engine order, wait4 resource accounting inside identical Bubblewrap limits; process-per-document. Candidate is a release XLSX-only adapter; baseline is the full Davros executable and additionally installs its own hardening.',
                  binary_bytes={'candidate':candidate.stat().st_size,'office2pdf':(bundle/'davros-preview').stat().st_size}, cases=[])
    for fixture in manifest['fixtures']:
        source = run.HERE/'files'/fixture['file']; run.verify(source, fixture)
        samples = {engine:[] for engine in ['candidate','office2pdf']}
        for trial in range(args.repetitions + 1):
            engines = [('candidate',candidate),('office2pdf',bundle/'davros-preview')]
            if trial % 2: engines.reverse()
            for engine, executable in engines:
                work = output/fixture['id']/engine/str(trial);work.mkdir(parents=True)
                command = run.sandbox(source, work, bundle, executable)
                (work/'measure.py').write_text("""import os, time, json, sys
start = time.perf_counter()
pid = os.fork()
if pid == 0:
    os.execv(sys.argv[1], sys.argv[1:])
_, status, usage = os.wait4(pid, 0)
with open('/work/usage.json', 'w') as f:
    json.dump(dict(seconds=time.perf_counter()-start, peak_rss_kib=usage.ru_maxrss), f)
sys.exit(os.waitstatus_to_exitcode(status))
""")
                command[-3:] = ['/usr/bin/python3','/work/measure.py']+command[-3:]
                stage = run.run(command, work, 60)
                if stage['exit_code'] or stage['timeout']: raise RuntimeError((fixture['id'], engine, stage))
                if not (work/'output.pdf').read_bytes().startswith(b'%PDF-'): raise RuntimeError('invalid PDF')
                if trial: samples[engine].append(json.loads((work/'usage.json').read_text()))
        result['cases'].append(dict(id=fixture['id'], samples=samples, medians={engine:{key:statistics.median(s[key] for s in values) for key in ['seconds','peak_rss_kib']} for engine,values in samples.items()}))
        print(fixture['id'], result['cases'][-1]['medians'], flush=True)
    result['summary'] = {engine:dict(sum_document_median_seconds=sum(c['medians'][engine]['seconds'] for c in result['cases']), max_document_median_rss_kib=max(c['medians'][engine]['peak_rss_kib'] for c in result['cases'])) for engine in ['candidate','office2pdf']}
    (output/'results.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result['summary'],indent=2))
if __name__ == '__main__': main()
