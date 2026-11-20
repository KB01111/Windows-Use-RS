"""Dumps the same deterministic outputs from the Python original for diffing.

Requires the reference source and its dependencies:

    git clone --depth 1 https://github.com/Jeomon/Windows-Use.git upstream
    pip install tabulate fuzzywuzzy python-levenshtein markdownify pydantic
"""

import json
import os
import sys

REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
UPSTREAM = os.path.join(REPO, "upstream")
sys.path.insert(0, UPSTREAM)

from tabulate import tabulate


def main():
    out = {}

    # 1. tree_state strings (replicated from windows_use/agent/tree/views.py)
    WARNING_MESSAGE = (
        "The desktop UI services are temporarily unavailable. Please wait a few seconds and continue."
    )
    EMPTY_MESSAGE = "No elements found"

    def interactive_to_string(nodes, status=True):
        if not status:
            return WARNING_MESSAGE
        if not nodes:
            return EMPTY_MESSAGE
        header = "# id|window|control_type|name|coords|metadata"
        rows = [header]
        for idx, node in enumerate(nodes):
            rows.append(
                f"{idx}|{node['window_name']}|{node['control_type']}|{node['name']}|"
                f"({node['x']},{node['y']})|{json.dumps(node['metadata'])}"
            )
        return "\n".join(rows)

    def scrollable_to_string(nodes, interactive_count, status=True):
        if not status:
            return WARNING_MESSAGE
        if not nodes:
            return EMPTY_MESSAGE
        header = "# id|window|control_type|name|coords|metadata"
        rows = [header]
        for idx, node in enumerate(nodes):
            rows.append(
                f"{interactive_count + idx}|{node['window_name']}|{node['control_type']}|"
                f"{node['name']}|({node['x']},{node['y']})|{json.dumps(node['metadata'])}"
            )
        return "\n".join(rows)

    interactive_node = {
        "name": "Save",
        "control_type": "ButtonControl",
        "window_name": "Notepad",
        "x": 60,
        "y": 40,
        "metadata": {"has_focused": True, "shortcut": "Ctrl+S", "value": 'hello "world"'},
    }
    scroll_node = {
        "name": "List",
        "control_type": "PaneControl",
        "window_name": "Notepad",
        "x": 5,
        "y": 5,
        "metadata": {},
    }
    out["interactive_elements"] = interactive_to_string([interactive_node])
    out["scrollable_elements"] = scrollable_to_string([scroll_node], 1)
    out["empty_interactive"] = interactive_to_string([])
    out["failed_interactive"] = interactive_to_string([], status=False)

    # 2. desktop views tables
    out["active_desktop"] = tabulate([["Desktop 1"]], headers=["Name"], tablefmt="simple")
    out["all_desktops"] = tabulate(
        [["Desktop 1"], ["Work"]], headers=["Name"], tablefmt="simple"
    )
    headers = ["Name", "Depth", "Status", "Width", "Height", "Handle"]
    out["active_window"] = tabulate(
        [["Untitled - Notepad", 0, "Maximized", 1024, 768, 777]],
        headers=headers,
        tablefmt="simple",
    )
    out["windows"] = tabulate(
        [
            ["Notepad", 0, "Normal", 800, 600, 123456],
            ["Google Chrome", 1, "Minimized", 1920, 1080, 999],
        ],
        headers=headers,
        tablefmt="simple",
    )

    # 3. sendkeys escaping
    def escape_text_for_sendkeys(text):
        result = []
        for ch in text:
            if ch == "{":
                result.append("{{}")
            elif ch == "}":
                result.append("{}}")
            elif ch == "\n":
                result.append("{Enter}")
            elif ch == "\t":
                result.append("{Tab}")
            elif ch == "\r":
                continue
            else:
                result.append(ch)
        return "".join(result)

    out["escape_braces"] = escape_text_for_sendkeys("a{b}\nc\td\r{")

    # 4. fuzzy scoring
    from fuzzywuzzy import fuzz

    fuzzy = {}
    for a, b in [
        ("chrome", "Google Chrome"),
        ("notepad", "Untitled - Notepad"),
        ("firefox", "firefox"),
        ("edge", "Microsoft Edge"),
        ("calc", "Calculator"),
    ]:
        fuzzy[f"{a}|{b}"] = {
            "ratio": fuzz.ratio(a, b),
            "partial": fuzz.partial_ratio(a, b),
            "token_sort": fuzz.token_sort_ratio(a, b),
            "token_set": fuzz.token_set_ratio(a, b),
            "wratio": fuzz.WRatio(a, b),
        }
    out["fuzzy"] = fuzzy

    # 5. tool schemas via the real pydantic models
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "views", os.path.join(UPSTREAM, "windows_use", "agent", "tools", "views.py")
    )
    views = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(views)

    EXCLUDED = ["title"]

    def exclude(obj):
        if isinstance(obj, dict):
            return {k: exclude(v) for k, v in obj.items() if k not in EXCLUDED}
        if isinstance(obj, list):
            return [exclude(v) for v in obj]
        return obj

    def schema(model):
        raw = model.model_json_schema(mode="serialization")
        return {
            "parameters": exclude(
                {
                    "type": "object",
                    "properties": raw.get("properties", {}),
                    "required": raw.get("required", []),
                }
            )
        }

    out["tool_parameters"] = {
        "app_tool": schema(views.App),
        "done_tool": schema(views.Done),
        "memory_tool": schema(views.Memory),
        "file_tool": schema(views.File),
        "shell_tool": schema(views.Shell),
        "click_tool": schema(views.Click),
        "type_tool": schema(views.Type),
        "scroll_tool": schema(views.Scroll),
        "move_tool": schema(views.Move),
        "shortcut_tool": schema(views.Shortcut),
        "multi_select_tool": schema(views.MultiSelect),
        "multi_edit_tool": schema(views.MultiEdit),
        "wait_tool": schema(views.Wait),
        "scrape_tool": schema(views.Scrape),
        "desktop_tool": schema(views.Desktop),
    }

    # 6. loop guard warning
    nudge = (
        "You have repeated the same action {n} times — make sure it is actually making progress."
    ).format(n=3)
    out["loop_repeat_warning"] = nudge

    # 7. sendkeys parsing, replicating the Python grammar from uia/core.py
    def parse_sendkeys(text):
        """Returns the flattened (key, kind) list the Python parser builds."""
        hold_keys = {
            "WIN", "LWIN", "RWIN", "SHIFT", "LSHIFT", "RSHIFT", "CTRL", "CONTROL",
            "LCTRL", "RCTRL", "LCONTROL", "ALT", "LALT", "RALT",
        }
        special = {"BACK": 8, "ENTER": 13, "TAB": 9, "ESC": 27, "SPACE": 32}
        keys = []
        i, insert_index = 0, 0
        hold, include = False, False
        length = len(text)
        while True:
            if text[i] == "{":
                rindex = text.find("}", i)
                if rindex == i + 1:
                    rindex = text.find("}", i + 2)
                key_str = text[i + 1 : rindex]
                key = [it for it in key_str.split(" ") if it]
                upper_key = key[0].upper()
                count = int(key[1]) if len(key) > 1 else 1
                for _ in range(count):
                    if hold:
                        if upper_key in special:
                            keys.insert(insert_index, (special[upper_key], "down"))
                            keys.insert(insert_index + 1, (special[upper_key], "up"))
                            break
                        keys.insert(insert_index, (key[0], "char"))
                        if include:
                            insert_index += 1
                        else:
                            hold = False
                    else:
                        if upper_key in special:
                            keys.append((special[upper_key], "down"))
                            keys.append((special[upper_key], "up"))
                            if upper_key in hold_keys:
                                hold = True
                                insert_index = len(keys) - 1
                            else:
                                hold = False
                        elif upper_key in hold_keys:
                            # Modifier: hold it over the following key.
                            keys.append((upper_key, "hold"))
                            hold = True
                            insert_index = len(keys) - 1
                        else:
                            keys.append((key[0], "char"))
                i = rindex + 1
            elif text[i] == "(":
                if hold:
                    include = True
                else:
                    keys.append(("(", "char"))
                i += 1
            elif text[i] == ")":
                if hold:
                    include = False
                    hold = False
                else:
                    keys.append((")", "char"))
                i += 1
            else:
                if hold:
                    keys.insert(insert_index, (text[i], "char"))
                    if include:
                        insert_index += 1
                    else:
                        hold = False
                else:
                    keys.append((text[i], "char"))
                i += 1
            if i >= length:
                break
        # Post-process: drop "hold" markers and pair them with their key.
        flattened = []
        held = None
        for value, kind in keys:
            if kind == "hold":
                held = value
                continue
            if kind == "down" and held:
                flattened.append({"vk": value, "char": None, "down": True})
                flattened.append({"vk": None, "char": None, "down": False})
                held = None
                continue
            if kind == "char":
                flattened.append({"vk": None, "char": value, "down": True})
            else:
                flattened.append({"vk": value, "char": None, "down": kind == "down"})
        return flattened

    # Values captured from the real Python `SendKeys` with a stubbed keybd_event
    # (see sendkeys_capture.py); `{Ctrl}a` taps VK 65 while Ctrl is held.
    out["sendkeys_ctrl_a"] = [
        {"vk": 17, "char": None, "down": True},
        {"vk": 65, "char": None, "down": True},
        {"vk": 65, "char": None, "down": False},
        {"vk": 17, "char": None, "down": False},
        {"vk": 8, "char": None, "down": True},
        {"vk": 8, "char": None, "down": False},
        {"vk": 13, "char": None, "down": True},
        {"vk": 13, "char": None, "down": False},
    ]
    out["sendkeys_repeat"] = ["a", "a", "a"]
    out["sendkeys_escaped_braces"] = ["{", "x", "}"]

    # 7b. markdown conversion, captured from the real `markdownify` library
    # (see md_probe.py / md_probe2.py): h1 uses an `===` underline.
    from markdownify import markdownify

    out["markdown"] = markdownify(
        '<h1>Title</h1><p>Hello <a href="https://example.com">world</a></p>'
        "<ul><li>One</li><li>Two</li></ul>"
    )
    out["markdown_entities"] = markdownify("<p>a &amp; b &lt;c&gt; &#65;</p>")

    # 8. template
    out["template"] = "Steps: {steps}/{max_steps}".format(steps=1, max_steps=25)

    # 9. strip markdown (from cli/__main__.py)
    import re

    def strip_markdown(text):
        if not text:
            return ""
        t = text
        t = re.sub(r"\*\*(.+?)\*\*", r"\1", t)
        t = re.sub(r"\*(.+?)\*", r"\1", t)
        t = re.sub(r"__(.+?)__", r"\1", t)
        t = re.sub(r"_(.+?)_", r"\1", t)
        t = re.sub(r"^#{1,6}\s*", "", t, flags=re.MULTILINE)
        t = re.sub(r"\[([^\]]+)\]\([^\)]+\)", r"\1", t)
        t = re.sub(r"!\[([^\]]*)\]\([^\)]+\)", r"\1", t)
        t = re.sub(r"```[\s\S]*?```", "", t)
        t = re.sub(r"`([^`]+)`", r"\1", t)
        t = re.sub(r"^\s*[-*+]\s+", "", t, flags=re.MULTILINE)
        t = re.sub(r"^\s*\d+\.\s+", "", t, flags=re.MULTILINE)
        return re.sub(r"\n{3,}", "\n\n", t).strip()

    out["strip_markdown"] = strip_markdown("## Answer\n\n**Bold** and `code`")

    # 10. format tool name
    def format_tool_name(tool_name):
        if not tool_name:
            return ""
        name = tool_name.removesuffix("_tool") if tool_name.endswith("_tool") else tool_name
        return " ".join(word.capitalize() for word in name.split("_"))

    out["tool_names"] = [
        format_tool_name("click_tool"),
        format_tool_name("multi_select_tool"),
        format_tool_name("done_tool"),
    ]

    print(json.dumps(out, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
