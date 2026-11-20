"""Generates src/agent/tool_schemas.rs from the Python pydantic models.

The generated table is the exact `parameters` object that `Tool.json_schema`
produces upstream, so the Rust port sends byte-identical schemas to the LLM.

Fetch the reference source first:

    git clone --depth 1 https://github.com/Jeomon/Windows-Use.git upstream
"""

import importlib.util
import json
import os

REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
VIEWS = os.path.join(REPO, "upstream", "windows_use", "agent", "tools", "views.py")
OUTPUT = os.path.join(REPO, "rust", "windows-use", "src", "agent", "tool_schemas.rs")

EXCLUDED = ["title"]

TOOLS = [
    ("app_tool", "App"),
    ("done_tool", "Done"),
    ("memory_tool", "Memory"),
    ("file_tool", "File"),
    ("shell_tool", "Shell"),
    ("click_tool", "Click"),
    ("type_tool", "Type"),
    ("scroll_tool", "Scroll"),
    ("move_tool", "Move"),
    ("shortcut_tool", "Shortcut"),
    ("multi_select_tool", "MultiSelect"),
    ("multi_edit_tool", "MultiEdit"),
    ("wait_tool", "Wait"),
    ("scrape_tool", "Scrape"),
    ("desktop_tool", "Desktop"),
]


def exclude(obj):
    if isinstance(obj, dict):
        return {k: exclude(v) for k, v in obj.items() if k not in EXCLUDED}
    if isinstance(obj, list):
        return [exclude(v) for v in obj]
    return obj


def parameters(model):
    raw = model.model_json_schema(mode="serialization")
    return exclude(
        {
            "type": "object",
            "properties": raw.get("properties", {}),
            "required": raw.get("required", []),
        }
    )


def rust_literal(value, indent=0):
    """Emits a `serde_json::from_str::<Value>` literal that round-trips to the same JSON.

    The inner string is escaped for a Rust string literal (quotes and backslashes),
    so non-ASCII text stays literal and `\\u` escape lookalikes cannot appear.
    """
    payload = json.dumps(value, ensure_ascii=False)
    rust_escaped = (
        payload.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
    )
    return (
        'serde_json::from_str::<Value>("'
        + rust_escaped
        + '").expect("schema literal")'
    )


def main():
    spec = importlib.util.spec_from_file_location("views", VIEWS)
    views = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(views)

    parts = [
        "//! Tool parameter schemas.",
        "//!",
        "//! GENERATED from `upstream/windows_use/agent/tools/views.py` by",
        "//! `generate_tool_schemas.py`. Do not edit by hand: regenerate instead so the",
        "//! parameters sent to the model stay byte-identical to the Python original.",
        "",
        "use serde_json::{json, Value};",
        "",
        "/// The `parameters` object for a tool, exactly as `Tool.json_schema` builds it.",
        "pub fn parameters(tool_name: &str) -> Value {",
        "    match tool_name {",
    ]

    for name, model_name in TOOLS:
        model = getattr(views, model_name)
        rendered = rust_literal(parameters(model))
        parts.append(f'        "{name}" => {rendered},')

    parts += [
        '        _ => json!({"type": "object", "properties": {}, "required": []}),',
        "    }",
        "}",
        "",
    ]

    os.makedirs(os.path.dirname(OUTPUT), exist_ok=True)
    with open(OUTPUT, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n".join(parts))
    print(f"wrote {OUTPUT}")


if __name__ == "__main__":
    main()
