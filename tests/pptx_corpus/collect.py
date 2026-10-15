#!/usr/bin/env python3
"""Collect all <=32 MiB PPTX fixtures from pinned upstream regression directories."""
import concurrent.futures, hashlib, json, re, time, urllib.request, zipfile
from pathlib import Path
HERE=Path(__file__).resolve().parent
SOURCES=[
 ('poi','apache/poi','trunk','test-data/slideshow',['legal/LICENSE','legal/NOTICE']),
 ('python','scanny/python-pptx','master','',['LICENSE']),
 ('lo','LibreOffice/core','master','sd/qa/unit/data',['COPYING','COPYING.MPL','COPYING.LGPL']),
 ('office','developer0hye/office2pdf','2e0ae824ac1b7f49a20e8fedcd019a87941d80c0','tests/fixtures/pptx',['LICENSE']),
]
def get(url):
 for attempt in range(4):
  try:
   with urllib.request.urlopen(urllib.request.Request(url,headers={'User-Agent':'libreoffice-rs-corpus'}),timeout=30) as r:return r.read()
  except Exception:
   if attempt==3:raise
   time.sleep(attempt+1)
def api(path):return json.loads(get('https://api.github.com/'+path))
def main():
 pins_path=HERE/'sources.json'
 if pins_path.exists(): pins=json.loads(pins_path.read_text())
 else:
  pins=[]
  for prefix,repo,ref,folder,licenses in SOURCES:
   revision=ref if re.fullmatch('[a-f0-9]{40}',ref) else api(f'repos/{repo}/commits/{ref}')['sha']
   tree=api(f'repos/{repo}/git/trees/{revision}'+(':'+folder if folder else '')+'?recursive=1')
   if tree.get('truncated'):raise ValueError('Truncated inventory '+repo)
   fixtures=[dict(path=(folder+'/' if folder else '')+x['path'],size=x['size'],blob=x['sha']) for x in tree['tree'] if x['path'].lower().endswith('.pptx') and x['type']=='blob']
   pins.append(dict(prefix=prefix,repository='https://github.com/'+repo,revision=revision,fixtures=fixtures,licenses=licenses))
   print(prefix,revision,len(fixtures),'files',flush=True)
  pins_path.write_text(json.dumps(pins,indent=2)+'\n')
 jobs=[]; exclusions=[]
 for source in pins:
  for f in source['fixtures']:
   if not 0<f['size']<=32*1024**2:
    exclusions.append(dict(source=source['prefix'],**f,reason='outside documented 32 MiB fixture cap'));continue
   stem=re.sub('[^A-Za-z0-9_.-]','_',Path(f['path']).stem)
   name=source['prefix']+'-'+stem+'-'+hashlib.sha256(f['path'].encode()).hexdigest()[:6]
   jobs.append((source,dict(id=name,file=name+'.pptx',purpose='upstream presentation regression: '+Path(f['path']).stem,**f),'files'))
  for path in source['licenses']:jobs.append((source,dict(file=source['prefix']+'-'+path.replace('/','-'),path=path),'licenses'))
 def fetch(job):
  source,record,folder=job; dest=HERE/folder/record['file'];dest.parent.mkdir(exist_ok=True)
  data=dest.read_bytes() if dest.exists() else get(source['repository'].replace('https://github.com/','https://raw.githubusercontent.com/')+'/'+source['revision']+'/'+record['path'])
  if 'blob' in record:
   assert len(data)==record['size'] and hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest()==record['blob'],record['path']
  dest.write_bytes(data)
  return folder,dict(repository=source['repository'],revision=source['revision'],**{**record,'size':len(data),'sha256':hashlib.sha256(data).hexdigest()})
 records=[];errors=[]
 with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
  futures={pool.submit(fetch,j):j for j in jobs}
  for i,future in enumerate(concurrent.futures.as_completed(futures),1):
   try: records.append(future.result())
   except Exception as e:errors.append(dict(path=futures[future][1]['path'],error=str(e)))
   if i%50==0:print('Downloaded',i,'/',len(jobs),flush=True)
 fixtures=sorted([r for folder,r in records if folder=='files'],key=lambda r:r['id']);notices=sorted([r for folder,r in records if folder=='licenses'],key=lambda r:r['file'])
 manifest=dict(schema_version=1,repository=pins[0]['repository'],revision=pins[0]['revision'],fixtures=fixtures,notices=notices,exclusions=exclusions,download_errors=errors)
 (HERE/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
 unique=[];seen={};aliases=[];non_zip=[]
 for f in fixtures:
  if f['sha256'] in seen:aliases.append(dict(id=f['id'],same_as=seen[f['sha256']]))
  else:
   seen[f['sha256']]=f['id'];unique.append(f)
   if not zipfile.is_zipfile(HERE/'files'/f['file']):non_zip.append(f['id'])
 (HERE/'inventory.json').write_text(json.dumps(dict(duplicate_aliases=aliases,non_zip=non_zip),indent=2)+'\n')
 for i in range(4):
  (HERE/f'shard-{i}.json').write_text(json.dumps({**manifest,'fixtures':unique[i::4]},indent=2)+'\n')
 print('Collected',len(fixtures),'PPTX files;',len(errors),'errors',flush=True)
 if errors:raise RuntimeError(errors)
if __name__=='__main__':main()
