//! Tree state models, mirroring `windows_use/agent/tree/views.py`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Warning shown when UIA capture fails.
pub const WARNING_MESSAGE: &str =
    "The desktop UI services are temporarily unavailable. Please wait a few seconds and continue.";
/// Text shown when no elements are found.
pub const EMPTY_MESSAGE: &str = "No elements found";

/// Screen rectangle matching the Python `BoundingBox`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BoundingBox {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub width: i32,
    pub height: i32,
}

impl BoundingBox {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { left, top, right, bottom, width: right - left, height: bottom - top }
    }

    pub fn from_rectangle(rectangle: &crate::uia::BoundingRectangle) -> Self {
        Self {
            left: rectangle.left,
            top: rectangle.top,
            right: rectangle.right,
            bottom: rectangle.bottom,
            width: rectangle.width(),
            height: rectangle.height(),
        }
    }

    pub fn get_center(&self) -> Center {
        Center {
            x: self.left + self.width.div_euclid(2),
            y: self.top + self.height.div_euclid(2),
        }
    }

    pub fn xywh_to_string(&self) -> String {
        format!("({},{},{},{})", self.left, self.top, self.width, self.height)
    }

    pub fn xyxy_to_string(&self) -> String {
        let (x1, y1, x2, y2) = self.convert_xywh_to_xyxy();
        format!("({x1},{y1},{x2},{y2})")
    }

    pub fn convert_xywh_to_xyxy(&self) -> (i32, i32, i32, i32) {
        (self.left, self.top, self.left + self.width, self.top + self.height)
    }
}

/// Screen point matching the Python `Center`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Center {
    pub x: i32,
    pub y: i32,
}

impl Center {
    pub fn to_string(&self) -> String {
        format!("({},{})", self.x, self.y)
    }
}

/// An interactive element in the accessibility tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TreeElementNode {
    pub name: String,
    pub control_type: String,
    pub bounding_box: BoundingBox,
    pub center: Center,
    pub window_name: String,
    pub hwnd: i64,
    /// Index into the traversal's live control list (replaces the Python `control` reference).
    pub control_index: Option<usize>,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

/// A scrollable container in the accessibility tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScrollElementNode {
    pub name: String,
    pub control_type: String,
    pub window_name: String,
    pub bounding_box: BoundingBox,
    pub center: Center,
    pub horizontal_scrollable: bool,
    pub horizontal_scroll_percent: f64,
    pub vertical_scrollable: bool,
    pub vertical_scroll_percent: f64,
    pub hwnd: i64,
    pub control_index: Option<usize>,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

/// Visible text captured from a browser DOM.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextElementNode {
    pub text: String,
}

/// The captured state of one desktop tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeState {
    pub status: bool,
    pub root_node: Option<TreeElementNode>,
    pub dom_node: Option<ScrollElementNode>,
    pub interactive_nodes: Vec<TreeElementNode>,
    pub scrollable_nodes: Vec<ScrollElementNode>,
    pub dom_informative_nodes: Vec<TextElementNode>,
}

impl Default for TreeState {
    fn default() -> Self {
        Self {
            status: true,
            root_node: None,
            dom_node: None,
            interactive_nodes: Vec::new(),
            scrollable_nodes: Vec::new(),
            dom_informative_nodes: Vec::new(),
        }
    }
}

/// Renders metadata the way Python's `json.dumps` does for the common cases.
fn metadata_to_string(metadata: &BTreeMap<String, serde_json::Value>) -> String {
    let mut parts = Vec::with_capacity(metadata.len());
    for (key, value) in metadata {
        let rendered = match value {
            serde_json::Value::String(text) => {
                let mut escaped = String::with_capacity(text.len() + 2);
                escaped.push('"');
                for character in text.chars() {
                    match character {
                        '"' => escaped.push_str("\\\""),
                        '\\' => escaped.push_str("\\\\"),
                        '\n' => escaped.push_str("\\n"),
                        '\r' => escaped.push_str("\\r"),
                        '\t' => escaped.push_str("\\t"),
                        c if (c as u32) < 0x20 => {
                            escaped.push_str(&format!("\\u{:04x}", c as u32));
                        }
                        c => escaped.push(c),
                    }
                }
                escaped.push('"');
                escaped
            }
            serde_json::Value::Bool(boolean) => boolean.to_string(),
            serde_json::Value::Array(items) => {
                let joined: Vec<String> = items
                    .iter()
                    .map(|item| match item {
                        serde_json::Value::String(text) => format!("\"{text}\""),
                        other => other.to_string(),
                    })
                    .collect();
                format!("[{}]", joined.join(", "))
            }
            serde_json::Value::Null => "null".to_string(),
            other => other.to_string(),
        };
        parts.push(format!("\"{key}\": {rendered}"));
    }
    format!("{{{}}}", parts.join(", "))
}

impl TreeState {
    /// Formats the interactive elements table exactly as the Python port does.
    pub fn interactive_elements_to_string(&self) -> String {
        if !self.status {
            return WARNING_MESSAGE.to_string();
        }
        if self.interactive_nodes.is_empty() {
            return EMPTY_MESSAGE.to_string();
        }

        let mut rows = Vec::with_capacity(self.interactive_nodes.len() + 1);
        rows.push("# id|window|control_type|name|coords|metadata".to_string());
        for (index, node) in self.interactive_nodes.iter().enumerate() {
            rows.push(format!(
                "{}|{}|{}|{}|{}|{}",
                index,
                node.window_name,
                node.control_type,
                node.name,
                node.center.to_string(),
                metadata_to_string(&node.metadata)
            ));
        }
        rows.join("\n")
    }

    /// Formats the scrollable elements table; ids continue after the interactive rows.
    pub fn scrollable_elements_to_string(&self) -> String {
        if !self.status {
            return WARNING_MESSAGE.to_string();
        }
        if self.scrollable_nodes.is_empty() {
            return EMPTY_MESSAGE.to_string();
        }

        let mut rows = Vec::with_capacity(self.scrollable_nodes.len() + 1);
        rows.push("# id|window|control_type|name|coords|metadata".to_string());
        let base_index = self.interactive_nodes.len();
        for (index, node) in self.scrollable_nodes.iter().enumerate() {
            rows.push(format!(
                "{}|{}|{}|{}|{}|{}",
                base_index + index,
                node.window_name,
                node.control_type,
                node.name,
                node.center.to_string(),
                metadata_to_string(&node.metadata)
            ));
        }
        rows.join("\n")
    }

    /// Selector map from label index to node, matching `build_selector_map`.
    pub fn build_selector_map(&self) -> SelectorMap {
        let mut entries = Vec::new();
        for node in &self.interactive_nodes {
            entries.push(SelectorNode::Interactive(node.clone()));
        }
        for node in &self.scrollable_nodes {
            entries.push(SelectorNode::Scrollable(node.clone()));
        }
        SelectorMap { entries }
    }
}

/// One entry in the selector map.
#[derive(Debug, Clone)]
pub enum SelectorNode {
    Interactive(TreeElementNode),
    Scrollable(ScrollElementNode),
}

impl SelectorNode {
    pub fn hwnd(&self) -> i64 {
        match self {
            SelectorNode::Interactive(node) => node.hwnd,
            SelectorNode::Scrollable(node) => node.hwnd,
        }
    }

    pub fn control_index(&self) -> Option<usize> {
        match self {
            SelectorNode::Interactive(node) => node.control_index,
            SelectorNode::Scrollable(node) => node.control_index,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            SelectorNode::Interactive(node) => &node.name,
            SelectorNode::Scrollable(node) => &node.name,
        }
    }
}

/// Index-aligned map of label → node, replacing the Python `SelectorMap` dict.
#[derive(Debug, Clone, Default)]
pub struct SelectorMap {
    entries: Vec<SelectorNode>,
}

impl SelectorMap {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn node_of(&self, index: usize) -> Option<&SelectorNode> {
        self.entries.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, &SelectorNode)> {
        self.entries.iter().enumerate()
    }
}

impl std::fmt::Display for SelectorMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SelectorMap({} elements)", self.entries.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str) -> TreeElementNode {
        TreeElementNode {
            name: name.to_string(),
            control_type: "ButtonControl".to_string(),
            bounding_box: BoundingBox::new(10, 20, 110, 60),
            center: Center { x: 60, y: 40 },
            window_name: "Notepad".to_string(),
            hwnd: 42,
            control_index: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn interactive_table_matches_python_format() {
        let state = TreeState {
            status: true,
            interactive_nodes: vec![node("Save"), node("Search")],
            ..Default::default()
        };
        let rendered = state.interactive_elements_to_string();
        let expected = "# id|window|control_type|name|coords|metadata\n0|Notepad|ButtonControl|Save|(60,40)|{}\n1|Notepad|ButtonControl|Search|(60,40)|{}";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn empty_state_message() {
        let state = TreeState::default();
        assert_eq!(state.interactive_elements_to_string(), EMPTY_MESSAGE);
    }

    #[test]
    fn failed_status_message() {
        let state = TreeState { status: false, ..Default::default() };
        assert_eq!(state.interactive_elements_to_string(), WARNING_MESSAGE);
    }

    #[test]
    fn metadata_rendering() {
        let mut metadata = BTreeMap::new();
        metadata.insert("has_focused".to_string(), serde_json::Value::Bool(true));
        metadata.insert(
            "shortcut".to_string(),
            serde_json::Value::String("Ctrl+S".to_string()),
        );
        assert_eq!(
            metadata_to_string(&metadata),
            "{\"has_focused\": true, \"shortcut\": \"Ctrl+S\"}"
        );
    }

    #[test]
    fn bounding_box_center_uses_integer_division() {
        let bbox = BoundingBox::new(0, 0, 5, 5);
        assert_eq!(bbox.get_center(), Center { x: 2, y: 2 });
    }

    #[test]
    fn scrollable_rows_start_after_interactive() {
        let state = TreeState {
            status: true,
            interactive_nodes: vec![node("Save")],
            scrollable_nodes: vec![ScrollElementNode {
                name: "List".to_string(),
                control_type: "PaneControl".to_string(),
                window_name: "Notepad".to_string(),
                bounding_box: BoundingBox::new(0, 0, 10, 10),
                center: Center { x: 5, y: 5 },
                hwnd: 1,
                ..Default::default()
            }],
            ..Default::default()
        };
        let rendered = state.scrollable_elements_to_string();
        assert!(rendered.contains("\n1|Notepad|PaneControl|List|(5,5)|{}"));
    }
}
