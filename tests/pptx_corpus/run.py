#!/usr/bin/env python3
"""PPTX adapter for the shared sandboxed Office comparison runner."""
import importlib.util
from pathlib import Path
spec = importlib.util.spec_from_file_location('office_corpus', Path(__file__).resolve().parents[1]/'xlsx_corpus/run.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
runner.HERE = Path(__file__).resolve().parent
runner.FORMAT = 'pptx'
if __name__ == '__main__':
    runner.main()
