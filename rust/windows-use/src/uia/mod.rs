//! UI Automation (UIA3) access through native COM interfaces.

pub mod control;
pub mod elements;
pub mod input;
pub mod patterns;

pub use control::{
    escape_text_for_sendkeys, parse_send_keys, parse_shortcut, send_keys, send_shortcut,
    type_literal, SendKeysAction,
};
pub use elements::{
    automation_client, ensure_com_initialized, BoundingRectangle, CachedElement, ElementHandle,
    UiAutomation, UiaError, UiaResult,
};
pub use input::{
    click_at, double_click_at, drag_to, get_cursor_pos, middle_click_at, move_to,
    press_key, release_key, right_click_at, scroll_horizontal, scroll_vertical,
    set_cursor_pos, show_window, virtual_screen_rect, virtual_screen_size, wheel_down,
    wheel_left, wheel_right, wheel_up, MouseButton,
};
pub use patterns::{
    scroll_pattern_metadata, toggle_state_of, CachedScrollInfo, ToggleStateValue,
};

/// Control type name for a UIA control type id (matches the Python port's table).
pub fn control_type_name(id: i32) -> &'static str {
    crate::uia::control_type_names::control_type_name(id)
}

/// Accessible role name for a legacy accessible role id.
pub fn accessible_role_name(id: i32) -> &'static str {
    crate::uia::role_names::accessible_role_name(id)
}

#[path = "control_type_names.rs"]
mod control_type_names;
#[path = "role_names.rs"]
mod role_names;
