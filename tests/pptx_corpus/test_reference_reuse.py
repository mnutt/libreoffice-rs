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


class CandidateCompactionTest(unittest.TestCase):
    def test_interrupted_replacement_retains_original_and_can_be_retried(self):
        import loop
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = []
            for name in ('a', 'b'):
                folder = root/name/'fixture'/'candidate'
                folder.mkdir(parents=True)
                path = folder/'page-1.png'
                path.write_bytes(b'identical image bytes')
                paths.append(path)
                (root/name/'results.json').write_text('{"cases":[{"id":"fixture"}]}')
            with patch.object(loop.os, 'replace', side_effect=OSError('interrupted')):
                with self.assertRaises(OSError):
                    loop.compact_completed_candidates(root)
            self.assertTrue(all(p.read_bytes() == b'identical image bytes' for p in paths))
            self.assertFalse(list(root.glob('**/*.dedup-*')))
            self.assertEqual(loop.compact_completed_candidates(root), len(b'identical image bytes'))
            self.assertEqual(paths[0].stat().st_ino, paths[1].stat().st_ino)

    def test_only_completed_outputs_are_shared_and_repeat_is_noop(self):
        import loop
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths=[]
            for name in ('done-a','done-b','active'):
                folder=root/name/'fixture'/'candidate'
                folder.mkdir(parents=True)
                path=folder/'page-1.png'
                path.write_bytes(b'identical image bytes')
                paths.append(path)
                # An active shard already has results for other completed cases.
                (root/name/'results.json').write_text(
                    '{"cases":[{"id":"fixture"}]}' if name != 'active'
                    else '{"cases":[{"id":"previous-fixture"}]}')
            active_inode=paths[2].stat().st_ino
            self.assertEqual(loop.compact_completed_candidates(root),len(b'identical image bytes'))
            self.assertEqual(paths[0].stat().st_ino,paths[1].stat().st_ino)
            self.assertEqual(paths[2].stat().st_ino,active_inode)
            self.assertNotEqual(paths[0].stat().st_ino,active_inode)
            self.assertTrue(all(p.read_bytes()==b'identical image bytes' for p in paths))
            self.assertEqual(loop.compact_completed_candidates(root),0)
