import struct
import tempfile
import unittest
from pathlib import Path
from visual_audit import compatible, png_size

class VisualAuditTests(unittest.TestCase):
    def test_only_tiny_page_rounding_is_accepted(self):
        self.assertTrue(compatible([720,540],[720.01,539.99]))
        self.assertFalse(compatible([720,540],[721,540]))
        self.assertFalse(compatible([720,540],[540,720]))
        self.assertFalse(compatible([720,540],[float('nan'),540]))

    def test_png_dimensions_are_read_without_resampling(self):
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp)/'page.png'
            p.write_bytes(b'\x89PNG\r\n\x1a\n'+struct.pack('>I',13)+b'IHDR'+struct.pack('>II',1600,900))
            self.assertEqual(png_size(p),(1600,900))
            p.write_bytes(b'bad')
            with self.assertRaises(ValueError):png_size(p)

if __name__=='__main__':unittest.main()
