//! Tree traversal, mirroring `windows_use/agent/tree/service.py`.

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Duration;

use serde_json::Value;

use crate::tree::config::{
    is_document_control_type, is_informative_control_type, is_interactive_control_type,
    is_interactive_role, DEFAULT_ACTIONS, THREAD_MAX_RETRIES,
};
use crate::tree::views::{
    BoundingBox, Center, ScrollElementNode, TextElementNode, TreeElementNode, TreeState,
};
use crate::uia::patterns::{
    control_types, expand_collapse_state, is_modal_window, range_value_info,
    scroll_pattern_metadata, selection_info, toggle_state_of, ExpandCollapseValue,
    ToggleStateValue,
};
use crate::uia::{accessible_role_name, control_type_name, ElementHandle, UiaError, UiaResult};

/// Focusable set used by the interactive check (adds list items).
fn interactive_focusable(control_type_name: &str) -> bool {
    matches!(
        control_type_name,
        "EditControl"
            | "ButtonControl"
            | "CheckBoxControl"
            | "RadioButtonControl"
            | "TabItemControl"
            | "ListItemControl"
    )
}

/// Accumulators shared across one traversal.
pub struct TraversalContext {
    pub window_bounding_box: crate::uia::BoundingRectangle,
    pub window_name: String,
    pub is_browser: bool,
    pub hwnd: i64,
    pub interactive_nodes: Vec<TreeElementNode>,
    pub scrollable_nodes: Vec<ScrollElementNode>,
    pub dom_interactive_nodes: Vec<TreeElementNode>,
    pub dom_informative_nodes: Vec<TextElementNode>,
    pub dom_bounding_box: Option<crate::uia::BoundingRectangle>,
    dom_seen: bool,
    /// Live controls parallel to the emitted nodes, so tools can act on them.
    pub live_controls: Vec<ElementHandle>,
}

impl TraversalContext {
    pub fn new(
        window_bounding_box: crate::uia::BoundingRectangle,
        window_name: String,
        is_browser: bool,
        hwnd: i64,
    ) -> Self {
        Self {
            window_bounding_box,
            window_name,
            is_browser,
            hwnd,
            interactive_nodes: Vec::new(),
            scrollable_nodes: Vec::new(),
            dom_interactive_nodes: Vec::new(),
            dom_informative_nodes: Vec::new(),
            dom_bounding_box: None,
            dom_seen: false,
            live_controls: Vec::new(),
        }
    }

    fn push_interactive(&mut self, node: TreeElementNode, control: ElementHandle) {
        let index = self.live_controls.len();
        self.live_controls.push(control);
        let mut node = node;
        node.control_index = Some(index);
        self.interactive_nodes.push(node);
    }

    fn push_dom_interactive(&mut self, node: TreeElementNode, control: ElementHandle) {
        let index = self.live_controls.len();
        self.live_controls.push(control);
        let mut node = node;
        node.control_index = Some(index);
        self.dom_interactive_nodes.push(node);
    }

    fn push_scrollable(&mut self, node: ScrollElementNode, control: ElementHandle) {
        let index = self.live_controls.len();
        self.live_controls.push(control);
        let mut node = node;
        node.control_index = Some(index);
        self.scrollable_nodes.push(node);
    }
}

/// Clips an element rectangle to the window and the screen, as `iou_bounding_box` does.
pub fn iou_bounding_box(
    window_box: crate::uia::BoundingRectangle,
    element_box: crate::uia::BoundingRectangle,
    screen_box: crate::uia::BoundingRectangle,
) -> BoundingBox {
    let clipped = element_box.intersect(&window_box).intersect(&screen_box);
    if clipped.right > clipped.left && clipped.bottom > clipped.top {
        BoundingBox {
            left: clipped.left,
            top: clipped.top,
            right: clipped.right,
            bottom: clipped.bottom,
            width: clipped.width(),
            height: clipped.height(),
        }
    } else {
        BoundingBox::default()
    }
}

fn help_text_ascii(text: &str) -> String {
    text.chars().filter(|character| character.is_ascii()).collect()
}

/// `window_name_correction` from the Python traversal.
pub fn window_name_correction(window_name: &str) -> String {
    match window_name {
        "Progman" => "Desktop".to_string(),
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" => "Taskbar".to_string(),
        "Microsoft.UI.Content.PopupWindowSiteBridge" => "Context Menu".to_string(),
        other => other.to_string(),
    }
}

/// A random point inside a scaled-down bounding box (Python's `random_point_within_bounding_box`).
pub fn random_point_within_bounding_box(
    rectangle: crate::uia::BoundingRectangle,
    scale_factor: f64,
) -> (i32, i32) {
    use rand::Rng;
    let scaled_width = (rectangle.width() as f64 * scale_factor) as i32;
    let scaled_height = (rectangle.height() as f64 * scale_factor) as i32;
    let scaled_left = rectangle.left + (rectangle.width() - scaled_width) / 2;
    let scaled_top = rectangle.top + (rectangle.height() - scaled_height) / 2;
    let mut rng = rand::thread_rng();
    (
        rng.gen_range(scaled_left..=scaled_left + scaled_width),
        rng.gen_range(scaled_top..=scaled_top + scaled_height),
    )
}

/// Tree traversal, mirroring the recursive `Tree.tree_traversal` in Python.
#[allow(clippy::too_many_arguments)]
pub fn tree_traversal(
    node: &ElementHandle,
    context: &mut TraversalContext,
    screen_box: crate::uia::BoundingRectangle,
    is_dom: bool,
    is_dialog: bool,
    depth: usize,
) -> UiaResult<()> {
    if depth > 64 {
        return Ok(());
    }

    let cached = node.build_cached().unwrap_or_else(|_| node.clone());
    let snapshot = cached.cached_snapshot();
    let type_name = control_type_name(snapshot.control_type);

    // Scrollable check
    if snapshot.control_type != control_types::EDIT
        && !is_interactive_control_type(type_name)
        && !is_informative_control_type(type_name)
        && !snapshot.is_offscreen
    {
        let scroll = scroll_pattern_metadata(&cached);
        if scroll.vertically_scrollable {
            let rectangle = snapshot.bounding_rectangle;
            let (x, y) = random_point_within_bounding_box(rectangle, 0.8);
            let mut metadata = BTreeMap::new();
            metadata.insert(
                "has_focused".to_string(),
                Value::Bool(snapshot.has_keyboard_focus),
            );
            let name = if !snapshot.name.trim().is_empty() {
                snapshot.name.trim().to_string()
            } else if !snapshot.automation_id.is_empty() {
                snapshot.automation_id.clone()
            } else if !snapshot.localized_control_type.is_empty() {
                capitalize(&snapshot.localized_control_type)
            } else {
                "''".to_string()
            };
            let scrollable = ScrollElementNode {
                name,
                control_type: title_case(&snapshot.localized_control_type),
                window_name: context.window_name.clone(),
                bounding_box: BoundingBox::from_rectangle(&rectangle),
                center: Center { x, y },
                horizontal_scrollable: scroll.horizontally_scrollable,
                horizontal_scroll_percent: round2(if scroll.horizontally_scrollable {
                    scroll.horizontally_percent
                } else {
                    0.0
                }),
                vertical_scrollable: scroll.vertically_scrollable,
                vertical_scroll_percent: round2(if scroll.vertically_scrollable {
                    scroll.vertically_percent
                } else {
                    0.0
                }),
                hwnd: context.hwnd,
                control_index: None,
                metadata,
            };
            context.push_scrollable(scrollable, cached.clone());
        }
    }

    let rectangle = snapshot.bounding_rectangle;
    let area = rectangle.width() * rectangle.height();
    let is_visible = area > 0
        && (!snapshot.is_offscreen
            || type_name == "EditControl"
            || (type_name == "ListItemControl" && context.is_browser))
        && snapshot.is_control_element;

    if is_visible && snapshot.is_enabled {
        let is_keyboard_focusable = if interactive_focusable(type_name) {
            true
        } else {
            snapshot.is_keyboard_focusable
        };

        let mut is_interactive = false;
        if context.is_browser && type_name == "DataItemControl" && !is_keyboard_focusable {
            is_interactive = false;
        } else if !context.is_browser && type_name == "ImageControl" && is_keyboard_focusable {
            is_interactive = true;
        } else if is_interactive_control_type(type_name)
            || is_document_control_type(type_name)
        {
            let role = snapshot
                .legacy_role
                .map(accessible_role_name)
                .unwrap_or("Default");
            let is_role_interactive = is_interactive_role(role);

            let mut is_image = false;
            if type_name == "ImageControl" {
                let localized = &snapshot.localized_control_type;
                if localized == "graphic" || !is_keyboard_focusable {
                    is_image = true;
                }
            }

            if is_role_interactive && (!is_image || is_keyboard_focusable) {
                is_interactive = true;
            }
        } else if type_name == "GroupControl" && context.is_browser {
            let mut has_expand_collapse = false;
            if let Some(state) = expand_collapse_state(&cached) {
                has_expand_collapse = matches!(state, ExpandCollapseValue::Collapsed
                    | ExpandCollapseValue::Expanded
                    | ExpandCollapseValue::PartiallyExpanded
                    | ExpandCollapseValue::LeafNode);
            }
            if has_expand_collapse {
                is_interactive = true;
            }

            let role = snapshot
                .legacy_role
                .map(accessible_role_name)
                .unwrap_or("Default");
            let is_role_interactive = is_interactive_role(role);

            let is_default_action = snapshot
                .legacy_default_action
                .as_deref()
                .map(|action| DEFAULT_ACTIONS.contains(&title_case(action).as_str()))
                .unwrap_or(false);

            if is_role_interactive && (is_default_action || is_keyboard_focusable) {
                is_interactive = true;
            }
        }

        if is_interactive {
            let mut metadata = BTreeMap::new();
            metadata.insert(
                "has_focused".to_string(),
                Value::Bool(snapshot.has_keyboard_focus),
            );
            if !snapshot.accelerator_key.is_empty() {
                metadata.insert(
                    "shortcut".to_string(),
                    Value::String(snapshot.accelerator_key.clone()),
                );
            }
            if !snapshot.help_text.is_empty() {
                metadata.insert(
                    "help_text".to_string(),
                    Value::String(help_text_ascii(&snapshot.help_text)),
                );
            }

            match snapshot.control_type {
                control_types::BUTTON | control_types::CHECKBOX => {
                    if let Some(state) = toggle_state_of(&cached) {
                        let value = match state {
                            ToggleStateValue::On => Some("on"),
                            ToggleStateValue::Off => Some("off"),
                            ToggleStateValue::Indeterminate => None,
                        };
                        if let Some(value) = value {
                            metadata.insert(
                                "toggle_state".to_string(),
                                Value::String(value.to_string()),
                            );
                        }
                    }
                }
                control_types::EDIT => {
                    if let Some(value) = snapshot.legacy_value.as_deref() {
                        let trimmed = value.trim();
                        metadata.insert(
                            "value".to_string(),
                            Value::String(if trimmed.is_empty() {
                                "(empty)".to_string()
                            } else {
                                trimmed.to_string()
                            }),
                        );
                    }
                    if snapshot.is_password {
                        metadata.insert("is_password".to_string(), Value::Bool(true));
                    }
                }
                control_types::COMBOBOX => {
                    if let Some(state) = expand_collapse_state(&cached) {
                        let value = match state {
                            ExpandCollapseValue::Expanded => Some("expanded"),
                            ExpandCollapseValue::Collapsed => Some("collapsed"),
                            ExpandCollapseValue::PartiallyExpanded => Some("partially expanded"),
                            ExpandCollapseValue::LeafNode => None,
                        };
                        if let Some(value) = value {
                            metadata.insert(
                                "expand_collapse_state".to_string(),
                                Value::String(value.to_string()),
                            );
                        }
                    }
                    let selection = selection_info(&cached);
                    if let Some(can_select_multiple) = selection.can_select_multiple {
                        metadata.insert(
                            "is_selection_required".to_string(),
                            Value::Bool(can_select_multiple),
                        );
                    }
                    if let Some(required) = selection.is_selection_required {
                        metadata.insert(
                            "is_selection_required".to_string(),
                            Value::Bool(required),
                        );
                    }
                    if let Some(selected) = selection.is_selected {
                        metadata.insert("is_selected".to_string(), Value::Bool(selected));
                    }
                    if !selection.selected_names.is_empty() {
                        metadata.insert(
                            "selection".to_string(),
                            Value::Array(
                                selection
                                    .selected_names
                                    .iter()
                                    .map(|name| Value::String(name.clone()))
                                    .collect(),
                            ),
                        );
                    }
                }
                control_types::SLIDER => {
                    let range = range_value_info(&cached);
                    if let Some(value) = range.value {
                        metadata.insert("value".to_string(), json_number(round2(value)));
                    }
                    if let Some(minimum) = range.minimum {
                        metadata.insert("min".to_string(), json_number(round2(minimum)));
                    }
                    if let Some(maximum) = range.maximum {
                        metadata.insert("max".to_string(), json_number(round2(maximum)));
                    }
                }
                _ => {}
            }

            let name = snapshot.name.trim().to_string();
            if context.is_browser && is_dom {
                let bounding_box = match context.dom_bounding_box {
                    Some(dom_box) => iou_bounding_box(dom_box, rectangle, screen_box),
                    None => iou_bounding_box(context.window_bounding_box, rectangle, screen_box),
                };
                let center = bounding_box.get_center();
                let node = TreeElementNode {
                    name,
                    control_type: title_case(&snapshot.localized_control_type),
                    bounding_box,
                    center,
                    window_name: context.window_name.clone(),
                    hwnd: context.hwnd,
                    control_index: None,
                    metadata,
                };
                context.push_dom_interactive(node, cached.clone());
            } else {
                let bounding_box = iou_bounding_box(context.window_bounding_box, rectangle, screen_box);
                let center = bounding_box.get_center();
                let node = TreeElementNode {
                    name,
                    control_type: title_case(&snapshot.localized_control_type),
                    bounding_box,
                    center,
                    window_name: context.window_name.clone(),
                    hwnd: context.hwnd,
                    control_index: None,
                    metadata,
                };
                context.push_interactive(node, cached.clone());
            }
        }

        // Informative check
        if is_informative_control_type(type_name) {
            // Images are excluded from the informative text list; the Python port
            // branches twice on the same outcome, so a single check is enough.
            let is_image_check = type_name == "ImageControl";
            if !is_image_check && context.is_browser && is_dom {
                context.dom_informative_nodes.push(TextElementNode {
                    text: snapshot.name.trim().to_string(),
                });
            }
        }
    }

    let children = cached.children()?;
    let iter: Box<dyn Iterator<Item = &ElementHandle>> = if is_dom {
        Box::new(children.iter())
    } else {
        Box::new(children.iter().rev())
    };

    for child in iter {
        let child_snapshot = child.cached_snapshot();
        if context.is_browser && child_snapshot.automation_id == "RootWebArea" {
            let rectangle = child_snapshot.bounding_rectangle;
            context.dom_bounding_box = Some(rectangle);
            context.dom_seen = true;
            tree_traversal(child, context, screen_box, true, is_dialog, depth + 1)?;
        } else if child_snapshot.control_type == control_types::WINDOW {
            if !child_snapshot.is_offscreen {
                if is_dom {
                    if let Some(dom_box) = context.dom_bounding_box {
                        if child_snapshot.bounding_rectangle.width() as f64
                            > 0.8 * dom_box.width() as f64
                        {
                            context.dom_interactive_nodes.clear();
                        }
                    }
                } else if is_modal_window(child) {
                    context.interactive_nodes.clear();
                }
            }
            tree_traversal(child, context, screen_box, is_dom, true, depth + 1)?;
        } else {
            tree_traversal(child, context, screen_box, is_dom, is_dialog, depth + 1)?;
        }
    }

    Ok(())
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn json_number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < i64::MAX as f64 {
        Value::Number(serde_json::Number::from(value as i64))
    } else {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// Python's `str.title()` for ASCII words.
pub fn title_case(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut boundary = true;
    for character in text.chars() {
        if character.is_alphanumeric() {
            if boundary {
                for upper in character.to_uppercase() {
                    result.push(upper);
                }
            } else {
                for lower in character.to_lowercase() {
                    result.push(lower);
                }
            }
            boundary = false;
        } else {
            result.push(character);
            boundary = true;
        }
    }
    result
}

/// Python's `str.capitalize()`.
pub fn capitalize(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => {
            let mut result: String = first.to_uppercase().collect();
            result.push_str(&characters.as_str().to_lowercase());
            result
        }
        None => String::new(),
    }
}

/// Captures the tree for one window, retrying like the Python `get_nodes`.
pub fn get_nodes(
    automation: &crate::uia::UiAutomation,
    handle: i64,
    is_browser: bool,
    screen_box: crate::uia::BoundingRectangle,
    wait_time: f64,
) -> UiaResult<(Vec<TreeElementNode>, Vec<ScrollElementNode>, Vec<TextElementNode>, Vec<ElementHandle>)>
{
    if wait_time > 0.0 {
        sleep(Duration::from_secs_f64(wait_time));
    }

    let node = automation.element_from_handle(handle)?;
    let window_bounding_box = node.bounding_rectangle()?;
    let mut window_name = node.name().trim().to_string();
    window_name = window_name_correction(&window_name);

    let mut context = TraversalContext::new(window_bounding_box, window_name, is_browser, handle);
    tree_traversal(&node, &mut context, screen_box, false, false, 0)?;

    let mut interactive = context.interactive_nodes;
    interactive.extend(context.dom_interactive_nodes);
    Ok((
        interactive,
        context.scrollable_nodes,
        context.dom_informative_nodes,
        context.live_controls,
    ))
}

/// Captures the tree state for a set of windows.
pub struct TreeCapture {
    pub state: TreeState,
    pub live_controls: Vec<ElementHandle>,
    pub failed_handles: Vec<i64>,
}

pub struct TreeOptions {
    pub use_accessibility: bool,
}

/// Captures the whole desktop tree, mirroring `Tree.get_state`.
pub fn capture_tree_state(
    automation: &crate::uia::UiAutomation,
    active_window_handle: Option<i64>,
    other_window_handles: &[i64],
    screen_box: crate::uia::BoundingRectangle,
    is_browser: impl Fn(i64) -> bool,
) -> TreeCapture {
    let mut handles: Vec<i64> = Vec::new();
    if let Some(active) = active_window_handle {
        handles.push(active);
    }
    handles.extend_from_slice(other_window_handles);

    let mut interactive_nodes = Vec::new();
    let mut scrollable_nodes = Vec::new();
    let mut dom_informative_nodes = Vec::new();
    let mut live_controls = Vec::new();
    let mut failed_handles = Vec::new();
    let mut dom_node = None;

    let mut task_inputs: Vec<(i64, bool)> = Vec::with_capacity(handles.len());
    for handle in &handles {
        let mut is_browser_flag = false;
        if let Ok(node) = automation.element_from_handle(*handle) {
            if active_window_handle == Some(*handle) && node.class_name() == "Progman" {
                continue;
            }
            is_browser_flag = is_browser(*handle);
        }
        task_inputs.push((*handle, is_browser_flag));
    }

    for (handle, is_browser_flag) in task_inputs {
        let mut attempt = 0usize;
        loop {
            let wait_time = if attempt > 0 {
                0.5 * 2f64.powi(attempt as i32 - 1)
            } else {
                0.0
            };
            match get_nodes(automation, handle, is_browser_flag, screen_box, wait_time) {
                Ok((mut elements, mut scrolls, mut infos, mut controls)) => {
                    // Offsets keep control indices valid after merging windows.
                    let base = live_controls.len();
                    if base > 0 {
                        for node in elements.iter_mut() {
                            if let Some(index) = node.control_index.as_mut() {
                                *index += base;
                            }
                        }
                        for node in scrolls.iter_mut() {
                            if let Some(index) = node.control_index.as_mut() {
                                *index += base;
                            }
                        }
                    }
                    interactive_nodes.append(&mut elements);
                    scrollable_nodes.append(&mut scrolls);
                    dom_informative_nodes.append(&mut infos);
                    live_controls.append(&mut controls);
                    break;
                }
                Err(error) if error.dead_element => break,
                Err(_) if attempt < THREAD_MAX_RETRIES => {
                    attempt += 1;
                    continue;
                }
                Err(_) => {
                    failed_handles.push(handle);
                    break;
                }
            }
        }
    }

    // The DOM scroll node comes from the last window that entered a DOM subtree.
    if let Ok(root) = automation.root() {
        let _ = root;
    }
    let root_node = TreeElementNode {
        name: "Desktop".to_string(),
        control_type: "PaneControl".to_string(),
        bounding_box: BoundingBox::from_rectangle(&screen_box),
        center: BoundingBox::from_rectangle(&screen_box).get_center(),
        window_name: "Desktop".to_string(),
        hwnd: 0,
        control_index: None,
        metadata: BTreeMap::new(),
    };

    let status = failed_handles.is_empty();
    if !status {
        eprintln!(
            "[Tree] {} window(s) failed to capture — UI services may be loading",
            failed_handles.len()
        );
    }

    TreeCapture {
        state: TreeState {
            status,
            root_node: Some(root_node),
            dom_node: dom_node.take(),
            interactive_nodes,
            scrollable_nodes,
            dom_informative_nodes,
        },
        live_controls,
        failed_handles,
    }
}

/// True when the error indicates a dead element.
pub fn is_dead(error: &UiaError) -> bool {
    error.dead_element
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_case_matches_python() {
        assert_eq!(title_case("edit"), "Edit");
        assert_eq!(title_case("list item"), "List Item");
        assert_eq!(title_case("TEXT"), "Text");
        assert_eq!(title_case(""), "");
    }

    #[test]
    fn capitalize_matches_python() {
        assert_eq!(capitalize("button"), "Button");
        assert_eq!(capitalize("BUTTON"), "Button");
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn correction_matches_python() {
        assert_eq!(window_name_correction("Progman"), "Desktop");
        assert_eq!(window_name_correction("Shell_TrayWnd"), "Taskbar");
        assert_eq!(
            window_name_correction("Microsoft.UI.Content.PopupWindowSiteBridge"),
            "Context Menu"
        );
        assert_eq!(window_name_correction("Notepad"), "Notepad");
    }

    #[test]
    fn clipping_zeroes_non_overlapping_boxes() {
        let window = crate::uia::BoundingRectangle { left: 0, top: 0, right: 100, bottom: 100 };
        let element = crate::uia::BoundingRectangle { left: 200, top: 200, right: 300, bottom: 300 };
        let screen = crate::uia::BoundingRectangle { left: 0, top: 0, right: 1920, bottom: 1080 };
        let result = iou_bounding_box(window, element, screen);
        assert_eq!(result, BoundingBox::default());
    }

    #[test]
    fn clipping_intersects_boxes() {
        let window = crate::uia::BoundingRectangle { left: 0, top: 0, right: 100, bottom: 100 };
        let element = crate::uia::BoundingRectangle { left: 50, top: 50, right: 150, bottom: 150 };
        let screen = crate::uia::BoundingRectangle { left: 0, top: 0, right: 1920, bottom: 1080 };
        let result = iou_bounding_box(window, element, screen);
        assert_eq!((result.left, result.top, result.right, result.bottom), (50, 50, 100, 100));
        assert_eq!((result.width, result.height), (50, 50));
    }

    #[test]
    fn rounding_keeps_two_decimals() {
        assert_eq!(round2(12.3456), 12.35);
        assert_eq!(round2(100.0), 100.0);
    }
}
