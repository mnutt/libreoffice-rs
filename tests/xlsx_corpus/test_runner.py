#!/usr/bin/env python3
import copy
import json
from pathlib import Path
import tempfile
import unittest
import run

class CorpusTests(unittest.TestCase):
    def test_feature_expectations_catch_missing_images_and_chart_pages(self):
        stage={'status':'ok','pdf':{'pages':1,'image_count':0}}
        expected={'min_images':1,'min_pages':2,'page_text':{'2':['Chart title']}}
        self.assertEqual(len(run.check_expectations(stage,'Chart title',expected)),3)
        stage['pdf']={'pages':2,'image_count':1}
        self.assertEqual(run.check_expectations(stage,'cells\fChart   title\f',expected),[])

    def test_print_area_and_page_break_expectations(self):
        stage={'status':'ok','pdf':{'pages':1,'image_count':0}}
        self.assertEqual(len(run.check_expectations(stage,'OUTSIDE',{'pages':2,'absent_text':['outside']})),2)

    def test_generated_manifest_and_generator_match(self):
        manifest=json.loads((run.HERE/'generated.json').read_text())
        run.validate_manifest(manifest)
        self.assertEqual(manifest['generator_sha256'],run.digest(run.HERE/'generate.py'))

    def test_visual_expectations_catch_missing_features(self):
        stage={'status':'ok','pdf':{'pages':1,'image_count':1,'colors':{'red':200,'blue':0},'font_names':['Helvetica']}}
        expected={'min_colors':{'blue':100},'max_colors':{'red':0},'fonts':['LiberationSans-Bold']}
        self.assertEqual(len(run.check_expectations(stage,'',expected)),3)

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
