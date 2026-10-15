#!/usr/bin/env python3
"""Rebuild the bundled geometry archive from the pinned upstream XML download."""
import hashlib
import json
from pathlib import Path
import sys
import xml.etree.ElementTree as ET
import zipfile

HERE=Path(__file__).resolve().parent
source=Path(sys.argv[1]).read_bytes()
metadata=json.loads((HERE/'preset-shapes-source.json').read_text())
if hashlib.sha256(source).hexdigest()!=metadata['source_sha256']:
    raise SystemExit('Source differs from the pinned upstream XML')
root=ET.fromstring(source)
for shape in root:
    for child in list(shape):
        if child.tag.rsplit('}',1)[-1] not in ('avLst','gdLst','pathLst'):
            shape.remove(child)
for element in root.iter():
    element.tag=element.tag.rsplit('}',1)[-1]
    element.text=None
    element.tail=None
xml=ET.tostring(root)
with zipfile.ZipFile(HERE/'preset-shapes.zip','w',compression=zipfile.ZIP_DEFLATED,compresslevel=9) as archive:
    entry=zipfile.ZipInfo('presets.xml',date_time=(2026,1,1,0,0,0))
    entry.compress_type=zipfile.ZIP_DEFLATED
    archive.writestr(entry,xml)
if hashlib.sha256((HERE/'preset-shapes.zip').read_bytes()).hexdigest()!=metadata['archive_sha256']:
    raise SystemExit('Generated archive differs from the pinned output')
print(f'Reproduced {len(root)} presets; {len(xml)} uncompressed bytes')
