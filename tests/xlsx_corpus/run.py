#!/usr/bin/env python3
"""Pinned XLSX corpus: compare candidate, Davros/office2pdf, and real LibreOffice."""
import argparse
from collections import Counter
import hashlib
import html
import json
import os
from pathlib import Path
import re
import resource
import signal
import shutil
import subprocess
import time
import unicodedata
import urllib.parse
import urllib.request
import zipfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(path, record):
    if path.stat().st_size != record['size'] or digest(path) != record['sha256']:
        raise ValueError(f'Changed fixture/notice: {path}')


def validate_manifest(manifest):
    if not re.fullmatch(r'https://github.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', manifest['repository']):
        raise ValueError('Invalid repository')
    if not re.fullmatch(r'[a-f0-9]{40}', manifest['revision']):
        raise ValueError('Source revision must be a full commit')
    ids = [f['id'] for f in manifest['fixtures']]
    if len(ids) != len(set(ids)):
        raise ValueError('Duplicate fixture IDs')
    for f in manifest['fixtures']:
        if not re.fullmatch(r'[A-Za-z0-9_.-]+', f['id']):
            raise ValueError('Unsafe fixture ID')
    for group in ['fixtures', 'notices']:
        names = [f['file'] for f in manifest[group]]
        if len(names) != len(set(names)):
            raise ValueError('Duplicate local names')
        for f in manifest[group]:
            if Path(f['file']).name != f['file'] or f['file'] in ('.', '..'):
                raise ValueError('Unsafe local filename')
            if Path(f['path']).is_absolute() or '..' in Path(f['path']).parts:
                raise ValueError('Unsafe source path')
            if not re.fullmatch(r'[a-f0-9]{64}', f['sha256']) or not 0 < f['size'] <= 32 * 1024**2:
                raise ValueError('Invalid checksum/size pin')


def fetch(manifest):
    if manifest.get('generated_by'):
        if manifest['generated_by'] != 'generate.py' or digest(HERE/'generate.py') != manifest['generator_sha256']:
            raise ValueError('Generated corpus does not match its pinned generator')
        subprocess.run(['python3', str(HERE/'generate.py')], check=True)
    for group, folder in [('fixtures', 'files'), ('notices', 'licenses')]:
        for record in manifest[group]:
            target = HERE / folder / record['file']
            if not target.exists():
                repo = manifest['repository'].removeprefix('https://github.com/')
                url = f"https://raw.githubusercontent.com/{repo}/{manifest['revision']}/{urllib.parse.quote(record['path'])}"
                with urllib.request.urlopen(url, timeout=60) as response:
                    data = response.read(record['size'] + 1)
                if len(data) != record['size'] or hashlib.sha256(data).hexdigest() != record['sha256']:
                    raise ValueError(f'Checksum mismatch: {url}')
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
            verify(target, record)
    print(f"Verified {len(manifest['fixtures'])} XLSX fixtures and notices")


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (4 * 1024**3,) * 2)
    resource.setrlimit(resource.RLIMIT_FSIZE, (128 * 1024**2,) * 2)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def run(command, directory, timeout):
    started = time.monotonic()
    with (directory / 'stdout.log').open('wb') as out, (directory / 'stderr.log').open('wb') as err:
        child = subprocess.Popen([str(x) for x in command], stdout=out, stderr=err,
                                 start_new_session=True, preexec_fn=limits)
        expired = False
        try:
            child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            expired = True
        finally:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
    return dict(exit_code=child.returncode, timeout=expired,
                seconds=round(time.monotonic() - started, 3),
                stderr=(directory / 'stderr.log').read_text(errors='replace')[-3000:])


def sandbox(source, work, bundle, executable=None):
    cmd = ['bwrap', '--die-with-parent', '--unshare-net', '--unshare-pid',
           '--ro-bind', '/usr', '/usr', '--ro-bind', '/etc', '/etc']
    for path in ['/bin', '/sbin', '/lib', '/lib64']:
        if Path(path).exists():
            cmd += ['--ro-bind', path, path]
    cmd += ['--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
            '--ro-bind', str(source), '/input.xlsx', '--ro-bind', str(bundle), '/bundle',
            '--bind', str(work), '/work', '--chdir', '/work', '--clearenv',
            '--setenv', 'PATH', '/usr/bin', '--setenv', 'HOME', '/work',
            '--setenv', 'LANG', 'C.UTF-8', '--setenv', 'TMPDIR', '/work',
            '--setenv', 'XDG_CACHE_HOME', '/work/cache',
            '--setenv', 'DAVROS_PREVIEW_FONTS', '/bundle/fonts',
            '--setenv', 'FONTCONFIG_FILE', '/bundle/fonts.conf',
            '--setenv', 'RAYON_NUM_THREADS', '2']
    if executable:
        return cmd + ['--ro-bind', str(executable), '/converter', '/converter', '/input.xlsx', '/work/output.pdf']
    profile = work / 'profile/user'
    profile.mkdir(parents=True)
    (profile / 'registrymodifications.xcu').write_text('''<?xml version="1.0"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
</oor:items>''')
    return cmd + ['--setenv', 'SAL_USE_VCLPLUGIN', 'svp', '/usr/bin/soffice',
                  '-env:UserInstallation=file:///work/profile', '--headless', '--nologo',
                  '--nodefault', '--norestore', '--convert-to', 'pdf', '--outdir', '/work', '/input.xlsx']


def raster_color_counts(pdf):
    """Small deterministic semantic probes, independent of font antialiasing."""
    data = subprocess.check_output(['pdftoppm', '-f', '1', '-singlefile', '-scale-to', '600', str(pdf)], timeout=30)
    match = re.match(rb'P6\s+(\d+)\s+(\d+)\s+255\s', data)
    if not match:
        raise ValueError('Unexpected PPM header')
    pixels = data[match.end():]
    if len(pixels) != int(match[1]) * int(match[2]) * 3:
        raise ValueError('Unexpected PPM dimensions')
    colors = dict(red=0, blue=0, orange=0, green=0, rose=0)
    for i in range(0, len(pixels), 3):
        r,g,b = pixels[i:i+3]
        colors['red'] += r > 180 and g < 100 and b < 100
        colors['blue'] += b > 140 and r < 100 and g < 170
        colors['orange'] += r > 180 and 80 < g < 180 and b < 100
        colors['green'] += g > 140 and r < 140 and b < 160
        colors['rose'] += r > 200 and g < 150 and b < 160
    return colors

def inspect_pdf(pdf, previews):
    if not pdf.is_file() or pdf.read_bytes()[:5] != b'%PDF-':
        raise ValueError('No valid PDF output')
    info = subprocess.check_output(['pdfinfo', str(pdf)], text=True, timeout=30)
    pages = int(re.search(r'^Pages:\s+(\d+)', info, re.M)[1])
    info = subprocess.check_output(['pdfinfo', '-f', '1', '-l', str(pages), str(pdf)], text=True, timeout=30)
    sizes = [[float(w), float(h)] for w, h in re.findall(r'Page\s+\d+\s+size:\s+([\d.]+) x ([\d.]+)', info)]
    text = subprocess.check_output(['pdftotext', '-layout', str(pdf), '-'], text=True, timeout=30)
    pdf.with_suffix('.txt').write_text(text)
    images = []
    for page in range(1, min(pages, previews) + 1):
        prefix = pdf.parent / f'page-{page}'
        subprocess.run(['pdftoppm', '-r', '72', '-scale-to', '1600', '-f', str(page), '-l', str(page),
                        '-singlefile', '-png', str(pdf), str(prefix)], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=30)
        images.append(prefix.with_suffix('.png').name)
    if len(sizes) != pages:
        raise ValueError('Incomplete PDF page geometry')
    image_list = subprocess.check_output(['pdfimages', '-list', str(pdf)], text=True, timeout=30)
    image_count = len(re.findall(r'^\s*\d+\s+\d+\s+image\s', image_list, re.M))
    return dict(pages=pages, page_sizes_pt=sizes, bytes=pdf.stat().st_size,
                image_count=image_count, previews=images)


def compare_text(actual, reference):
    def words(path):
        return Counter(re.findall(r'\w+', unicodedata.normalize('NFKC', path.read_text()).casefold()))
    a, r = words(actual), words(reference)
    matched = sum((a & r).values())
    return dict(recall=matched / sum(r.values()) if r else None,
                precision=matched / sum(a.values()) if a else None,
                missing=(r-a).most_common(20), extra=(a-r).most_common(20))


def check_expectations(stage, text, expected):
    failures = []
    if stage['status'] != 'ok':
        return ['candidate did not produce a valid PDF']
    for key, metric in [('min_images', 'image_count'), ('min_pages', 'pages')]:
        if stage['pdf'][metric] < expected.get(key, 0):
            failures.append(f'{metric}: {stage["pdf"][metric]} < {expected[key]}')
    if 'pages' in expected and stage['pdf']['pages'] != expected['pages']:
        failures.append(f'pages: {stage["pdf"]["pages"]} != {expected["pages"]}')
    for color, minimum in expected.get('min_colors', {}).items():
        if stage['pdf'].get('colors', {}).get(color, 0) < minimum:
            failures.append(f'missing visible {color} pixels')
    for color, maximum in expected.get('max_colors', {}).items():
        if stage['pdf'].get('colors', {}).get(color, 0) > maximum:
            failures.append(f'unexpected visible {color} pixels')
    for font in expected.get('fonts', []):
        if not any(name.endswith(font) for name in stage['pdf'].get('font_names', [])):
            failures.append(f'missing font variant: {font}')
    normalize = lambda s: ' '.join(unicodedata.normalize('NFKC', s).casefold().split())
    for phrase in expected.get('absent_text', []):
        if normalize(phrase) in normalize(text):
            failures.append(f'unexpected text: {phrase}')
    pages = text.split('\f')
    for phrase in expected.get('text', []):
        if normalize(phrase) not in normalize(text):
            failures.append(f'missing text: {phrase}')
    for page, phrases in expected.get('page_text', {}).items():
        actual = pages[int(page)-1] if int(page) <= len(pages) else ''
        for phrase in phrases:
            if normalize(phrase) not in normalize(actual):
                failures.append(f'page {page} missing text: {phrase}')
    return failures


def report(results, output):
    rows, sections = [], []
    for case in results['cases']:
        rows.append('<tr><td><a href="#{0}">{0}</a></td><td>{1}</td>{2}</tr>'.format(
            html.escape(case['id']), html.escape(case['purpose']), ''.join(
                f'<td>{html.escape(stage["status"])} / {stage.get("pdf", {}).get("pages", "—")}</td>'
                for stage in case['stages'].values())))
        panels = []
        for name, stage in case['stages'].items():
            path = f'{case["id"]}/{name}'
            images = ''.join(f'<a href="{path}/{p}"><img loading="lazy" src="{path}/{p}"></a>'
                             for p in stage.get('pdf', {}).get('previews', []))
            panels.append(f'<article><h3>{name}</h3><a href="{path}/output.pdf">PDF</a> · '
                          f'<a href="{path}/stderr.log">log</a>{images}</article>')
        sections.append(f'<section id="{case["id"]}"><h2>{case["id"]}</h2><div class="panels">'
                        + ''.join(panels) + '</div><pre>' + html.escape(json.dumps(case, indent=2)) + '</pre></section>')
    (output / 'report.html').write_text('''<!doctype html><meta charset="utf-8"><title>XLSX renderer comparison</title>
<style>body{font:15px system-ui;margin:24px;background:#eee}table{border-collapse:collapse}td,th{border:1px solid #aaa;padding:6px}section{margin:40px 0}.panels{display:flex;gap:12px}article{flex:1;min-width:0}img{width:100%;display:block;margin:12px 0;background:white}pre{white-space:pre-wrap;overflow-wrap:anywhere;font-size:12px}</style>
<h1>XLSX renderer comparison</h1><p>Selectable text recall/precision ignore order and layout. Raster RMSE is reported only for matching page geometry, for the previewed pages; it is not a fidelity pass. LibreOffice is an independent reference, not Microsoft Excel. Davros intentionally widens sheets instead of printing horizontal page tiles. Open full PDFs to inspect later pages.</p>
''' + '<pre>' + html.escape(json.dumps(results['summary'], indent=2)) + '</pre><table><tr><th>Case</th><th>Focus</th><th>Candidate status/pages</th><th>Office2PDF status/pages</th><th>LibreOffice status/pages</th></tr>' + ''.join(rows) + '</table>' + ''.join(sections))


def evaluate(args, manifest):
    bundle, candidate, output = args.bundle.resolve(), args.candidate.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    fixtures = [f for f in manifest['fixtures'] if not args.ids or f['id'] in args.ids]
    if not fixtures:
        raise ValueError('No selected fixtures')
    for f in fixtures:
        verify(HERE / 'files' / f['file'], f)
    for n in manifest['notices']:
        verify(HERE / 'licenses' / n['file'], n)
    # Fail loudly if isolation is unavailable, rather than recording every document as broken.
    subprocess.run(['bwrap', '--unshare-net', '--ro-bind', '/usr', '/usr', '--symlink', 'usr/lib', '/lib',
                    '--symlink', 'usr/lib64', '/lib64', '/usr/bin/true'], check=True)
    expectations = json.loads((HERE/"expectations.json").read_text())
    references = None
    if args.references:
        references = json.loads((args.references/'results.json').read_text())
        if references['manifest_sha256'] != digest(args.manifest) or references['office2pdf_sha256'] != digest(bundle/'davros-preview'):
            raise ValueError('Reference run uses different corpus or baseline binary')
        fonts={p.name:digest(p) for p in sorted((bundle/'fonts').glob('*'))}
        if references.get('fontconfig_sha256') != digest(bundle/'fonts.conf'):
            raise ValueError('Reference run has different or unpinned font configuration')
        if references['fonts'] != fonts:
            raise ValueError('Reference run uses different fonts')
        references={case['id']:case for case in references['cases']}
    source_hash=hashlib.sha256()
    for path in sorted((ROOT/'crates').rglob('*')):
        if path.suffix in ('.rs','.toml'):
            source_hash.update(str(path.relative_to(ROOT)).encode()+b'\0'+path.read_bytes()+b'\0')
    results = dict(schema_version=1, checkout_source_sha256=source_hash.hexdigest(), manifest_sha256=digest(args.manifest),
                   runner_sha256=digest(Path(__file__)), candidate_sha256=digest(candidate),
                   expectations_sha256=digest(HERE/"expectations.json"),
                   office2pdf_sha256=digest(bundle / 'davros-preview'),
                   fontconfig_sha256=digest(bundle/'fonts.conf'),
                   fonts={p.name: digest(p) for p in sorted((bundle/'fonts').glob('*'))},
                   libreoffice_version=subprocess.check_output(['soffice', '--version'], text=True).strip(),
                   checkout_tracked_diff_sha256=hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','crates'],cwd=ROOT)).hexdigest(),
                   checkout_revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                   cases=[])
    for fixture in fixtures:
        source = HERE / 'files' / fixture['file']
        with zipfile.ZipFile(source) as archive:
            members=archive.namelist()
        case = dict(id=fixture['id'], purpose=fixture['purpose'], stages={}, comparisons={},
                    input_parts={kind:sum(bool(re.fullmatch(pattern,p)) for p in members)
                                 for kind,pattern in [('charts',r'xl/charts/chart[0-9]+\.xml'),
                                                      ('drawings',r'xl/drawings/drawing[0-9]+\.xml'),
                                                      ('media',r'xl/media/[^/]+')]})
        for name, binary in [('candidate', candidate), ('office2pdf', bundle/'davros-preview'), ('libreoffice', None)]:
            work = output / fixture['id'] / name
            work.mkdir(parents=True)
            pdf = work / 'output.pdf'
            if references is not None and name != 'candidate':
                stage = dict(references[fixture['id']]['stages'][name])
                previous = args.references.resolve()/fixture['id']/name
                stage['reused_from'] = str(previous)
                for filename in ['output.pdf', 'stdout.log', 'stderr.log']:
                    if (previous/filename).exists():
                        shutil.copyfile(previous/filename,work/filename)
            else:
                stage = run(sandbox(source, work, bundle, binary), work, args.timeout)
                if name == 'libreoffice' and (work/'input.pdf').exists():
                    (work/'input.pdf').rename(pdf)
                stage['status'] = 'timeout' if stage['timeout'] else 'error' if stage['exit_code'] else 'ok'
            if stage['status'] == 'ok':
                try:
                    stage['pdf'] = inspect_pdf(pdf, args.preview_pages)
                    expected = expectations.get(fixture['id'], {})
                    if expected.get('min_colors') or expected.get('max_colors'):
                        stage['pdf']['colors'] = raster_color_counts(pdf)
                    if expected.get('fonts'):
                        font_listing = subprocess.check_output(['pdffonts', str(pdf)], text=True, timeout=30)
                        stage['pdf']['font_names'] = [line.split()[0] for line in font_listing.splitlines()[2:] if line.strip()]
                except (ValueError, subprocess.SubprocessError, OSError) as error:
                    stage.update(status='invalid_pdf', inspection_error=str(error))
            case['stages'][name] = stage
        for a, r in [('candidate', 'office2pdf'), ('candidate', 'libreoffice'), ('office2pdf', 'libreoffice')]:
            if any(case['stages'][n]['status'] != 'ok' for n in [a, r]):
                continue
            dirs = [output/fixture['id']/n for n in [a, r]]
            comparison = compare_text(*(d/'output.txt' for d in dirs))
            comparison['raster_rmse'] = []
            am, rm = [case['stages'][n]['pdf'] for n in [a, r]]
            for i in range(min(len(am['previews']), len(rm['previews']))):
                score = None
                if am['page_sizes_pt'][i] == rm['page_sizes_pt'][i]:
                    result = subprocess.run(['magick', 'compare', '-metric', 'RMSE',
                                             str(dirs[0]/am['previews'][i]), str(dirs[1]/rm['previews'][i]), 'null:'],
                                            capture_output=True, text=True, timeout=30)
                    match = re.search(r'\(([\d.eE+-]+)\)', result.stderr)
                    if result.returncode in (0, 1) and match:
                        score = float(match[1])
                comparison['raster_rmse'].append(score)
            case['comparisons'][a+'_vs_'+r] = comparison
        text_file = output/fixture['id']/'candidate'/'output.txt'
        case['expectation_failures'] = check_expectations(case['stages']['candidate'],
            text_file.read_text() if text_file.exists() else '', expectations.get(fixture['id'], {}))
        results['cases'].append(case)
        print(fixture['id'], ', '.join(f'{k}={v["status"]}' for k,v in case['stages'].items()), flush=True)
        results['summary'] = {name: dict(Counter(c['stages'][name]['status'] for c in results['cases']))
                              for name in ['candidate','office2pdf','libreoffice']}
        results['summary']['expectation_failures'] = sum(len(c['expectation_failures']) for c in results['cases'])
        (output/'results.json').write_text(json.dumps(results, indent=2)+'\n')
        report(results, output)
    print(output/'report.html')
    if results['summary']['expectation_failures']:
        raise SystemExit('Corpus feature expectations failed; see report.html')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    fetch_parser = commands.add_parser('fetch')
    fetch_parser.add_argument('--manifest', type=Path, default=HERE/'manifest.json')
    run = commands.add_parser('evaluate')
    run.add_argument('--manifest', type=Path, default=HERE/'manifest.json')
    run.add_argument('--candidate', type=Path, default=ROOT/'target/debug/examples/xlsx-preview')
    run.add_argument('--bundle', type=Path, required=True, help='Davros runtime bundle with office2pdf and fonts')
    run.add_argument('--output', type=Path, required=True, help='new output directory')
    run.add_argument('--ids', nargs='+')
    run.add_argument('--references', type=Path, help='reuse Office2PDF/LibreOffice stages from a matching prior run')
    run.add_argument('--timeout', type=int, default=60)
    run.add_argument('--preview-pages', type=int, default=2)
    args=parser.parse_args()
    manifest=json.loads(args.manifest.read_text())
    validate_manifest(manifest)
    if args.command == 'fetch': fetch(manifest)
    else: evaluate(args, manifest)

if __name__ == '__main__':
    main()
