//! Agent tool implementations, mirroring `windows_use/agent/tools/service.py`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::desktop::service::{AppMode, AppRequest, Desktop};
use crate::tools::ParamSpec;

/// Maximum bytes read by `file_tool` in read mode.
pub const MAX_FILE_READ_SIZE: u64 = 1024 * 1024;

/// Formats a byte count as `B`, `KB`, `MB`, ...
pub fn format_size(size: u64) -> String {
    let mut value = size as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if value < 1024.0 {
            return format!("{} {}", value as u64, unit);
        }
        value /= 1024.0;
    }
    format!("{value:.1} PB")
}

/// Root directory for memory files (`~/.windows-use/memories`).
pub fn memory_root() -> PathBuf {
    crate::win32::home_dir().join(".windows-use").join("memories")
}

/// Resolves a memory path inside the memory root, rejecting escapes.
pub fn resolve_memory_path(path: &str) -> anyhow::Result<PathBuf> {
    let root = memory_root();
    let candidate = normalize_path(&root.join(path));
    let root_normalized = normalize_path(&root);
    if !candidate.starts_with(&root_normalized) {
        anyhow::bail!("Path escapes the .windows-use/memories directory: {path}");
    }
    Ok(candidate)
}

/// Resolves a file path; relative paths resolve against the user's home.
pub fn resolve_file_path(path: &str) -> anyhow::Result<PathBuf> {
    if path.contains('\0') {
        anyhow::bail!("Path contains null byte");
    }
    let candidate = PathBuf::from(path);
    let joined = if candidate.is_absolute() {
        candidate
    } else {
        crate::win32::home_dir().join(candidate)
    };
    Ok(normalize_path(&joined))
}

/// Lexically normalizes a path (no filesystem access).
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Tool parameter specs, matching `agent/tools/views.py`.
pub struct ToolSpecs;

impl ToolSpecs {
    pub fn shared_thought() -> ParamSpec {
        ParamSpec::string(
            "thought",
            "The rigorous thinking process where you analyze the current state, potential issues, and plan the next steps effectively. (max. 3-5 sentences)",
        )
    }

    pub fn click() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::int_array(
                "loc",
                "[x, y] pixel coordinates of the target element to click. Use coordinates from the Interactive Elements list.",
                true,
            )
            .examples(json!([[640, 360], [100, 200]])),
            ParamSpec::enumerated(
                "button",
                "Mouse button: 'left' for standard clicks and selection, 'right' for context menus, 'middle' for middle-click actions",
                &["left", "right", "middle"],
                Some("left"),
                false,
            )
            .examples(json!(["left", "right"])),
            ParamSpec::enumerated_int(
                "clicks",
                "Number of clicks: 1 for single click (select, focus, press buttons), 2 for double click (open files, folders, apps), 0 for hover only (no click performed)",
                &[0, 1, 2],
                1,
                false,
            )
            .examples(json!([1, 2])),
        ]
    }

    pub fn done() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::string(
                "answer",
                "The response to deliver to the user, formatted in markdown. This is the ONLY way to communicate with the user.",
            )
            .examples(json!([
                "## Task Completed\n\nThe file has been saved to Desktop.",
                "Hello! How can I help you today?",
                "I was unable to find the file. The Downloads folder is empty."
            ])),
        ]
    }

    pub fn app() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::enumerated(
                "mode",
                "Operation mode: 'launch' opens the application from Start Menu, 'resize' adjusts the active window's size and position, 'switch' brings a specific open window into the foreground",
                &["launch", "resize", "switch"],
                Some("launch"),
                false,
            )
            .examples(json!(["launch"])),
            ParamSpec::nullable("name", "Application name as it appears in Start Menu (for launch) or the window title as shown in the taskbar (for switch/resize). Example: 'Notepad' to launch, 'Untitled - Notepad' to switch.", json!({"type": "string"}))
                .examples(json!(["Notepad", "Google Chrome", "Untitled - Notepad"])),
            ParamSpec::nullable("loc", "Target [x, y] pixel coordinates for the window's top-left corner. Required for resize mode only.", json!({"type": "array", "items": {"type": "integer"}}))
                .examples(json!([[0, 0]])),
            ParamSpec::nullable("size", "Target [width, height] in pixels for the window dimensions. Required for resize mode only.", json!({"type": "array", "items": {"type": "integer"}}))
                .examples(json!([[1920, 1080]])),
        ]
    }

    pub fn shell() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::string(
                "command",
                "PowerShell command to execute. Runs in the user's HOME directory. Use for file operations, system queries, installations, and automation tasks.",
            )
            .examples(json!([
                "Get-Process",
                "ls Desktop",
                "Get-ChildItem -Path C:\\\\Users -Recurse",
                "echo \"Hello World\""
            ])),
            ParamSpec::integer(
                "timeout",
                "Maximum seconds to wait for command completion. Increase for long-running operations.",
                10,
                false,
            )
            .examples(json!([10, 30, 60])),
        ]
    }

    pub fn type_tool() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::int_array(
                "loc",
                "[x, y] pixel coordinates of the input field to type into. The tool clicks this location automatically before typing — do not pre-click.",
                true,
            )
            .examples(json!([[640, 360], [200, 150]])),
            ParamSpec::string("text", "The text string to type into the element")
                .examples(json!(["hello world", "user@example.com", "search query"])),
            ParamSpec::boolean(
                "clear",
                "If true, selects all existing text and replaces it. If false, appends to whatever is already in the field.",
                false,
            )
            .examples(json!([true, false])),
            ParamSpec::enumerated(
                "caret_position",
                "Where to position the text cursor before typing: 'start' moves to the beginning of the field, 'end' moves to the end, 'idle' leaves it where it is",
                &["start", "idle", "end"],
                Some("idle"),
                false,
            )
            .examples(json!(["start", "end", "idle"])),
            ParamSpec::boolean(
                "press_enter",
                "If true, presses Enter after typing to submit the input (search, form, dialog). If false, leaves the cursor in the field.",
                false,
            )
            .examples(json!([true, false])),
        ]
    }

    pub fn scroll() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::nullable(
                "loc",
                "[x, y] pixel coordinates where scrolling occurs. If omitted, scrolls at the current cursor position.",
                json!({"type": "array", "items": {"type": "integer"}}),
            )
            .examples(json!([[640, 360], [800, 400]])),
            ParamSpec::enumerated(
                "type",
                "Scroll axis: 'vertical' for up/down scrolling, 'horizontal' for left/right scrolling",
                &["horizontal", "vertical"],
                Some("vertical"),
                false,
            )
            .examples(json!(["vertical", "horizontal"])),
            ParamSpec::enumerated(
                "direction",
                "Scroll direction: 'up' or 'down' for vertical, 'left' or 'right' for horizontal",
                &["up", "down", "left", "right"],
                Some("down"),
                false,
            )
            .examples(json!(["down", "up", "right"])),
            ParamSpec::integer(
                "wheel_times",
                "Number of scroll increments. Each increment scrolls roughly 3-5 lines of text. Use 3-5 for moderate scrolling, 10+ for large jumps.",
                1,
                false,
            )
            .examples(json!([1, 3, 5, 10])),
        ]
    }

    pub fn move_tool() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::int_array(
                "loc",
                "[x, y] pixel coordinates to move the mouse cursor to",
                true,
            )
            .examples(json!([[640, 360], [100, 100]])),
            ParamSpec::boolean(
                "drag",
                "If true, holds the left mouse button and drags from the current position to the target (drag-and-drop). If false, moves the cursor without clicking (hover).",
                false,
            )
            .examples(json!([true, false])),
        ]
    }

    pub fn shortcut() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::string(
                "shortcut",
                "Keyboard shortcut to press. Use '+' to combine keys for simultaneous press. Examples: 'ctrl+c' for copy, 'alt+tab' for window switch, 'enter' for confirm, 'escape' for cancel.",
            )
            .examples(json!([
                "enter", "escape", "ctrl+c", "ctrl+v", "alt+tab", "ctrl+shift+n", "win"
            ])),
        ]
    }

    pub fn multi_select() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::boolean(
                "press_ctrl",
                "If true, holds Ctrl while clicking each element to accumulate a multi-selection (files, checkboxes, list items). If false, clicks each location sequentially without Ctrl.",
                false,
            ),
            ParamSpec::array_of_arrays(
                "elements",
                "List of [x, y] pixel coordinates to click. Each coordinate pair is clicked in order.",
                json!({"type": "integer"}),
            )
            .examples(json!([[[640, 360], [800, 400]], [[100, 200], [200, 300]]])),
        ]
    }

    pub fn multi_edit() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::array_of_arrays(
                "elements",
                "List of [x, y, text] entries. For each entry, clicks the location (x, y) and types the text. Use to fill multiple form fields in one action.",
                json!({}),
            )
            .examples(json!([
                [[640, 360, "hello"], [800, 400, "world"]],
                [[100, 200, "John"], [200, 300, "Doe"]]
            ])),
        ]
    }

    pub fn wait() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::integer(
                "duration",
                "Number of seconds to pause. Use to wait for applications to launch, pages to load, or animations to finish before the next action.",
                2,
                true,
            ),
        ]
    }

    pub fn scrape() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::string(
                "url",
                "URL of the webpage currently open in the browser. The tool extracts visible text content from the rendered page via the accessibility tree and returns it as markdown.",
            ),
        ]
    }

    pub fn desktop() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::enumerated(
                "action",
                "Virtual desktop action: 'create' adds a new desktop, 'remove' deletes a desktop by name, 'rename' changes a desktop's name, 'switch' activates a desktop by name",
                &["create", "remove", "rename", "switch"],
                None,
                true,
            ),
            ParamSpec::nullable(
                "desktop_name",
                "Name of the target desktop. Required for remove, rename, and switch. Optional for create (auto-named if omitted).",
                json!({"type": "string"}),
            ),
            ParamSpec::nullable(
                "new_name",
                "New name for the desktop. Required for rename only.",
                json!({"type": "string"}),
            ),
        ]
    }

    pub fn memory() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::enumerated(
                "mode",
                "Operation mode: 'view' lists all stored files, 'read' retrieves file content, 'write' creates a new file, 'update' modifies an existing file, 'delete' removes a file",
                &["view", "read", "write", "delete", "update"],
                None,
                true,
            ),
            ParamSpec::nullable(
                "path",
                "Relative file path from the .windows-use/memories directory (e.g., 'notes.md', 'project/data.md'). Required for read, write, update, and delete.",
                json!({"type": "string"}),
            ),
            ParamSpec::nullable(
                "content",
                "Text content to write into the file. Required for 'write' mode and 'insert' operation in 'update' mode.",
                json!({"type": "string"}),
            ),
            ParamSpec::nullable(
                "operation",
                "Update strategy: 'replace' finds old_str and substitutes it with new_str, 'insert' adds content at a specific line number. Only used in 'update' mode.",
                json!({"type": "string", "enum": ["replace", "insert"]}),
            ),
            ParamSpec::nullable(
                "old_str",
                "Exact string to find in the file. Required when operation='replace'.",
                json!({"type": "string"}),
            ),
            ParamSpec::nullable(
                "new_str",
                "Replacement string. Required when operation='replace'.",
                json!({"type": "string"}),
            ),
            ParamSpec::nullable(
                "line_number",
                "0-indexed line number where content will be inserted. Required when operation='insert'.",
                json!({"type": "integer"}),
            ),
            ParamSpec::nullable(
                "read_range",
                "Optional [start, end] line range for partial reads (0-indexed, end exclusive). Example: [0, 10] reads lines 0 through 9.",
                json!({"type": "array", "items": {"type": "integer"}}),
            ),
        ]
    }

    pub fn file() -> Vec<ParamSpec> {
        vec![
            Self::shared_thought(),
            ParamSpec::enumerated(
                "mode",
                "Operation: 'read' retrieves file content, 'write' creates/overwrites a file, 'list' lists directory contents, 'delete' removes a file or directory, 'move' relocates, 'copy' duplicates, 'exists' checks if path exists",
                &["read", "write", "list", "delete", "move", "copy", "exists"],
                None,
                true,
            ),
            ParamSpec::string(
                "path",
                "Absolute path or path relative to user home directory. Examples: 'C:\\Users\\me\\file.txt', 'Desktop\\notes.txt', 'Documents\\config.json'",
            ),
            ParamSpec::nullable(
                "destination",
                "Target path for move or copy actions. Required when action is 'move' or 'copy'.",
                json!({"type": "string"}),
            ),
            ParamSpec::nullable(
                "content",
                "Text content to write. Required when action is 'write'.",
                json!({"type": "string"}),
            ),
            ParamSpec::nullable(
                "encoding",
                "Character encoding for read and write. Defaults to utf-8.",
                json!({"type": "string", "default": "utf-8"}),
            ),
        ]
    }
}

/// Tool descriptions matching the Python docstrings.
pub mod descriptions {
    pub const DONE: &str = "Delivers a response to the user. This is the ONLY way to communicate with the user.\n\nMUST be called for every type of response:\n- Task completion: summarize what was accomplished\n- Answers to questions: provide the requested information\n- Conversational replies: greetings, clarifications, explanations\n- Error reports: explain what failed and why\n\nThe answer should be formatted in clean markdown.";

    pub const APP: &str = "Manages application windows: launch new apps, switch between open windows, or resize/reposition the active window.\n\n- launch: Opens an application via the Start Menu. Provide the app name as it appears in Start Menu.\n- switch: Brings an already-open window to the foreground. Provide the window title from the Window Info list.\n- resize: Moves and resizes the currently active window. Provide loc=[x,y] for position and size=[w,h] for dimensions.";

    pub const MEMORY: &str = "Persistent file-based storage for saving and retrieving information across steps. Files are stored as markdown in the .windows-use/memories directory (user home).\n\n- view: List all stored memory files. Optionally provide a subdirectory path to list only files within it.\n- write: Create a new file. Requires path and content. Path must end with .md extension.\n- read: Retrieve file contents. Optionally use read_range=[start, end] for partial reads.\n- update: Modify an existing file. Use operation='replace' with old_str/new_str, or operation='insert' with line_number/content.\n- delete: Remove a file by path.\n\nUse for storing intermediate results, research findings, plans, or any data needed in later steps.";

    pub const FILE: &str = "Performs file system operations: read, write, list, delete, move, copy, or check existence.\n\n- read: Returns file content. Paths are relative to user home directory or absolute.\n- write: Creates or overwrites a file. Requires content.\n- list: Lists directory contents (files and subdirectories).\n- delete: Removes a file or empty directory.\n- move: Moves a file or directory to destination.\n- copy: Copies a file to destination.\n- exists: Returns whether the path exists.\n\nUse for reading config files, writing outputs, organizing files, or checking file presence.";

    pub const SHELL: &str = "Executes a PowerShell command and returns the output and exit status code. Working directory is the user's HOME.\n\nUse for file operations, system queries, installations, running scripts, and any task better done via command line than GUI. Check the status code in the response: 0 means success, non-zero means failure.";

    pub const CLICK: &str = "Clicks at the specified pixel coordinates on screen.\n\n- Single left click (clicks=1): Select elements, press buttons, focus fields, follow links.\n- Double left click (clicks=2): Open files, folders, and desktop icons.\n- Right click (button='right'): Open context menus.\n- Hover only (clicks=0): Move cursor to location without clicking.\n\nUse coordinates from the Interactive Elements list in the Desktop State.";

    pub const TYPE: &str = "Clicks an input field and types text into it. Do NOT pre-click with click_tool — this tool handles focusing automatically.\n\n- Set clear=true to replace existing text, or clear=false to append.\n- Set press_enter=true to submit after typing (search bars, forms, dialogs).\n- Set caret_position to control where typing begins relative to existing text.\n\nUse for search queries, form fields, text editors, address bars, and any text input.";

    pub const SCROLL: &str = "Scrolls content at the specified location or at the current cursor position.\n\nEach wheel increment scrolls roughly 3-5 lines of text. Use wheel_times=3-5 for moderate scrolling, 10+ for large jumps.\n\nCheck scroll percentages in the Scrollable Elements list to gauge position before scrolling. If loc is omitted, scrolling occurs at the current cursor location.";

    pub const MOVE: &str = "Moves the mouse cursor to a target location, or drags from the current position to the target.\n\n- drag=false: Hover over elements to reveal tooltips, dropdown menus, or reposition the cursor.\n- drag=true: Hold left mouse button from the current cursor position and drag to the target coordinates. Use for drag-and-drop of files, window resizing, slider adjustment, or reordering items.";

    pub const SHORTCUT: &str = "Presses a keyboard shortcut. Use '+' to combine keys (e.g., 'ctrl+c').\n\nCommon shortcuts: ctrl+c (copy), ctrl+v (paste), ctrl+z (undo), ctrl+s (save), ctrl+a (select all), ctrl+f (find), ctrl+w (close tab), ctrl+t (new tab), alt+tab (switch window), alt+f4 (close app), enter (confirm), escape (cancel), win (start menu).\n\nPrefer shortcuts over mouse interactions when the operation is unambiguous.";

    pub const MULTI_SELECT: &str = "Clicks multiple locations in sequence. With press_ctrl=true, holds Ctrl to accumulate a multi-selection (e.g., selecting multiple files, checkboxes, or list items). With press_ctrl=false, clicks each location independently in order.\n\nProvide a list of [x, y] coordinates. Each is clicked once in the order given.";

    pub const MULTI_EDIT: &str = "Types text into multiple input fields in one action. Each entry is [x, y, text]: the tool clicks the location and types the text, then moves to the next entry.\n\nUse for filling out forms with multiple fields (name, email, address) or editing several text inputs at once. More efficient than calling type_tool repeatedly.";

    pub const WAIT: &str = "Pauses for the specified number of seconds before the next action. Use to wait for applications to launch, pages to load, dialogs to appear, or animations to finish. Typical waits: 2-3s for UI transitions, 5s for app launches, 10s+ for installations or downloads.";

    pub const SCRAPE: &str = "Extracts visible text content from the webpage currently displayed in the browser and returns it as markdown.\n\nThis reads the rendered page content via the accessibility tree — not the raw HTML. Provide the URL of the page currently open in the browser. The output includes scroll position indicators so you know if there is more content above or below.\n\nUse when you need to read, analyze, or extract information from a webpage.";

    pub const DESKTOP: &str = "Manages Windows virtual desktops for workspace organization.\n\n- create: Creates a new virtual desktop. Optionally provide a name via desktop_name.\n- remove: Deletes the desktop specified by desktop_name. Ensure no needed windows remain on it.\n- rename: Renames desktop_name to new_name.\n- switch: Activates the desktop specified by desktop_name. Verify success in the next Desktop State.";
}

/// The agent's tool set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Done,
    App,
    Memory,
    File,
    Shell,
    Click,
    Type,
    Scroll,
    Move,
    Shortcut,
    MultiSelect,
    MultiEdit,
    Wait,
    Scrape,
    Desktop,
}

impl ToolKind {
    pub fn name(&self) -> &'static str {
        match self {
            ToolKind::Done => "done_tool",
            ToolKind::App => "app_tool",
            ToolKind::Memory => "memory_tool",
            ToolKind::File => "file_tool",
            ToolKind::Shell => "shell_tool",
            ToolKind::Click => "click_tool",
            ToolKind::Type => "type_tool",
            ToolKind::Scroll => "scroll_tool",
            ToolKind::Move => "move_tool",
            ToolKind::Shortcut => "shortcut_tool",
            ToolKind::MultiSelect => "multi_select_tool",
            ToolKind::MultiEdit => "multi_edit_tool",
            ToolKind::Wait => "wait_tool",
            ToolKind::Scrape => "scrape_tool",
            ToolKind::Desktop => "desktop_tool",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            ToolKind::Done => descriptions::DONE,
            ToolKind::App => descriptions::APP,
            ToolKind::Memory => descriptions::MEMORY,
            ToolKind::File => descriptions::FILE,
            ToolKind::Shell => descriptions::SHELL,
            ToolKind::Click => descriptions::CLICK,
            ToolKind::Type => descriptions::TYPE,
            ToolKind::Scroll => descriptions::SCROLL,
            ToolKind::Move => descriptions::MOVE,
            ToolKind::Shortcut => descriptions::SHORTCUT,
            ToolKind::MultiSelect => descriptions::MULTI_SELECT,
            ToolKind::MultiEdit => descriptions::MULTI_EDIT,
            ToolKind::Wait => descriptions::WAIT,
            ToolKind::Scrape => descriptions::SCRAPE,
            ToolKind::Desktop => descriptions::DESKTOP,
        }
    }

    pub fn params(&self) -> Vec<ParamSpec> {
        match self {
            ToolKind::Done => ToolSpecs::done(),
            ToolKind::App => ToolSpecs::app(),
            ToolKind::Memory => ToolSpecs::memory(),
            ToolKind::File => ToolSpecs::file(),
            ToolKind::Shell => ToolSpecs::shell(),
            ToolKind::Click => ToolSpecs::click(),
            ToolKind::Type => ToolSpecs::type_tool(),
            ToolKind::Scroll => ToolSpecs::scroll(),
            ToolKind::Move => ToolSpecs::move_tool(),
            ToolKind::Shortcut => ToolSpecs::shortcut(),
            ToolKind::MultiSelect => ToolSpecs::multi_select(),
            ToolKind::MultiEdit => ToolSpecs::multi_edit(),
            ToolKind::Wait => ToolSpecs::wait(),
            ToolKind::Scrape => ToolSpecs::scrape(),
            ToolKind::Desktop => ToolSpecs::desktop(),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "done_tool" => ToolKind::Done,
            "app_tool" => ToolKind::App,
            "memory_tool" => ToolKind::Memory,
            "file_tool" => ToolKind::File,
            "shell_tool" => ToolKind::Shell,
            "click_tool" => ToolKind::Click,
            "type_tool" => ToolKind::Type,
            "scroll_tool" => ToolKind::Scroll,
            "move_tool" => ToolKind::Move,
            "shortcut_tool" => ToolKind::Shortcut,
            "multi_select_tool" => ToolKind::MultiSelect,
            "multi_edit_tool" => ToolKind::MultiEdit,
            "wait_tool" => ToolKind::Wait,
            "scrape_tool" => ToolKind::Scrape,
            "desktop_tool" => ToolKind::Desktop,
            _ => return None,
        })
    }
}

/// Built-in tools, matching `BUILTIN_TOOLS`.
pub const BUILTIN_TOOLS: [ToolKind; 11] = [
    ToolKind::Done,
    ToolKind::App,
    ToolKind::Shell,
    ToolKind::Click,
    ToolKind::Type,
    ToolKind::Scroll,
    ToolKind::Move,
    ToolKind::Shortcut,
    ToolKind::Wait,
    ToolKind::Scrape,
    ToolKind::Desktop,
];

/// Experimental tools, matching `EXPERIMENTAL_TOOLS`.
pub const EXPERIMENTAL_TOOLS: [ToolKind; 4] =
    [ToolKind::File, ToolKind::Memory, ToolKind::MultiSelect, ToolKind::MultiEdit];

fn string_arg(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(|value| value.to_string())
}

fn bool_arg(args: &Map<String, Value>, key: &str, default: bool) -> bool {
    match args.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(text)) => text == "true",
        _ => default,
    }
}

fn int_arg(args: &Map<String, Value>, key: &str, default: i64) -> i64 {
    args.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn loc_arg(args: &Map<String, Value>, key: &str) -> Option<(i32, i32)> {
    let array = args.get(key)?.as_array()?;
    if array.len() < 2 {
        return None;
    }
    Some((
        array[0].as_i64()? as i32,
        array[1].as_i64()? as i32,
    ))
}

fn truncate_output(content: String) -> String {
    if content.chars().count() > 10_000 {
        content.chars().take(10_000).collect()
    } else {
        content
    }
}

/// Executes one tool, returning output or an error message.
pub fn execute_tool(
    kind: ToolKind,
    args: &Map<String, Value>,
    desktop: Option<&mut Desktop>,
) -> Result<String, String> {
    match kind {
        ToolKind::Done => Ok(string_arg(args, "answer").unwrap_or_default()),
        ToolKind::Wait => {
            let duration = int_arg(args, "duration", 0).max(0) as u64;
            std::thread::sleep(std::time::Duration::from_secs(duration));
            Ok(format!("Waited for {duration} seconds."))
        }
        ToolKind::App => {
            let desktop = desktop.ok_or("No desktop available")?;
            let mode = string_arg(args, "mode").unwrap_or_else(|| "launch".to_string());
            let request = AppRequest {
                mode: match mode.as_str() {
                    "resize" => AppMode::Resize,
                    "switch" => AppMode::Switch,
                    _ => AppMode::Launch,
                },
                name: string_arg(args, "name"),
                loc: loc_arg(args, "loc"),
                size: loc_arg(args, "size"),
            };
            Ok(desktop.app(&request))
        }
        ToolKind::Shell => {
            let desktop = desktop.ok_or("No desktop available")?;
            let command = string_arg(args, "command").unwrap_or_default();
            let timeout = int_arg(args, "timeout", 10).max(1) as u64;
            let (response, status) = desktop.execute_command(&command, timeout);
            Ok(format!("Response: {response}\nStatus Code: {status}"))
        }
        ToolKind::Click => {
            let desktop = desktop.ok_or("No desktop available")?;
            let (x, y) = loc_arg(args, "loc").ok_or("loc is required")?;
            let button = string_arg(args, "button").unwrap_or_else(|| "left".to_string());
            let clicks = int_arg(args, "clicks", 1).max(0) as u32;
            desktop.click((x, y), &button, clicks);
            let label = match clicks {
                1 => "Single",
                2 => "Double",
                3 => "Triple",
                _ => "",
            };
            Ok(format!("{label} {button} clicked at ({x},{y})."))
        }
        ToolKind::Type => {
            let desktop = desktop.ok_or("No desktop available")?;
            let (x, y) = loc_arg(args, "loc").ok_or("loc is required")?;
            let text = string_arg(args, "text").unwrap_or_default();
            let clear = bool_arg(args, "clear", false);
            let caret = string_arg(args, "caret_position").unwrap_or_else(|| "idle".to_string());
            let press_enter = bool_arg(args, "press_enter", false);
            desktop.type_text((x, y), &text, &caret, clear, press_enter);
            Ok(format!("Typed {text} at ({x},{y})."))
        }
        ToolKind::Scroll => {
            let desktop = desktop.ok_or("No desktop available")?;
            let loc = loc_arg(args, "loc");
            let scroll_type = string_arg(args, "type").unwrap_or_else(|| "vertical".to_string());
            let direction = string_arg(args, "direction").unwrap_or_else(|| "down".to_string());
            let wheel_times = int_arg(args, "wheel_times", 1).max(0) as i32;
            match desktop.scroll(loc, &scroll_type, &direction, wheel_times) {
                Some(error) => Ok(error),
                None => Ok(format!(
                    "Scrolled {scroll_type} {direction} by {wheel_times} wheel times."
                )),
            }
        }
        ToolKind::Move => {
            let desktop = desktop.ok_or("No desktop available")?;
            let (x, y) = loc_arg(args, "loc").ok_or("loc is required")?;
            if bool_arg(args, "drag", false) {
                desktop.drag((x, y));
                Ok(format!("Dragged the selected element to ({x},{y})."))
            } else {
                desktop.move_mouse((x, y));
                Ok(format!("Moved the mouse pointer to ({x},{y})."))
            }
        }
        ToolKind::Shortcut => {
            let desktop = desktop.ok_or("No desktop available")?;
            let shortcut = string_arg(args, "shortcut").unwrap_or_default();
            desktop.shortcut(&shortcut);
            Ok(format!("Pressed {shortcut}."))
        }
        ToolKind::MultiSelect => {
            let desktop = desktop.ok_or("No desktop available")?;
            let press_ctrl = bool_arg(args, "press_ctrl", true);
            let elements: Vec<(i32, i32)> = args
                .get("elements")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            let array = item.as_array()?;
                            Some((array.first()?.as_i64()? as i32, array.get(1)?.as_i64()? as i32))
                        })
                        .collect()
                })
                .unwrap_or_default();
            desktop.multi_select(press_ctrl, &elements);
            let rendered = elements
                .iter()
                .map(|(x, y)| format!("({x},{y})"))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!("Multi-selected elements at {rendered}."))
        }
        ToolKind::MultiEdit => {
            let desktop = desktop.ok_or("No desktop available")?;
            let elements: Vec<(i32, i32, String)> = args
                .get("elements")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            let array = item.as_array()?;
                            Some((
                                array.first()?.as_i64()? as i32,
                                array.get(1)?.as_i64()? as i32,
                                array.get(2)?.as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            desktop.multi_edit(&elements);
            let rendered = elements
                .iter()
                .map(|(x, y, text)| format!("({x},{y}) text={text}"))
                .collect::<Vec<_>>()
                .join(",");
            Ok(format!("Multi-edited elements at {rendered}."))
        }
        ToolKind::Scrape => {
            let desktop = desktop.ok_or("No desktop available")?;
            let url = string_arg(args, "url").unwrap_or_default();
            let state = desktop
                .desktop_state
                .as_ref()
                .ok_or("No desktop state available")?;
            let tree_state = state.tree_state.as_ref().ok_or("No tree state available")?;

            let Some(dom_node) = &tree_state.dom_node else {
                // Blocking scrape via a short-lived runtime keeps the sync tool path.
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())?;
                let content = runtime
                    .block_on(desktop.scrape(&url))
                    .map_err(|error| error.to_string())?;
                return Ok(format!("URL:{url}\nContent:\n{content}"));
            };

            let vertical = dom_node
                .metadata
                .get("vertical_scroll_percent")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let content = tree_state
                .dom_informative_nodes
                .iter()
                .map(|node| node.text.clone())
                .collect::<Vec<_>>()
                .join("\n");
            let header = if vertical <= 0.0 {
                "[Reached top]".to_string()
            } else {
                format!("[Scroll up ({:.2}%) to see more content]", vertical * 100.0)
            };
            let footer = if vertical >= 100.0 {
                "[Reached bottom]".to_string()
            } else {
                format!(
                    "[Scroll down ({:.2}%) to see more content]",
                    (1.0 - vertical) * 100.0
                )
            };
            Ok(format!("URL:{url}\nContent:\n{header}\n{content}\n{footer}"))
        }
        ToolKind::Desktop => {
            let action = string_arg(args, "action").unwrap_or_default();
            let desktop_name = string_arg(args, "desktop_name");
            let new_name = string_arg(args, "new_name");
            match action.as_str() {
                "create" => match crate::vdm::create_desktop(desktop_name.as_deref()) {
                    Ok(name) => Ok(format!("Created desktop: '{name}'")),
                    Err(error) => Ok(format!("Error executing desktop action 'create': {error}")),
                },
                "remove" => {
                    let Some(name) = desktop_name else {
                        return Ok("Error: desktop_name is required for removal.".to_string());
                    };
                    match crate::vdm::remove_desktop(&name) {
                        Ok(()) => Ok(format!("Removed desktop '{name}'")),
                        Err(error) => Ok(format!("Error executing desktop action 'remove': {error}")),
                    }
                }
                "rename" => {
                    let (Some(name), Some(new_name)) = (desktop_name, new_name) else {
                        return Ok(
                            "Error: desktop_name and new_name are required for rename.".to_string()
                        );
                    };
                    match crate::vdm::rename_desktop(&name, &new_name) {
                        Ok(()) => Ok(format!("Renamed desktop '{name}' to '{new_name}'")),
                        Err(error) => Ok(format!("Error executing desktop action 'rename': {error}")),
                    }
                }
                "switch" => {
                    let Some(name) = desktop_name else {
                        return Ok("Error: desktop_name is required for switching.".to_string());
                    };
                    match crate::vdm::switch_desktop(&name) {
                        Ok(()) => Ok(format!("Switched to desktop '{name}'")),
                        Err(error) => Ok(format!("Error executing desktop action 'switch': {error}")),
                    }
                }
                other => Ok(format!("Error: Unknown action: {other}")),
            }
        }
        ToolKind::Memory => execute_memory(args),
        ToolKind::File => execute_file(args),
    }
}

fn execute_memory(args: &Map<String, Value>) -> Result<String, String> {
    let mode = string_arg(args, "mode").unwrap_or_default();
    let path = string_arg(args, "path");
    let root = memory_root();

    match mode.as_str() {
        "view" => {
            let search_dir = match &path {
                Some(path) => resolve_memory_path(path).map_err(|error| error.to_string())?,
                None => root.clone(),
            };
            if !search_dir.exists() {
                return Ok("No memory files found.".to_string());
            }
            let mut files: Vec<PathBuf> = Vec::new();
            collect_markdown_files(&search_dir, &mut files);
            if files.is_empty() {
                return Ok("No memory files found.".to_string());
            }
            let listing = files
                .iter()
                .enumerate()
                .map(|(index, file)| {
                    let relative = file
                        .strip_prefix(&root)
                        .map(|value| value.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|_| file.to_string_lossy().to_string());
                    format!("{}. {}", index + 1, relative)
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(listing)
        }
        "write" => {
            let Some(mut path) = path else {
                return Ok("Error: path is required for write mode.".to_string());
            };
            if !path.ends_with(".md") {
                path.push_str(".md");
            }
            let file_path = resolve_memory_path(&path).map_err(|error| error.to_string())?;
            if let Some(parent) = file_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let content = string_arg(args, "content").unwrap_or_default();
            std::fs::write(&file_path, content).map_err(|error| error.to_string())?;
            let parent = file_path
                .parent()
                .and_then(|parent| parent.strip_prefix(&root).ok())
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            Ok(format!(
                "{} created in {}.",
                file_path
                    .file_name()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default(),
                if parent.is_empty() { ".".to_string() } else { parent }
            ))
        }
        "read" => {
            let Some(path) = path else {
                return Ok("Error: path is required for read mode.".to_string());
            };
            let file_path = resolve_memory_path(&path).map_err(|error| error.to_string())?;
            if !file_path.exists() {
                return Ok(format!(
                    "Error: {} not found.",
                    file_path
                        .file_name()
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_default()
                ));
            }
            let content = std::fs::read_to_string(&file_path).map_err(|error| error.to_string())?;
            let relative = file_path
                .strip_prefix(&root)
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();

            if let Some(range) = args.get("read_range").and_then(Value::as_array) {
                if range.len() >= 2 {
                    let start = range[0].as_i64().unwrap_or(0) as isize;
                    let end = range[1].as_i64().unwrap_or(0) as isize;
                    let lines: Vec<&str> = content.lines().collect();
                    if start < 0 || start >= lines.len() as isize {
                        return Ok(format!(
                            "Error: start line {start} out of range (0-{}).",
                            lines.len().saturating_sub(1)
                        ));
                    }
                    if end < start || end > lines.len() as isize {
                        return Ok(format!(
                            "Error: end line {end} out of range ({start}-{}).",
                            lines.len()
                        ));
                    }
                    let selected = lines[start as usize..end as usize].join("\n");
                    return Ok(format!("File: {relative}\nLines {start}-{}:\n{selected}", end - 1));
                }
            }
            Ok(format!("File: {relative}\nContent:\n{content}"))
        }
        "update" => {
            let Some(path) = path else {
                return Ok("Error: path is required for update mode.".to_string());
            };
            let file_path = resolve_memory_path(&path).map_err(|error| error.to_string())?;
            if !file_path.exists() {
                return Ok(format!(
                    "Error: {} not found. Use \"write\" mode to create a new file.",
                    file_path
                        .file_name()
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_default()
                ));
            }
            let current = std::fs::read_to_string(&file_path).map_err(|error| error.to_string())?;
            let operation = string_arg(args, "operation").unwrap_or_else(|| "replace".to_string());
            let name = file_path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default();

            match operation.as_str() {
                "replace" => {
                    let (Some(old_str), Some(new_str)) =
                        (string_arg(args, "old_str"), string_arg(args, "new_str"))
                    else {
                        return Ok(
                            "Error: both old_str and new_str are required for replace operation."
                                .to_string(),
                        );
                    };
                    if !current.contains(&old_str) {
                        return Ok(format!("Error: \"{old_str}\" not found in file."));
                    }
                    let updated = current.replacen(&old_str, &new_str, 1);
                    std::fs::write(&file_path, updated).map_err(|error| error.to_string())?;
                    let old_display = format!("\"{}\"", truncate_display(&old_str));
                    let new_display = format!("\"{}\"", truncate_display(&new_str));
                    Ok(format!(
                        "{name} updated: replaced {old_display} with {new_display}."
                    ))
                }
                "insert" => {
                    let Some(line_number) = args.get("line_number").and_then(Value::as_i64) else {
                        return Ok("Error: line_number is required for insert operation.".to_string());
                    };
                    let Some(content) = string_arg(args, "content") else {
                        return Ok("Error: content is required for insert operation.".to_string());
                    };
                    let mut lines: Vec<String> =
                        current.split_inclusive('\n').map(|line| line.to_string()).collect();
                    if lines.is_empty() {
                        lines.push(String::new());
                    }
                    if line_number < 0 || line_number as usize > lines.len() {
                        return Ok(format!(
                            "Error: line_number {line_number} out of range (0-{}).",
                            lines.len()
                        ));
                    }
                    let insert = if content.ends_with('\n') {
                        content
                    } else {
                        format!("{content}\n")
                    };
                    lines.insert(line_number as usize, insert);
                    std::fs::write(&file_path, lines.concat()).map_err(|error| error.to_string())?;
                    Ok(format!("{name} updated: inserted content at line {line_number}."))
                }
                other => Ok(format!("Error: Unknown operation \"{other}\".")),
            }
        }
        "delete" => {
            let Some(path) = path else {
                return Ok("Error: path is required for delete mode.".to_string());
            };
            let file_path = resolve_memory_path(&path).map_err(|error| error.to_string())?;
            if !file_path.exists() {
                return Ok(format!(
                    "Error: {} not found.",
                    file_path
                        .file_name()
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_default()
                ));
            }
            std::fs::remove_file(&file_path).map_err(|error| error.to_string())?;
            let parent = file_path
                .parent()
                .and_then(|parent| parent.strip_prefix(&root).ok())
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| ".".to_string());
            Ok(format!(
                "{} deleted from {parent}.",
                file_path
                    .file_name()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default()
            ))
        }
        _ => Ok("Invalid mode. Use 'view', 'write', 'read', 'update', or 'delete'.".to_string()),
    }
}

fn truncate_display(text: &str) -> String {
    if text.chars().count() > 50 {
        format!("{}...", text.chars().take(50).collect::<String>())
    } else {
        text.to_string()
    }
}

fn collect_markdown_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files.sort();
}

fn execute_file(args: &Map<String, Value>) -> Result<String, String> {
    let mode = string_arg(args, "mode").unwrap_or_default();
    let raw_path = string_arg(args, "path").unwrap_or_default();
    let resolved = match resolve_file_path(&raw_path) {
        Ok(path) => path,
        Err(error) => return Ok(format!("Error: {error}")),
    };

    match mode.as_str() {
        "read" => {
            if !resolved.exists() {
                return Ok(format!("Error: Path does not exist: {raw_path}"));
            }
            if resolved.is_dir() {
                return Ok(format!("Error: Cannot read directory as file: {raw_path}"));
            }
            let metadata = std::fs::metadata(&resolved).map_err(|error| error.to_string())?;
            if metadata.len() > MAX_FILE_READ_SIZE {
                return Ok(format!(
                    "Error: File too large to read ({} bytes, max {}).",
                    metadata.len(),
                    MAX_FILE_READ_SIZE
                ));
            }
            match std::fs::read_to_string(&resolved) {
                Ok(text) => Ok(format!("File: {}\nContent:\n{text}", resolved.display())),
                Err(error) => Ok(format!("Error reading file: {error}")),
            }
        }
        "write" => {
            let Some(content) = string_arg(args, "content") else {
                return Ok("Error: content is required for write action.".to_string());
            };
            if let Some(parent) = resolved.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&resolved, &content) {
                Ok(()) => Ok(format!(
                    "Wrote {} characters to {}.",
                    content.chars().count(),
                    resolved.display()
                )),
                Err(error) => Ok(format!("Error writing file: {error}")),
            }
        }
        "list" => {
            if !resolved.exists() {
                return Ok(format!("Error: Path does not exist: {raw_path}"));
            }
            if !resolved.is_dir() {
                return Ok(format!("Error: Not a directory: {raw_path}"));
            }
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&resolved)
                .map_err(|error| error.to_string())?
                .flatten()
                .map(|entry| entry.path())
                .collect();
            entries.sort_by(|left, right| {
                let left_dir = left.is_dir();
                let right_dir = right.is_dir();
                let left_name = left
                    .file_name()
                    .map(|value| value.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let right_name = right
                    .file_name()
                    .map(|value| value.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                (!left_dir, left_name).cmp(&(!right_dir, right_name))
            });

            let mut lines = Vec::new();
            let mut table: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for entry in entries {
                let Ok(metadata) = entry.metadata() else {
                    lines.push(format!("?  {:>19}  {:>10}  {}", "?", "?", entry.display()));
                    continue;
                };
                let modified = metadata
                    .modified()
                    .ok()
                    .map(|time| {
                        let datetime: chrono::DateTime<chrono::Local> = time.into();
                        datetime.format("%Y-%m-%d %H:%M").to_string()
                    })
                    .unwrap_or_else(|| "?".to_string());
                let size = if entry.is_dir() { 0 } else { metadata.len() };
                let kind = if entry.is_dir() { "d" } else { "-" };
                let name = entry
                    .file_name()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default();
                lines.push(format!(
                    "{kind}  {modified}  {:>10}  {name}",
                    format_size(size)
                ));
                table.insert(name, vec![kind.to_string()]);
            }
            let header = format!("{:<4} {:<17} {:>10}  Name", "Type", "Modified", "Size");
            Ok(format!(
                "Directory: {}\n\n{header}\n{}\n{}",
                resolved.display(),
                "-".repeat(45),
                lines.join("\n")
            ))
        }
        "delete" => {
            if !resolved.exists() {
                return Ok(format!("Error: Path does not exist: {raw_path}"));
            }
            if resolved.is_dir() {
                match std::fs::remove_dir(&resolved) {
                    Ok(()) => Ok(format!("Removed directory: {}", resolved.display())),
                    Err(error) => Ok(format!("Error deleting: {error}")),
                }
            } else {
                match std::fs::remove_file(&resolved) {
                    Ok(()) => Ok(format!("Deleted file: {}", resolved.display())),
                    Err(error) => Ok(format!("Error deleting: {error}")),
                }
            }
        }
        "move" => {
            let Some(destination) = string_arg(args, "destination") else {
                return Ok("Error: destination is required for move action.".to_string());
            };
            let destination = match resolve_file_path(&destination) {
                Ok(path) => path,
                Err(error) => return Ok(format!("Error: {error}")),
            };
            match std::fs::rename(&resolved, &destination) {
                Ok(()) => Ok(format!(
                    "Moved {} to {}.",
                    resolved.display(),
                    destination.display()
                )),
                Err(error) => Ok(format!("Error moving: {error}")),
            }
        }
        "copy" => {
            let Some(destination) = string_arg(args, "destination") else {
                return Ok("Error: destination is required for copy action.".to_string());
            };
            if resolved.is_dir() {
                return Ok(
                    "Error: copy does not support directories. Use move for directories."
                        .to_string(),
                );
            }
            let destination = match resolve_file_path(&destination) {
                Ok(path) => path,
                Err(error) => return Ok(format!("Error: {error}")),
            };
            match std::fs::copy(&resolved, &destination) {
                Ok(_) => Ok(format!(
                    "Copied {} to {}.",
                    resolved.display(),
                    destination.display()
                )),
                Err(error) => Ok(format!("Error copying: {error}")),
            }
        }
        "exists" => Ok(format!("Path exists: {}", resolved.exists())),
        other => Ok(format!("Error: Unknown action '{other}'.")),
    }
}

/// Trims tool output like the Python `MAX_TOOL_OUTPUT_LENGTH` guard.
pub fn trim_output(content: &str) -> String {
    truncate_output(content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_formatting_matches_python() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1 KB");
        assert_eq!(format_size(1536), "1 KB");
        assert_eq!(format_size(1024 * 1024), "1 MB");
    }

    #[test]
    fn memory_path_escapes_are_rejected() {
        assert!(resolve_memory_path("../secret.md").is_err());
        assert!(resolve_memory_path("notes/../../secret.md").is_err());
        assert!(resolve_memory_path("notes/today.md").is_ok());
    }

    #[test]
    fn file_paths_resolve_against_home() {
        let home = crate::win32::home_dir();
        let resolved = resolve_file_path("Desktop\\notes.txt").expect("resolve");
        assert!(resolved.starts_with(home));
        assert!(resolve_file_path("bad\0path").is_err());
    }

    #[test]
    fn done_tool_returns_the_answer() {
        let mut args = Map::new();
        args.insert("answer".to_string(), json!("hello"));
        assert_eq!(execute_tool(ToolKind::Done, &args, None).unwrap(), "hello");
    }

    #[test]
    fn builtin_tool_names_match_python() {
        let names: Vec<&str> = BUILTIN_TOOLS.iter().map(|tool| tool.name()).collect();
        assert_eq!(
            names,
            vec![
                "done_tool",
                "app_tool",
                "shell_tool",
                "click_tool",
                "type_tool",
                "scroll_tool",
                "move_tool",
                "shortcut_tool",
                "wait_tool",
                "scrape_tool",
                "desktop_tool",
            ]
        );
        assert_eq!(EXPERIMENTAL_TOOLS.len(), 4);
    }
}
