"""Captures the exact key event sequence the real Python SendKeys emits."""

import importlib.util
import io
import json
import os
import sys
import types

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")

UIA_DIR = os.path.join(os.path.dirname(__file__), "upstream", "windows_use", "uia")

# Build a synthetic package so `from .core import ...` resolves without importing
# the whole windows_use package (which pulls in the LLM SDKs).
package = types.ModuleType("uia_pkg")
package.__path__ = [UIA_DIR]
sys.modules["uia_pkg"] = package


def load(name):
    spec = importlib.util.spec_from_file_location(
        f"uia_pkg.{name}", os.path.join(UIA_DIR, f"{name}.py")
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[f"uia_pkg.{name}"] = module
    spec.loader.exec_module(module)
    return module


enums = load("enums")
exceptions = load("exceptions")
core = load("core")

captured = []


def fake_keybd_event(bVk, bScan, dwFlags, dwExtraInfo):
    if dwFlags & 0x2:  # KEYEVENTF_KEYUP
        captured.append(("up", bVk))
    else:
        captured.append(("down", bVk))


def fake_unicode(ch, charMode=True):
    captured.append(("char", ch))


core.keybd_event = fake_keybd_event
core.SendUnicodeChar = fake_unicode

probes = ["{Ctrl}a{Back}{Enter}", "{a 3}", "{{}x{}}", "{Shift}(ab)", "hello", "{Delete}"]
results = {}
for probe in probes:
    captured.clear()
    try:
        core.SendKeys(probe, interval=0, waitTime=0)
        results[probe] = [list(item) for item in captured]
    except Exception as error:  # noqa: BLE001
        results[probe] = f"error: {error}"

print(json.dumps(results, indent=1, ensure_ascii=False))
