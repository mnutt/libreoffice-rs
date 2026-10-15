#!/usr/bin/env python3
import copy
import json
from pathlib import Path
import tempfile
import unittest
import run

class CorpusTests(unittest.TestCase):
    def test_checked_in_manifest(self):
        run.validate_manifest(json.loads((run.HERE/'manifest.json').read_text()))

    def test_unsafe_paths_and_duplicate_ids_are_rejected(self):
        original=json.loads((run.HERE/'manifest.json').read_text())
        for field,value in [('file','../bad.xlsx'),('path','/absolute'),('id','../bad'),('sha256','unchecked')]:
            manifest=copy.deepcopy(original);manifest['fixtures'][0][field]=value
            with self.assertRaises(ValueError): run.validate_manifest(manifest)
        original['fixtures'].append(original['fixtures'][0])
        with self.assertRaises(ValueError): run.validate_manifest(original)

    def test_token_counts_and_empty_reference(self):
        with tempfile.TemporaryDirectory() as tmp:
            a,r=Path(tmp)/'a.txt',Path(tmp)/'r.txt'
            a.write_text('A A extra');r.write_text('a a a')
            score=run.compare_text(a,r)
            self.assertAlmostEqual(score['recall'],2/3)
            self.assertAlmostEqual(score['precision'],2/3)
            self.assertEqual(score['missing'],[('a',1)])
            r.write_text('')
            self.assertIsNone(run.compare_text(a,r)['recall'])

    def test_checksum_mismatch_is_not_silently_accepted(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'x';path.write_bytes(b'new')
            with self.assertRaises(ValueError): run.verify(path,dict(size=3,sha256='0'*64))

if __name__=='__main__': unittest.main()
