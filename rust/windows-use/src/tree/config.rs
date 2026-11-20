//! Tree configuration tables, mirroring `windows_use/agent/tree/config.py`.

/// Control type names considered interactive.
pub const INTERACTIVE_CONTROL_TYPE_NAMES: [&str; 18] = [
    "ButtonControl",
    "ListItemControl",
    "MenuItemControl",
    "EditControl",
    "CheckBoxControl",
    "RadioButtonControl",
    "DocumentControl",
    "ComboBoxControl",
    "HyperlinkControl",
    "SplitButtonControl",
    "TabItemControl",
    "TreeItemControl",
    "DataItemControl",
    "HeaderItemControl",
    "TextBoxControl",
    "SpinnerControl",
    "ScrollBarControl",
    "SliderControl",
];

/// Legacy accessible roles considered interactive.
pub const INTERACTIVE_ROLES: [&str; 27] = [
    "PushButton",
    "SplitButton",
    "ButtonDropDown",
    "ButtonMenu",
    "ButtonDropDownGrid",
    "OutlineButton",
    "Link",
    "Text",
    "IpAddress",
    "HotkeyField",
    "ComboBox",
    "DropList",
    "CheckButton",
    "RadioButton",
    "MenuItem",
    "ListItem",
    "PageTab",
    "OutlineItem",
    "Slider",
    "SpinButton",
    "Dial",
    "ScrollBar",
    "Grip",
    "ColumnHeader",
    "RowHeader",
    "Cell",
    "Document",
];

/// Document control type names.
pub const DOCUMENT_CONTROL_TYPE_NAMES: [&str; 1] = ["DocumentControl"];

/// Structural control type names.
pub const STRUCTURAL_CONTROL_TYPE_NAMES: [&str; 3] = ["PaneControl", "GroupControl", "CustomControl"];

/// Informative control type names.
pub const INFORMATIVE_CONTROL_TYPE_NAMES: [&str; 3] =
    ["TextControl", "ImageControl", "StatusBarControl"];

/// Legacy default action titles treated as interactive.
pub const DEFAULT_ACTIONS: [&str; 8] = [
    "Click",
    "Press",
    "Jump",
    "Check",
    "Uncheck",
    "Double Click",
    "Expand",
    "Collapse",
];

/// Maximum retries per window during capture.
pub const THREAD_MAX_RETRIES: usize = 3;

pub fn is_interactive_control_type(name: &str) -> bool {
    INTERACTIVE_CONTROL_TYPE_NAMES.contains(&name)
}

pub fn is_informative_control_type(name: &str) -> bool {
    INFORMATIVE_CONTROL_TYPE_NAMES.contains(&name)
}

pub fn is_document_control_type(name: &str) -> bool {
    DOCUMENT_CONTROL_TYPE_NAMES.contains(&name)
}

pub fn is_interactive_role(role: &str) -> bool {
    INTERACTIVE_ROLES.contains(&role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_python_membership() {
        assert!(is_interactive_control_type("ButtonControl"));
        assert!(is_interactive_control_type("EditControl"));
        assert!(!is_interactive_control_type("TextControl"));
        assert!(is_informative_control_type("TextControl"));
        assert!(is_interactive_role("PushButton"));
        assert!(is_interactive_role("Cell"));
        assert!(!is_interactive_role("StaticText"));
        assert!(DEFAULT_ACTIONS.contains(&"Double Click"));
    }
}
