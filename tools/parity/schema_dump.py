import io, sys, json, importlib.util
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
spec = importlib.util.spec_from_file_location("views", "upstream/windows_use/agent/tools/views.py")
views = importlib.util.module_from_spec(spec); spec.loader.exec_module(views)

EXCLUDED = ["title"]
def exclude(obj):
    if isinstance(obj, dict):
        return {k: exclude(v) for k, v in obj.items() if k not in EXCLUDED}
    if isinstance(obj, list):
        return [exclude(v) for v in obj]
    return obj

models = {
    "app_tool": views.App, "done_tool": views.Done, "memory_tool": views.Memory,
    "file_tool": views.File, "shell_tool": views.Shell, "click_tool": views.Click,
    "type_tool": views.Type, "scroll_tool": views.Scroll, "move_tool": views.Move,
    "shortcut_tool": views.Shortcut, "multi_select_tool": views.MultiSelect,
    "multi_edit_tool": views.MultiEdit, "wait_tool": views.Wait,
    "scrape_tool": views.Scrape, "desktop_tool": views.Desktop,
}
for name, model in models.items():
    raw = exclude(model.model_json_schema(mode="serialization"))
    print(f"### {name}")
    print(json.dumps(raw, ensure_ascii=False, indent=1))
