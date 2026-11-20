//! UIA pattern accessors used by tree traversal and tool metadata.

use windows::core::VARIANT;
use windows::Win32::UI::Accessibility::{
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_ScrollHorizontalScrollPercentPropertyId,
    UIA_ScrollHorizontallyScrollablePropertyId, UIA_ScrollPatternId,
    UIA_ScrollVerticalScrollPercentPropertyId, UIA_ScrollVerticallyScrollablePropertyId,
    UIA_SelectionCanSelectMultiplePropertyId, UIA_SelectionIsSelectionRequiredPropertyId,
    UIA_SelectionItemIsSelectedPropertyId, UIA_SelectionSelectionPropertyId,
    UIA_ToggleToggleStatePropertyId,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomationScrollPattern, IUIAutomationSelectionPattern, IUIAutomationTogglePattern,
    UIA_RangeValueMaximumPropertyId, UIA_RangeValueMinimumPropertyId, UIA_RangeValueValuePropertyId,
};

use crate::uia::elements::{CachedElement, ElementHandle};

/// Scroll state read from an element's cached scroll pattern.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CachedScrollInfo {
    pub horizontally_scrollable: bool,
    pub horizontally_percent: f64,
    pub vertically_scrollable: bool,
    pub vertically_percent: f64,
}

/// Toggle state values, matching the Python `ToggleState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleStateValue {
    Off,
    On,
    Indeterminate,
}

/// Expand/collapse states, matching the Python `ExpandCollapseState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpandCollapseValue {
    Collapsed,
    Expanded,
    PartiallyExpanded,
    LeafNode,
}

/// Reads the cached scroll pattern; falls back to cached scroll properties.
pub fn scroll_pattern_metadata(element: &ElementHandle) -> CachedScrollInfo {
    let mut info = CachedScrollInfo::default();

    if let Ok(scroll) = unsafe { element.element.GetCachedPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId) } {
        unsafe {
            let horizontally = scroll.CurrentHorizontallyScrollable().unwrap_or_default();
            let vertically = scroll.CurrentVerticallyScrollable().unwrap_or_default();
            info.horizontally_scrollable = horizontally.as_bool();
            info.vertically_scrollable = vertically.as_bool();
            if info.horizontally_scrollable {
                info.horizontally_percent =
                    scroll.CurrentHorizontalScrollPercent().unwrap_or_default();
            }
            if info.vertically_scrollable {
                info.vertically_percent = scroll.CurrentVerticalScrollPercent().unwrap_or_default();
            }
        }
        return info;
    }

    if let Ok(value) = unsafe {
        element
            .element
            .GetCachedPropertyValue(UIA_ScrollHorizontallyScrollablePropertyId)
    } {
        info.horizontally_scrollable = variant_bool(&value);
    }
    if let Ok(value) = unsafe {
        element
            .element
            .GetCachedPropertyValue(UIA_ScrollVerticallyScrollablePropertyId)
    } {
        info.vertically_scrollable = variant_bool(&value);
    }
    if info.horizontally_scrollable {
        if let Ok(value) = unsafe {
            element
                .element
                .GetCachedPropertyValue(UIA_ScrollHorizontalScrollPercentPropertyId)
        } {
            info.horizontally_percent = variant_f64(&value);
        }
    }
    if info.vertically_scrollable {
        if let Ok(value) = unsafe {
            element
                .element
                .GetCachedPropertyValue(UIA_ScrollVerticalScrollPercentPropertyId)
        } {
            info.vertically_percent = variant_f64(&value);
        }
    }
    info
}

/// Reads the cached toggle state of a button/checkbox.
pub fn toggle_state_of(element: &ElementHandle) -> Option<ToggleStateValue> {
    if let Ok(toggle) = unsafe {
        element
            .element
            .GetCachedPatternAs::<IUIAutomationTogglePattern>(windows::Win32::UI::Accessibility::UIA_TogglePatternId)
    } {
        if let Ok(state) = unsafe { toggle.CurrentToggleState() } {
            return match state.0 {
                1 => Some(ToggleStateValue::On),
                0 => Some(ToggleStateValue::Off),
                _ => Some(ToggleStateValue::Indeterminate),
            };
        }
    }

    let value = unsafe {
        element
            .element
            .GetCachedPropertyValue(UIA_ToggleToggleStatePropertyId)
    }
    .ok()?;
    match raw_i32(&value) {
        Some(1) => Some(ToggleStateValue::On),
        Some(0) => Some(ToggleStateValue::Off),
        Some(_) => Some(ToggleStateValue::Indeterminate),
        None => None,
    }
}

/// Reads the cached expand/collapse state.
pub fn expand_collapse_state(element: &ElementHandle) -> Option<ExpandCollapseValue> {
    let value = unsafe {
        element
            .element
            .GetCachedPropertyValue(UIA_ExpandCollapseExpandCollapseStatePropertyId)
    }
    .ok()?;
    match raw_i32(&value) {
        Some(0) => Some(ExpandCollapseValue::Collapsed),
        Some(1) => Some(ExpandCollapseValue::Expanded),
        Some(2) => Some(ExpandCollapseValue::PartiallyExpanded),
        Some(3) => Some(ExpandCollapseValue::LeafNode),
        _ => None,
    }
}

/// Reads cached selection metadata for combo boxes.
#[derive(Debug, Clone, Default)]
pub struct SelectionInfo {
    pub can_select_multiple: Option<bool>,
    pub is_selection_required: Option<bool>,
    pub is_selected: Option<bool>,
    pub selected_names: Vec<String>,
}

pub fn selection_info(element: &ElementHandle) -> SelectionInfo {
    let mut info = SelectionInfo::default();

    if let Ok(value) = unsafe {
        element
            .element
            .GetCachedPropertyValue(UIA_SelectionCanSelectMultiplePropertyId)
    } {
        if let Some(boolean) = raw_bool(&value) {
            info.can_select_multiple = Some(boolean);
        }
    }
    if let Ok(value) = unsafe {
        element
            .element
            .GetCachedPropertyValue(UIA_SelectionIsSelectionRequiredPropertyId)
    } {
        if let Some(boolean) = raw_bool(&value) {
            info.is_selection_required = Some(boolean);
        }
    }
    if let Ok(value) = unsafe {
        element
            .element
            .GetCachedPropertyValue(UIA_SelectionItemIsSelectedPropertyId)
    } {
        if let Some(boolean) = raw_bool(&value) {
            info.is_selected = Some(boolean);
        }
    }

    if let Ok(selection) = unsafe {
        element.element.GetCachedPatternAs::<IUIAutomationSelectionPattern>(
            windows::Win32::UI::Accessibility::UIA_SelectionPatternId,
        )
    } {
        if let Ok(array) = unsafe { selection.GetCachedSelection() } {
            if let Ok(length) = unsafe { array.Length() } {
                for index in 0..length {
                    if let Ok(selected) = unsafe { array.GetElement(index) } {
                        let handle = ElementHandle { element: selected };
                        let snapshot = handle.cached_snapshot();
                        if !snapshot.name.is_empty() {
                            info.selected_names.push(snapshot.name);
                        }
                    }
                }
            }
        }
    }

    let _ = UIA_SelectionSelectionPropertyId;
    info
}

/// Range value metadata for sliders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RangeValueInfo {
    pub value: Option<f64>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
}

pub fn range_value_info(element: &ElementHandle) -> RangeValueInfo {
    let mut info = RangeValueInfo::default();
    if let Ok(value) = unsafe { element.element.GetCachedPropertyValue(UIA_RangeValueValuePropertyId) }
    {
        info.value = raw_f64(&value);
    }
    if let Ok(value) =
        unsafe { element.element.GetCachedPropertyValue(UIA_RangeValueMinimumPropertyId) }
    {
        info.minimum = raw_f64(&value);
    }
    if let Ok(value) =
        unsafe { element.element.GetCachedPropertyValue(UIA_RangeValueMaximumPropertyId) }
    {
        info.maximum = raw_f64(&value);
    }
    info
}

/// True when the element reports a modal window.
pub fn is_modal_window(element: &ElementHandle) -> bool {
    unsafe {
        element
            .element
            .GetCachedPropertyValue(windows::Win32::UI::Accessibility::UIA_WindowIsModalPropertyId)
    }
    .map(|value| raw_bool(&value).unwrap_or(false))
    .unwrap_or(false)
}

/// Returns whether a control type id belongs to the interactive set.
pub fn is_interactive_type(control_type: i32) -> bool {
    matches!(
        control_type,
        50000 // Button
            | 50002 // CheckBox
            | 50003 // ComboBox
            | 50004 // Edit
            | 50005 // Hyperlink
            | 50007 // ListItem
            | 50008 // MenuItem
            | 50010 // RadioButton
            | 50011 // ScrollBar
            | 50012 // Slider
            | 50013 // Spinner
            | 50019 // TabItem
            | 50023 // TreeItem
            | 50024 // DataItem
            | 50025 // Document
            | 50034 // TextControl? (kept for parity with table)
            | 50029 // HeaderItem
            | 50033 // Pane? see below
    )
}

/// Control-type ids grouped the way the Python tree module groups them.
pub mod control_types {
    pub const BUTTON: i32 = 50000;
    pub const CALENDAR: i32 = 50001;
    pub const CHECKBOX: i32 = 50002;
    pub const COMBOBOX: i32 = 50003;
    pub const EDIT: i32 = 50004;
    pub const HYPERLINK: i32 = 50005;
    pub const IMAGE: i32 = 50006;
    pub const LISTITEM: i32 = 50007;
    pub const MENUITEM: i32 = 50008;
    pub const PROGRESSBAR: i32 = 50009;
    pub const RADIOBUTTON: i32 = 50010;
    pub const SCROLLBAR: i32 = 50011;
    pub const SLIDER: i32 = 50012;
    pub const SPINNER: i32 = 50013;
    pub const STATUSBAR: i32 = 50014;
    pub const TAB: i32 = 50018;
    pub const TABITEM: i32 = 50019;
    pub const TEXT: i32 = 50020;
    pub const TOOLBAR: i32 = 50021;
    pub const TOOLTIP: i32 = 50022;
    pub const TREE: i32 = 50023;
    pub const TREEITEM: i32 = 50024;
    pub const CUSTOM: i32 = 50025;
    pub const GROUP: i32 = 50026;
    pub const THUMB: i32 = 50027;
    pub const DATAGRID: i32 = 50028;
    pub const DATAITEM: i32 = 50029;
    pub const DOCUMENT: i32 = 50030;
    pub const SPLITBUTTON: i32 = 50031;
    pub const WINDOW: i32 = 50032;
    pub const PANE: i32 = 50033;
    pub const HEADER: i32 = 50034;
    pub const HEADERITEM: i32 = 50035;
    pub const TABLE: i32 = 50036;
    pub const TITLEBAR: i32 = 50037;
    pub const SEPARATOR: i32 = 50038;
}

fn variant_bool(value: &VARIANT) -> bool {
    raw_bool(value).unwrap_or(false)
}

fn variant_f64(value: &VARIANT) -> f64 {
    raw_f64(value).unwrap_or(0.0)
}

fn raw_bool(value: &VARIANT) -> Option<bool> {
    <&VARIANT as TryInto<bool>>::try_into(value).ok()
}

fn raw_i32(value: &VARIANT) -> Option<i32> {
    <&VARIANT as TryInto<i32>>::try_into(value).ok()
}

fn raw_f64(value: &VARIANT) -> Option<f64> {
    <&VARIANT as TryInto<f64>>::try_into(value).ok()
}

/// Applies the tree module's `iou_bounding_box` clipping rule.
pub fn intersect_boxes(
    window: crate::uia::elements::BoundingRectangle,
    element: crate::uia::elements::BoundingRectangle,
    screen: crate::uia::elements::BoundingRectangle,
) -> crate::uia::elements::BoundingRectangle {
    let clipped = element.intersect(&window).intersect(&screen);
    if clipped.right > clipped.left && clipped.bottom > clipped.top {
        clipped
    } else {
        crate::uia::elements::BoundingRectangle::default()
    }
}

/// Snapshot metadata helper for callers that need process/window identity.
pub fn identity(snapshot: &CachedElement) -> (i32, i64) {
    (snapshot.process_id, snapshot.native_window_handle)
}

/// Kept for API parity with the Python module's cached pattern accessor.
pub fn has_scroll_pattern(element: &ElementHandle) -> bool {
    unsafe {
        element
            .element
            .GetCachedPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId)
    }
    .is_ok()
}

/// Re-exported pattern ids for traversal callers.
pub use windows::Win32::UI::Accessibility::{
    UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId, UIA_TextPatternId, UIA_ValuePatternId,
};
