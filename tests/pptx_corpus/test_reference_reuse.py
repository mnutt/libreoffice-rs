import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('office_runner', Path(__file__).resolve().parents[1]/'xlsx_corpus/run.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class ReferenceReuseTest(unittest.TestCase):
    def test_same_filesystem_shares_immutable_artifact(self):
        with tempfile.TemporaryDirectory() as root:
            source, target = Path(root)/'reference.pdf', Path(root)/'reused.pdf'
            source.write_bytes(b'%PDF-immutable-reference')
            runner.reuse_artifact(source, target)
            self.assertEqual(source.stat().st_ino, target.stat().st_ino)
            self.assertEqual(source.read_bytes(), target.read_bytes())

    def test_link_failure_copies_without_changing_reference(self):
        with tempfile.TemporaryDirectory() as root:
            source, target = Path(root)/'reference.pdf', Path(root)/'reused.pdf'
            source.write_bytes(b'%PDF-immutable-reference')
            with patch.object(runner.os, 'link', side_effect=OSError('different filesystem')):
                runner.reuse_artifact(source, target)
            self.assertNotEqual(source.stat().st_ino, target.stat().st_ino)
            self.assertEqual(source.read_bytes(), target.read_bytes())
