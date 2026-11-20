//! Desktop state models, mirroring `windows_use/agent/desktop/views.py`.

use serde::{Deserialize, Serialize};

use crate::tree::views::{BoundingBox, TreeState};

/// Creates the `tabulate(tablefmt="simple")` rendering the Python port produces.
pub fn tabulate_simple(headers: &[&str], rows: &[Vec<String>]) -> String {
    let columns = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|header| header.chars().count()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate().take(columns) {
            let length = cell.chars().count();
            if length > widths[index] {
                widths[index] = length;
            }
        }
    }

    // tabulate right-aligns numeric columns and reserves 2 characters of padding for them.
    let numeric: Vec<bool> = (0..columns)
        .map(|index| {
            !rows.is_empty()
                && rows.iter().all(|row| {
                    row.get(index)
                        .map(|cell| is_numeric(cell))
                        .unwrap_or(false)
                })
        })
        .collect();

    for index in 0..columns {
        if numeric[index] {
            widths[index] += 2;
        }
    }

    let render_row = |cells: &[String]| -> String {
        let mut line = String::new();
        for index in 0..columns {
            let cell = cells.get(index).map(|value| value.as_str()).unwrap_or("");
            let width = widths[index];
            let padded = if numeric[index] {
                format!("{cell:>width$}", width = width)
            } else {
                format!("{cell:<width$}", width = width)
            };
            if index > 0 {
                line.push_str("  ");
            }
            line.push_str(&padded);
        }
        line.trim_end().to_string()
    };

    let header_line = render_row(&headers.iter().map(|h| h.to_string()).collect::<Vec<_>>());
    let separator = (0..columns)
        .map(|index| "-".repeat(widths[index]))
        .collect::<Vec<_>>()
        .join("  ");

    let mut lines = vec![header_line, separator];
    for row in rows {
        lines.push(render_row(row));
    }
    lines.join("\n")
}

fn is_numeric(cell: &str) -> bool {
    !cell.is_empty() && cell.parse::<f64>().is_ok()
}

/// Browser identifiers supported by `Browser`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Browser {
    Chrome,
    Edge,
    Firefox,
}

impl Browser {
    pub fn value(&self) -> &'static str {
        match self {
            Browser::Chrome => "chrome",
            Browser::Edge => "msedge",
            Browser::Firefox => "firefox",
        }
    }

    /// True when the process name belongs to a supported browser.
    pub fn has_process(process_name: &str) -> bool {
        matches!(
            process_name.to_lowercase().as_str(),
            "chrome.exe" | "msedge.exe" | "firefox.exe"
        )
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "chrome" => Some(Browser::Chrome),
            "edge" | "msedge" => Some(Browser::Edge),
            "firefox" => Some(Browser::Firefox),
            _ => None,
        }
    }
}

impl Default for Browser {
    fn default() -> Self {
        Browser::Edge
    }
}

/// Window state matching the Python `Status` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Maximized,
    Minimized,
    Normal,
    Hidden,
}

impl Status {
    pub fn value(&self) -> &'static str {
        match self {
            Status::Maximized => "Maximized",
            Status::Minimized => "Minimized",
            Status::Normal => "Normal",
            Status::Hidden => "Hidden",
        }
    }
}

/// One top-level window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub name: String,
    pub is_browser: bool,
    pub depth: usize,
    pub status: Status,
    pub bounding_box: BoundingBox,
    pub handle: i64,
    pub process_id: i32,
}

impl Window {
    pub fn to_row(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.depth.to_string(),
            self.status.value().to_string(),
            self.bounding_box.width.to_string(),
            self.bounding_box.height.to_string(),
            self.handle.to_string(),
        ]
    }
}

/// A width/height pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

impl Size {
    pub fn to_string(&self) -> String {
        format!("({},{})", self.width, self.height)
    }
}

/// One virtual desktop.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtualDesktop {
    pub id: String,
    pub name: String,
}

/// The captured desktop state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DesktopState {
    pub active_desktop: VirtualDesktop,
    pub all_desktops: Vec<VirtualDesktop>,
    pub windows: Vec<Window>,
    pub active_window: Option<Window>,
    /// PNG bytes when vision is enabled.
    pub screenshot: Option<Vec<u8>>,
    pub tree_state: Option<TreeState>,
}

impl DesktopState {
    pub fn active_desktop_to_string(&self) -> String {
        tabulate_simple(&["Name"], &[vec![self.active_desktop.name.clone()]])
    }

    pub fn desktops_to_string(&self) -> String {
        let rows: Vec<Vec<String>> = self
            .all_desktops
            .iter()
            .map(|desktop| vec![desktop.name.clone()])
            .collect();
        tabulate_simple(&["Name"], &rows)
    }

    pub fn active_window_to_string(&self) -> String {
        match &self.active_window {
            Some(window) => tabulate_simple(
                &["Name", "Depth", "Status", "Width", "Height", "Handle"],
                &[window.to_row()],
            ),
            None => "No active window found".to_string(),
        }
    }

    pub fn windows_to_string(&self) -> String {
        if self.windows.is_empty() {
            return "No windows found".to_string();
        }
        let rows: Vec<Vec<String>> = self.windows.iter().map(|window| window.to_row()).collect();
        tabulate_simple(
            &["Name", "Depth", "Status", "Width", "Height", "Handle"],
            &rows,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_table_matches_tabulate_simple() {
        let state = DesktopState {
            windows: vec![
                Window {
                    name: "Notepad".to_string(),
                    is_browser: false,
                    depth: 0,
                    status: Status::Normal,
                    bounding_box: BoundingBox::new(0, 0, 800, 600),
                    handle: 123456,
                    process_id: 1,
                },
                Window {
                    name: "Google Chrome".to_string(),
                    is_browser: true,
                    depth: 1,
                    status: Status::Minimized,
                    bounding_box: BoundingBox::new(0, 0, 1920, 1080),
                    handle: 999,
                    process_id: 2,
                },
            ],
            ..Default::default()
        };
        let expected = "Name             Depth  Status       Width    Height    Handle\n-------------  -------  ---------  -------  --------  --------\nNotepad              0  Normal         800       600    123456\nGoogle Chrome        1  Minimized     1920      1080       999";
        assert_eq!(state.windows_to_string(), expected);
    }

    #[test]
    fn single_desktop_table() {
        let state = DesktopState {
            active_desktop: VirtualDesktop {
                id: "0".to_string(),
                name: "Desktop 1".to_string(),
            },
            ..Default::default()
        };
        assert_eq!(
            state.active_desktop_to_string(),
            "Name\n---------\nDesktop 1"
        );
    }

    #[test]
    fn missing_active_window() {
        let state = DesktopState::default();
        assert_eq!(state.active_window_to_string(), "No active window found");
        assert_eq!(state.windows_to_string(), "No windows found");
    }

    #[test]
    fn browser_process_matching() {
        assert!(Browser::has_process("chrome.exe"));
        assert!(Browser::has_process("MSEDGE.EXE"));
        assert!(!Browser::has_process("notepad.exe"));
    }
}
