"""Writes the Python side of the parity comparison as UTF-8 JSON.

    python tools/parity/parity_regen.py
"""

import io
import json
import os
import sys

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)

import parity_dump  # noqa: E402  (needs HERE on sys.path)

buffer = io.StringIO()
saved = sys.stdout
sys.stdout = buffer
try:
    parity_dump.main()
finally:
    sys.stdout = saved

text = buffer.getvalue().strip()
with open(os.path.join(REPO, "parity_python.json"), "w", encoding="utf-8") as handle:
    handle.write(text)
print("python dump written, bytes:", len(text.encode("utf-8")))
