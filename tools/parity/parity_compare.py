"""Diffs the Rust port's deterministic outputs against the Python original.

Usage (from anywhere):

    python tools/parity/parity_regen.py     # writes parity_python.json
    cargo run --bin dump_parity             # writes parity_rust.json
    python tools/parity/parity_compare.py   # reports differences
"""

import io
import json
import os
import sys

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")

REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
PYTHON_DUMP = os.path.join(REPO, "parity_python.json")
RUST_DUMP = os.path.join(REPO, "parity_rust.json")


def compare(path, a, b, diffs):
    if isinstance(a, bool) != isinstance(b, bool):
        diffs.append(f"{path}: bool mismatch {a!r} vs {b!r}")
        return
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        if a != b:
            diffs.append(f"{path}: {a!r} != {b!r}")
        return
    if type(a) is not type(b):
        diffs.append(f"{path}: type {type(a).__name__} vs {type(b).__name__}")
        return
    if isinstance(a, dict):
        for key in sorted(set(a) | set(b)):
            if key not in a:
                diffs.append(f"{path}.{key}: missing in python")
            elif key not in b:
                diffs.append(f"{path}.{key}: missing in rust")
            else:
                compare(f"{path}.{key}", a[key], b[key], diffs)
    elif isinstance(a, list):
        if len(a) != len(b):
            diffs.append(f"{path}: length {len(a)} vs {len(b)}")
        for index, (left, right) in enumerate(zip(a, b)):
            compare(f"{path}[{index}]", left, right, diffs)
    elif a != b:
        diffs.append(f"{path}: {a!r} != {b!r}")


def main():
    for path in (PYTHON_DUMP, RUST_DUMP):
        if not os.path.exists(path):
            sys.exit(f"missing {path}; run the dump steps first")

    with open(PYTHON_DUMP, encoding="utf-8-sig") as handle:
        py = json.load(handle)
    with open(RUST_DUMP, encoding="utf-8-sig") as handle:
        rs = json.load(handle)

    diffs = []
    for key in sorted(set(py) | set(rs)):
        if key == "tool_parameters" and key in py and "tools" in rs:
            for name, schema in py[key].items():
                if name not in rs["tools"]:
                    diffs.append(f"{name}: missing in rust")
                    continue
                compare(
                    f"parameters.{name}",
                    schema["parameters"],
                    rs["tools"][name]["parameters"],
                    diffs,
                )
            continue
        if key == "tools":
            continue
        if key in py and key in rs:
            compare(key, py[key], rs[key], diffs)
        else:
            diffs.append(f"{key}: present only in {'python' if key in py else 'rust'}")

    print(f"compared {len(py)} keys")
    if diffs:
        print(f"{len(diffs)} differences:")
        for diff in diffs[:40]:
            print("  " + diff)
        sys.exit(1)
    print("NO DIFFERENCES ACROSS ALL COMPARED OUTPUTS")


if __name__ == "__main__":
    main()
