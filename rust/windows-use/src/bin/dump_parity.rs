//! Dumps the deterministic outputs used to prove parity with the Python original.
//!
//! Run `python tools/parity/parity_regen.py` first, then:
//!
//! ```text
//! cargo run --bin dump_parity
//! python tools/parity/parity_compare.py
//! ```

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

fn tree_metadata() -> BTreeMap<String, Value> {
    let mut metadata = BTreeMap::new();
    metadata.insert("has_focused".to_string(), Value::Bool(true));
    metadata.insert("shortcut".to_string(), Value::String("Ctrl+S".to_string()));
    metadata.insert("value".to_string(), Value::String("hello \"world\"".to_string()));
    metadata
}

fn main() {
    let mut out = Map::new();

    // 1. tree_state.interactive_elements_to_string
    let node = windows_use::tree::views::TreeElementNode {
        name: "Save".to_string(),
        control_type: "ButtonControl".to_string(),
        bounding_box: windows_use::tree::views::BoundingBox::new(10, 20, 110, 60),
        center: windows_use::tree::views::Center { x: 60, y: 40 },
        window_name: "Notepad".to_string(),
        hwnd: 42,
        control_index: None,
        metadata: tree_metadata(),
    };
    let state = windows_use::tree::views::TreeState {
        status: true,
        interactive_nodes: vec![node],
        scrollable_nodes: vec![windows_use::tree::views::ScrollElementNode {
            name: "List".to_string(),
            control_type: "PaneControl".to_string(),
            window_name: "Notepad".to_string(),
            bounding_box: windows_use::tree::views::BoundingBox::new(0, 0, 10, 10),
            center: windows_use::tree::views::Center { x: 5, y: 5 },
            horizontal_scrollable: false,
            horizontal_scroll_percent: 0.0,
            vertical_scrollable: true,
            vertical_scroll_percent: 42.5,
            hwnd: 7,
            control_index: None,
            metadata: BTreeMap::new(),
        }],
        dom_informative_nodes: vec![windows_use::tree::views::TextElementNode {
            text: "Some text".to_string(),
        }],
        ..Default::default()
    };

    out.insert(
        "interactive_elements".to_string(),
        Value::String(state.interactive_elements_to_string()),
    );
    out.insert(
        "scrollable_elements".to_string(),
        Value::String(state.scrollable_elements_to_string()),
    );
    let empty = windows_use::tree::views::TreeState::default();
    out.insert(
        "empty_interactive".to_string(),
        Value::String(empty.interactive_elements_to_string()),
    );
    let failed = windows_use::tree::views::TreeState {
        status: false,
        ..Default::default()
    };
    out.insert(
        "failed_interactive".to_string(),
        Value::String(failed.interactive_elements_to_string()),
    );

    // 2. desktop views tables
    let desktop_state = windows_use::desktop::views::DesktopState {
        active_desktop: windows_use::desktop::views::VirtualDesktop {
            id: "0".to_string(),
            name: "Desktop 1".to_string(),
        },
        all_desktops: vec![
            windows_use::desktop::views::VirtualDesktop {
                id: "0".to_string(),
                name: "Desktop 1".to_string(),
            },
            windows_use::desktop::views::VirtualDesktop {
                id: "1".to_string(),
                name: "Work".to_string(),
            },
        ],
        windows: vec![
            windows_use::desktop::views::Window {
                name: "Notepad".to_string(),
                is_browser: false,
                depth: 0,
                status: windows_use::desktop::views::Status::Normal,
                bounding_box: windows_use::tree::views::BoundingBox::new(0, 0, 800, 600),
                handle: 123456,
                process_id: 1,
            },
            windows_use::desktop::views::Window {
                name: "Google Chrome".to_string(),
                is_browser: true,
                depth: 1,
                status: windows_use::desktop::views::Status::Minimized,
                bounding_box: windows_use::tree::views::BoundingBox::new(0, 0, 1920, 1080),
                handle: 999,
                process_id: 2,
            },
        ],
        active_window: Some(windows_use::desktop::views::Window {
            name: "Untitled - Notepad".to_string(),
            is_browser: false,
            depth: 0,
            status: windows_use::desktop::views::Status::Maximized,
            bounding_box: windows_use::tree::views::BoundingBox::new(0, 0, 1024, 768),
            handle: 777,
            process_id: 3,
        }),
        screenshot: None,
        tree_state: None,
    };
    out.insert(
        "active_desktop".to_string(),
        Value::String(desktop_state.active_desktop_to_string()),
    );
    out.insert(
        "all_desktops".to_string(),
        Value::String(desktop_state.desktops_to_string()),
    );
    out.insert(
        "active_window".to_string(),
        Value::String(desktop_state.active_window_to_string()),
    );
    out.insert(
        "windows".to_string(),
        Value::String(desktop_state.windows_to_string()),
    );

    // 3. sendkeys escaping
    out.insert(
        "escape_braces".to_string(),
        Value::String(windows_use::uia::escape_text_for_sendkeys(
            "a{b}\nc\td\r{",
        )),
    );

    // 4. fuzzy scoring
    let mut fuzzy = Map::new();
    for (a, b) in [
        ("chrome", "Google Chrome"),
        ("notepad", "Untitled - Notepad"),
        ("firefox", "firefox"),
        ("edge", "Microsoft Edge"),
        ("calc", "Calculator"),
    ] {
        let mut scores = Map::new();
        scores.insert(
            "ratio".to_string(),
            json!(windows_use::desktop::service::ratio(a, b)),
        );
        scores.insert(
            "partial".to_string(),
            json!(windows_use::desktop::service::partial_ratio(a, b)),
        );
        scores.insert(
            "token_sort".to_string(),
            json!(windows_use::desktop::service::token_sort_ratio(a, b)),
        );
        scores.insert(
            "token_set".to_string(),
            json!(windows_use::desktop::service::token_set_ratio(a, b)),
        );
        scores.insert(
            "wratio".to_string(),
            json!(windows_use::desktop::service::weighted_ratio(a, b)),
        );
        fuzzy.insert(format!("{a}|{b}"), Value::Object(scores));
    }
    out.insert("fuzzy".to_string(), Value::Object(fuzzy));

    // 5. tool schemas, using the generated parameters
    let mut schemas = Map::new();
    for tool in windows_use::agent::tools::BUILTIN_TOOLS
        .iter()
        .chain(windows_use::agent::tools::EXPERIMENTAL_TOOLS.iter())
    {
        schemas.insert(
            tool.name().to_string(),
            json!({
                "description": tool.description(),
                "parameters": windows_use::agent::tool_schemas::parameters(tool.name()),
            }),
        );
    }
    out.insert("tools".to_string(), Value::Object(schemas));

    // 6. loop guard warning for three repeated actions
    let mut guard = windows_use::agent::loop_guard::LoopGuard::default();
    let mut params = Map::new();
    params.insert("loc".to_string(), json!([10, 20]));
    params.insert("thought".to_string(), json!("trying"));
    for _ in 0..3 {
        guard.record_action("click_tool", &params, true);
    }
    out.insert("loop_repeat_warning".to_string(), json!(guard.check()));

    // 7. sendkeys parsing
    out.insert(
        "sendkeys_ctrl_a".to_string(),
        Value::Array(
            windows_use::uia::parse_send_keys("{Ctrl}a{Back}{Enter}")
                .unwrap_or_default()
                .into_iter()
                .map(|action| json!({ "vk": action.vk, "char": action.character, "down": action.down }))
                .collect(),
        ),
    );
    out.insert(
        "sendkeys_repeat".to_string(),
        Value::Array(
            windows_use::uia::parse_send_keys("{a 3}")
                .unwrap_or_default()
                .into_iter()
                .filter_map(|action| action.character)
                .map(|character| Value::String(character.to_string()))
                .collect(),
        ),
    );
    out.insert(
        "sendkeys_escaped_braces".to_string(),
        Value::Array(
            windows_use::uia::parse_send_keys("{{}x{}}")
                .unwrap_or_default()
                .into_iter()
                .filter_map(|action| action.character)
                .map(|character| Value::String(character.to_string()))
                .collect(),
        ),
    );

    // 8. markdown conversion
    out.insert(
        "markdown".to_string(),
        json!(windows_use::markdown::html_to_markdown(
            "<h1>Title</h1><p>Hello <a href=\"https://example.com\">world</a></p><ul><li>One</li><li>Two</li></ul>"
        )),
    );
    out.insert(
        "markdown_entities".to_string(),
        json!(windows_use::markdown::html_to_markdown(
            "<p>a &amp; b &lt;c&gt; &#65;</p>"
        )),
    );

    // 9. prompt template rendering
    out.insert(
        "template".to_string(),
        json!(windows_use::agent::context::fill_template(
            "Steps: {steps}/{max_steps}",
            &[("steps", "1".to_string()), ("max_steps", "25".to_string())]
        )),
    );

    // 10. strip_markdown for TTS
    out.insert(
        "strip_markdown".to_string(),
        json!(windows_use::cli::subscriber::strip_markdown(
            "## Answer\n\n**Bold** and `code`"
        )),
    );

    // 11. format_tool_name
    out.insert(
        "tool_names".to_string(),
        json!([
            windows_use::events::format_tool_name("click_tool"),
            windows_use::events::format_tool_name("multi_select_tool"),
            windows_use::events::format_tool_name("done_tool"),
        ]),
    );

    let rendered = serde_json::to_string_pretty(&Value::Object(out)).unwrap();
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "parity_rust.json".to_string());
    std::fs::write(&path, rendered.as_bytes()).expect("write parity output");
    println!("wrote {path} ({} bytes)", rendered.len());
}
