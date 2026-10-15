#!/usr/bin/env python3
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import run

class CorpusTests(unittest.TestCase):
    def test_limit_overrides_are_explicitly_forwarded_through_clearenv(self):
        settings = {name: str(100 + i) for i, name in enumerate(run.PREVIEW_LIMIT_ENV)}
        with patch.dict(run.os.environ, settings, clear=True):
            cmd = run.sandbox(Path('/input'), Path('/work'), Path('/bundle'), Path('/converter'))
        self.assertIn('--clearenv', cmd)
        for name, value in settings.items():
            i = cmd.index(name)
            self.assertEqual(cmd[i-1:i+2], ['--setenv', name, value])

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

    def test_heatmap_color_pixels_are_measured(self):
        data = b'P6\n3 1\n255\n' + bytes([248,105,107,99,190,123,255,255,255])
        with patch.object(run.subprocess, 'check_output', return_value=data):
            colors = run.raster_color_counts(Path('unused.pdf'))
        self.assertEqual(colors['rose'], 1)
        self.assertEqual(colors['green'], 1)

    def test_region_checks_do_not_count_colors_elsewhere_on_page(self):
        data = b'P6\n2 1\n255\n' + bytes([0,0,255,0,0,0])
        with patch.object(run.subprocess, 'check_output', return_value=data):
            colors = run.raster_color_counts(Path('unused.pdf'), [.5,0,1,1])
        self.assertEqual(colors['blue'], 0)
        self.assertEqual(colors['dark'], 1)
        stage = {'status':'ok','pdf':{'pages':1,'image_count':0,'regions':[colors]}}
        expected = {'regions':[{'min_colors':{'blue':1},'max_colors':{'dark':0}}]}
        self.assertEqual(len(run.check_expectations(stage,'',expected)), 2)

    def test_checked_in_manifest(self):
        for name in ['manifest.json', 'expanded.json']:
            run.validate_manifest(json.loads((run.HERE/name).read_text()))

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
