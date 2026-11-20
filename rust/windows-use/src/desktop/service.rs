//! Desktop service, mirroring `windows_use/agent/desktop/service.py`.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::desktop::views::{Browser, DesktopState, Size, Status, VirtualDesktop, Window};
use crate::tree::config::is_interactive_control_type;
use crate::tree::service::{capture_tree_state, title_case};
use crate::tree::views::BoundingBox;
use crate::uia::{self, ElementHandle, UiAutomation};

/// Key name aliases mirroring `desktop/config.py`.
pub fn key_alias(key: &str) -> String {
    match key.to_lowercase().as_str() {
        "backspace" => "Back".to_string(),
        "capslock" => "Capital".to_string(),
        "scrolllock" => "Scroll".to_string(),
        "windows" => "Win".to_string(),
        "command" => "Win".to_string(),
        "option" => "Alt".to_string(),
        other => other.to_string(),
    }
}

/// Applications that are never listed or targeted.
pub const AVOIDED_APPS: [&str; 1] = ["AgentUI"];

/// Window classes excluded from the window list.
pub const EXCLUDED_APPS: [&str; 5] = [
    "Progman",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Microsoft.UI.Content.PopupWindowSiteBridge",
    "Windows.UI.Core.CoreWindow",
];

/// One launch-mode request.
#[derive(Debug, Clone)]
pub struct AppRequest {
    pub mode: AppMode,
    pub name: Option<String>,
    pub loc: Option<(i32, i32)>,
    pub size: Option<(i32, i32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Launch,
    Resize,
    Switch,
}

/// Desktop automation entry point.
pub struct Desktop {
    pub use_vision: bool,
    pub use_annotation: bool,
    pub use_accessibility: bool,
    pub desktop_state: Option<DesktopState>,
    pub live_controls: Vec<ElementHandle>,
    automation: UiAutomation,
    cached_windows_version: Option<String>,
    cached_default_language: Option<String>,
    cached_user_account_type: Option<String>,
}

impl Desktop {
    pub fn new(use_vision: bool, use_annotation: bool, use_accessibility: bool) -> anyhow::Result<Self> {
        let automation = UiAutomation::new()?;
        Ok(Self {
            use_vision,
            use_annotation,
            use_accessibility,
            desktop_state: None,
            live_controls: Vec::new(),
            automation,
            cached_windows_version: None,
            cached_default_language: None,
            cached_user_account_type: None,
        })
    }

    pub fn automation(&self) -> &UiAutomation {
        &self.automation
    }

    /// Captures the desktop state.
    ///
    /// `as_bytes` mirrors the Python signature. This port always stores encoded PNG
    /// bytes (there is no PIL image object), so both modes produce the same data.
    pub fn get_state(&mut self, #[allow(unused_variables)] as_bytes: bool) -> anyhow::Result<&DesktopState> {
        let handles = self.get_controls_handles();
        let (mut windows, window_handles) = self.get_windows(Some(&handles))?;
        let active_window = self.get_active_window(Some(&windows));

        let (active_desktop, all_desktops) = (
            virtual_desktop_to_view(crate::vdm::get_current_desktop()),
            crate::vdm::get_all_desktops()
                .into_iter()
                .map(virtual_desktop_to_view)
                .collect::<Vec<_>>(),
        );

        if let Some(active) = &active_window {
            if let Some(index) = windows.iter().position(|window| window.handle == active.handle) {
                windows.remove(index);
            }
        }

        let active_handle = active_window.as_ref().map(|window| window.handle);
        let other_handles: Vec<i64> = handles
            .iter()
            .copied()
            .filter(|handle| !window_handles.contains(handle))
            .collect();

        let (tree_state, live_controls) = if self.use_accessibility {
            let screen_box = self.screen_box();
            let capture = capture_tree_state(
                &self.automation,
                active_handle,
                &other_handles,
                screen_box,
                |handle| self.is_window_browser_handle(handle),
            );
            (Some(capture.state), capture.live_controls)
        } else {
            (None, Vec::new())
        };

        let screenshot = if self.use_vision {
            let nodes = if self.use_annotation {
                tree_state
                    .as_ref()
                    .map(|state| state.interactive_nodes.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            self.capture_screenshot(&nodes)?
        } else {
            None
        };

        self.live_controls = live_controls;

        let state = DesktopState {
            active_desktop,
            all_desktops,
            windows,
            active_window,
            screenshot,
            tree_state,
        };
        self.desktop_state = Some(state);
        Ok(self.desktop_state.as_ref().expect("desktop state"))
    }

    /// Captures the virtual screen as PNG, annotated when annotation is enabled.
    fn capture_screenshot(
        &self,
        nodes: &[crate::tree::views::TreeElementNode],
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let screenshot = crate::screenshot::capture_virtual_screen()?;
        if self.use_annotation && !nodes.is_empty() {
            let boxes: Vec<(i32, i32, i32, i32)> = nodes
                .iter()
                .map(|node| {
                    (
                        node.bounding_box.left,
                        node.bounding_box.top,
                        node.bounding_box.right,
                        node.bounding_box.bottom,
                    )
                })
                .collect();
            let annotated = crate::screenshot::annotate_screenshot(&screenshot, &boxes, 5);
            return Ok(Some(annotated.to_png()?));
        }
        Ok(Some(screenshot.to_png()?))
    }

    fn screen_box(&self) -> uia::BoundingRectangle {
        let (left, top, width, height) = crate::screenshot::virtual_screen_rect();
        uia::BoundingRectangle {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    pub fn get_screen_size(&self) -> Size {
        let (width, height) = uia::virtual_screen_size();
        Size { width, height }
    }

    pub fn get_cursor_location(&self) -> (i32, i32) {
        uia::get_cursor_pos()
    }

    pub fn get_element_under_cursor(&self) -> anyhow::Result<ElementHandle> {
        let (x, y) = uia::get_cursor_pos();
        Ok(self.automation.element_from_point(x, y)?)
    }

    /// Runs a PowerShell command and returns `(output, exit_code)`.
    pub fn execute_command(&self, command: &str, timeout: u64) -> (String, i32) {
        let utf8_command = format!(
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {command}"
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(
            utf8_command
                .encode_utf16()
                .flat_map(|unit| unit.to_le_bytes())
                .collect::<Vec<u8>>(),
        );

        let child = Command::new("powershell")
            .args(["-NoProfile", "-EncodedCommand", &encoded])
            .current_dir(crate::win32::home_dir())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                return (
                    format!(
                        "Command execution failed: {}: {error}",
                        std::any::type_name_of_val(&error)
                    ),
                    1,
                )
            }
        };

        let deadline = std::time::Instant::now() + Duration::from_secs(timeout);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        return ("Command execution timed out".to_string(), 1);
                    }
                    sleep(Duration::from_millis(25));
                }
                Err(error) => {
                    return (format!("Command execution failed: {error}"), 1);
                }
            }
        }

        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(error) => return (format!("Command execution failed: {error}"), 1),
        };

        let status = output.status.code().unwrap_or(1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let text = if stdout.is_empty() { stderr } else { stdout };
        (text, status)
    }

    /// Start-menu applications as `name → appid`.
    pub fn get_apps_from_start_menu(&self) -> std::collections::BTreeMap<String, String> {
        let (apps_info, status) = self.execute_command(
            "Get-StartApps | ConvertTo-Csv -NoTypeInformation",
            10,
        );
        if status != 0 || apps_info.trim().is_empty() {
            return std::collections::BTreeMap::new();
        }
        parse_start_apps(&apps_info)
    }

    pub fn get_default_language(&mut self) -> String {
        if let Some(language) = &self.cached_default_language {
            return language.clone();
        }
        let (response, _) = self.execute_command(
            "Get-Culture | Select-Object Name,DisplayName | ConvertTo-Csv -NoTypeInformation",
            10,
        );
        let language = parse_csv_column(&response, "DisplayName").unwrap_or_default();
        self.cached_default_language = Some(language.clone());
        language
    }

    pub fn get_windows_version(&mut self) -> String {
        if let Some(version) = &self.cached_windows_version {
            return version.clone();
        }
        let (response, status) =
            self.execute_command("(Get-CimInstance Win32_OperatingSystem).Caption", 10);
        let result = if status == 0 {
            response.trim().to_string()
        } else {
            "Windows".to_string()
        };
        self.cached_windows_version = Some(result.clone());
        result
    }

    pub fn get_user_account_type(&mut self) -> String {
        if let Some(account_type) = &self.cached_user_account_type {
            return account_type.clone();
        }
        let (response, status) =
            self.execute_command("(Get-LocalUser -Name $env:USERNAME).PrincipalSource", 10);
        let trimmed = response.trim();
        let result = if trimmed == "Local" {
            "Local Account".to_string()
        } else if status == 0 {
            "Microsoft Account".to_string()
        } else {
            "Local Account".to_string()
        };
        self.cached_user_account_type = Some(result.clone());
        result
    }

    pub fn get_dpi_scaling(&self) -> f64 {
        crate::win32::dpi_scaling()
    }

    pub fn is_window_browser_handle(&self, handle: i64) -> bool {
        match crate::win32::process_name_for_window(handle) {
            Some(process) => Browser::has_process(&process),
            None => false,
        }
    }

    pub fn get_window_status(&self, handle: i64) -> Status {
        if crate::win32::is_iconic(handle) {
            Status::Minimized
        } else if crate::win32::is_zoomed(handle) {
            Status::Maximized
        } else if crate::win32::is_window_visible(handle) {
            Status::Normal
        } else {
            Status::Hidden
        }
    }

    /// All candidate window handles (visible windows plus the desktop and taskbars).
    pub fn get_controls_handles(&self) -> BTreeSet<i64> {
        let mut handles = BTreeSet::new();
        for handle in crate::win32::enum_windows() {
            if crate::win32::is_window_visible(handle)
                && crate::vdm::is_window_on_current_desktop(handle)
            {
                handles.insert(handle);
            }
        }
        for (class, title) in [
            ("Progman", None),
            ("Shell_TrayWnd", None),
            ("Shell_SecondaryTrayWnd", None),
        ] {
            if let Some(handle) = crate::win32::find_window(class, title) {
                handles.insert(handle);
            }
        }
        handles
    }

    pub fn is_overlay_window(&self, element: &ElementHandle) -> bool {
        let has_no_children = element.first_child().ok().flatten().is_none();
        let name = element.name();
        has_no_children || name.trim().contains("Overlay")
    }

    /// Enumerates visible top-level windows.
    pub fn get_windows(
        &self,
        controls_handles: Option<&BTreeSet<i64>>,
    ) -> anyhow::Result<(Vec<Window>, BTreeSet<i64>)> {
        let owned;
        let handles = match controls_handles {
            Some(handles) => handles,
            None => {
                owned = self.get_controls_handles();
                &owned
            }
        };

        let mut windows = Vec::new();
        let mut window_handles = BTreeSet::new();

        for (depth, handle) in handles.iter().enumerate() {
            let Ok(element) = self.automation.element_from_handle(*handle) else {
                continue;
            };
            if self.is_overlay_window(&element) {
                continue;
            }
            let snapshot = element.live_snapshot();
            if snapshot.control_type != crate::uia::patterns::control_types::WINDOW
                && snapshot.control_type != crate::uia::patterns::control_types::PANE
            {
                continue;
            }
            let Some((can_minimize, can_maximize)) = window_capabilities(&element) else {
                continue;
            };
            if !(can_minimize && can_maximize) {
                continue;
            }

            let status = self.get_window_status(*handle);
            let Ok(rectangle) = element.bounding_rectangle() else {
                continue;
            };
            if rectangle.is_empty() && status != Status::Minimized {
                continue;
            }

            let process_id = snapshot.process_id;
            windows.push(Window {
                name: snapshot.name,
                is_browser: self.is_window_browser_handle(*handle),
                depth,
                status,
                bounding_box: BoundingBox::from_rectangle(&rectangle),
                handle: *handle,
                process_id,
            });
            window_handles.insert(*handle);
        }

        Ok((windows, window_handles))
    }

    pub fn get_active_window(&self, windows: Option<&[Window]>) -> Option<Window> {
        let handle = crate::win32::foreground_window();
        if handle == 0 {
            return None;
        }
        let element = self.automation.element_from_handle(handle).ok()?;
        if element.class_name() == "Progman" {
            return None;
        }
        let snapshot = element.live_snapshot();

        if let Some(windows) = windows {
            if let Some(window) = windows.iter().find(|window| window.handle == handle) {
                return Some(window.clone());
            }
        }

        let rectangle = element.bounding_rectangle().ok()?;
        Some(Window {
            name: snapshot.name,
            is_browser: self.is_window_browser_handle(handle),
            depth: 0,
            bounding_box: BoundingBox::from_rectangle(&rectangle),
            status: self.get_window_status(handle),
            handle,
            process_id: snapshot.process_id,
        })
    }

    /// Fuzzy lookup of a window by name.
    pub fn find_window_by_name(&self, name: &str) -> Result<Window, String> {
        let state = self
            .desktop_state
            .as_ref()
            .ok_or_else(|| "No desktop state captured".to_string())?;
        let mut candidates: Vec<&Window> = Vec::new();
        if let Some(active) = &state.active_window {
            candidates.push(active);
        }
        candidates.extend(state.windows.iter());

        if candidates.is_empty() {
            return Err("No windows found on the desktop.".to_string());
        }

        let names: Vec<String> = candidates.iter().map(|window| window.name.clone()).collect();
        match best_match(name, &names, 70) {
            Some(index) => Ok(candidates[index].clone()),
            None => Err(format!("Application {} not found.", title_case(name))),
        }
    }

    pub fn is_app_running(&self, name: &str) -> bool {
        match self.get_windows(None) {
            Ok((windows, _)) => {
                let names: Vec<String> = windows.iter().map(|window| window.name.clone()).collect();
                best_match(name, &names, 60).is_some()
            }
            Err(_) => false,
        }
    }

    /// `app_tool` implementation.
    pub fn app(&mut self, request: &AppRequest) -> String {
        match request.mode {
            AppMode::Launch => {
                let Some(name) = request.name.as_deref() else {
                    return "Application name is required.".to_string();
                };
                let (response, status, pid) = self.launch_app(name);
                if status != 0 {
                    return response;
                }
                let mut launched = false;
                if pid > 0 && wait_for_process_window(pid, 10) {
                    launched = true;
                }
                if !launched && wait_for_window_name(name, 10) {
                    launched = true;
                }
                if launched {
                    format!("{} launched.", title_case(name))
                } else {
                    format!(
                        "Launching {} sent, but window not detected yet.",
                        title_case(name)
                    )
                }
            }
            AppMode::Resize => {
                let (response, _status) =
                    self.resize_app(request.name.as_deref(), request.size, request.loc);
                response
            }
            AppMode::Switch => {
                let Some(name) = request.name.as_deref() else {
                    return "Window name is required.".to_string();
                };
                let (response, _status) = self.switch_app(name);
                response
            }
        }
    }

    pub fn resize_app(
        &self,
        name: Option<&str>,
        size: Option<(i32, i32)>,
        loc: Option<(i32, i32)>,
    ) -> (String, i32) {
        let target = match name {
            Some(name) => match self.find_window_by_name(name) {
                Ok(window) => window,
                Err(error) => return (error, 1),
            },
            None => match self.desktop_state.as_ref().and_then(|state| state.active_window.clone()) {
                Some(window) => window,
                None => return ("No active app found".to_string(), 1),
            },
        };

        match target.status {
            Status::Minimized => return (format!("{} is minimized", target.name), 1),
            Status::Maximized => return (format!("{} is maximized", target.name), 1),
            _ => {}
        }

        let Ok(element) = self.automation.element_from_handle(target.handle) else {
            return ("Could not access the window".to_string(), 1);
        };
        let rectangle = element
            .bounding_rectangle()
            .unwrap_or(uia::BoundingRectangle::default());
        let (x, y) = loc.unwrap_or((rectangle.left, rectangle.top));
        let (width, height) = size.unwrap_or((rectangle.width(), rectangle.height()));

        if !crate::win32::move_window(target.handle, x, y, width, height) {
            return ("Window does not support moving".to_string(), 1);
        }

        (
            format!(
                "{} resized to {}x{} at {},{}.",
                target.name, width, height, x, y
            ),
            0,
        )
    }

    pub fn launch_app(&self, name: &str) -> (String, i32, u32) {
        let apps = self.get_apps_from_start_menu();
        if apps.is_empty() {
            return (format!("{} not found in start menu.", title_case(name)), 1, 0);
        }
        let keys: Vec<String> = apps.keys().cloned().collect();
        let Some(index) = best_match(name, &keys, 70) else {
            return (format!("{} not found in start menu.", title_case(name)), 1, 0);
        };
        let app_name = &keys[index];
        let Some(appid) = apps.get(app_name) else {
            return (format!("{} not found in start menu.", title_case(name)), 1, 0);
        };

        // File paths can return the PID through PassThru; AUMIDs cannot.
        if PathBuf::from(appid).exists() || appid.contains('\\') {
            let command = format!(
                "Start-Process \"{appid}\" -PassThru | Select-Object -ExpandProperty Id"
            );
            let (response, status) = self.execute_command(&command, 10);
            let pid = if status == 0 {
                response.trim().parse::<u32>().unwrap_or(0)
            } else {
                0
            };
            (response, status, pid)
        } else {
            let command = format!("Start-Process \"shell:AppsFolder\\{appid}\"");
            let (response, status) = self.execute_command(&command, 10);
            (response, status, 0)
        }
    }

    pub fn switch_app(&self, name: &str) -> (String, i32) {
        let app = match self.find_window_by_name(name) {
            Ok(window) => window,
            Err(error) => return (error, 1),
        };
        let was_minimized = crate::win32::is_iconic(app.handle);
        if let Err(error) = crate::win32::bring_window_to_top(app.handle) {
            return (format!("Failed to bring window to top: {error}"), 1);
        }
        if was_minimized {
            (
                format!("{} restored from minimized and switched to it.", title_case(&app.name)),
                0,
            )
        } else {
            (format!("Switched to {} window.", title_case(&app.name)), 0)
        }
    }

    pub fn bring_window_to_top(&self, handle: i64) -> anyhow::Result<()> {
        crate::win32::bring_window_to_top(handle)
    }

    /// Resolves a selector label to its live control.
    pub fn get_element_handle_from_label(&self, label: usize) -> anyhow::Result<ElementHandle> {
        let state = self
            .desktop_state
            .as_ref()
            .and_then(|state| state.tree_state.as_ref())
            .ok_or_else(|| anyhow::anyhow!("No tree state available"))?;
        let selector = state.build_selector_map();
        let node = selector
            .node_of(label)
            .ok_or_else(|| anyhow::anyhow!("No element found for label {label}"))?;
        if let Some(index) = node.control_index() {
            if let Some(control) = self.live_controls.get(index) {
                return Ok(control.clone());
            }
        }
        if node.hwnd() != 0 {
            return Ok(self.automation.element_from_handle(node.hwnd())?);
        }
        anyhow::bail!("No live control available for label {label}")
    }

    pub fn get_coordinates_from_label(&self, label: usize) -> anyhow::Result<(i32, i32)> {
        let element = self.get_element_handle_from_label(label)?;
        let rectangle = element.bounding_rectangle()?;
        Ok((
            rectangle.left + rectangle.width() / 2,
            rectangle.top + rectangle.height() / 2,
        ))
    }

    pub fn click(&self, loc: (i32, i32), button: &str, clicks: u32) {
        let (x, y) = loc;
        if clicks == 0 {
            uia::set_cursor_pos(x, y);
            return;
        }
        match button {
            "right" => {
                for _ in 0..clicks {
                    uia::right_click_at(x, y);
                }
            }
            "middle" => {
                for _ in 0..clicks {
                    uia::middle_click_at(x, y);
                }
            }
            _ => {
                if clicks >= 2 {
                    uia::double_click_at(x, y);
                } else {
                    uia::click_at(x, y);
                }
            }
        }
    }

    pub fn type_text(
        &self,
        loc: (i32, i32),
        text: &str,
        caret_position: &str,
        clear: bool,
        press_enter: bool,
    ) {
        let (x, y) = loc;
        uia::click_at(x, y);
        match caret_position {
            "start" => {
                let _ = uia::send_keys("{Home}", 0.01, 0.05);
            }
            "end" => {
                let _ = uia::send_keys("{End}", 0.01, 0.05);
            }
            _ => {}
        }
        if clear {
            sleep(Duration::from_millis(500));
            let _ = uia::send_keys("{Ctrl}a", 0.01, 0.05);
            let _ = uia::send_keys("{Back}", 0.01, 0.05);
        }
        let escaped = uia::escape_text_for_sendkeys(text);
        let _ = uia::send_keys(&escaped, 0.01, 0.05);
        if press_enter {
            let _ = uia::send_keys("{Enter}", 0.01, 0.05);
        }
    }

    pub fn scroll(
        &self,
        loc: Option<(i32, i32)>,
        scroll_type: &str,
        direction: &str,
        wheel_times: i32,
    ) -> Option<String> {
        if let Some(loc) = loc {
            self.move_mouse(loc);
        }
        match scroll_type {
            "vertical" => uia::scroll_vertical(direction, wheel_times),
            "horizontal" => uia::scroll_horizontal(direction, wheel_times),
            _ => Some("Invalid type. Use \"horizontal\" or \"vertical\".".to_string()),
        }
    }

    pub fn drag(&self, loc: (i32, i32)) {
        let (cursor_x, cursor_y) = uia::get_cursor_pos();
        uia::drag_to(cursor_x, cursor_y, loc.0, loc.1, 0.5, 20);
    }

    pub fn move_mouse(&self, loc: (i32, i32)) {
        uia::move_to(loc.0, loc.1, 10.0);
    }

    pub fn shortcut(&self, shortcut: &str) {
        let keys: Vec<String> = shortcut.split('+').map(|key| key.trim().to_string()).collect();
        let mut sendkeys = String::new();
        for key in keys {
            if key.chars().count() == 1 {
                sendkeys.push_str(&key);
            } else {
                let name = key_alias(&key);
                sendkeys.push('{');
                sendkeys.push_str(&name);
                sendkeys.push('}');
            }
        }
        let _ = uia::send_keys(&sendkeys, 0.01, 0.5);
    }

    pub fn multi_select(&self, press_ctrl: bool, elements: &[(i32, i32)]) {
        if press_ctrl {
            uia::press_key(uia::input::VK_CONTROL, 0.05);
        }
        for (x, y) in elements {
            uia::click_at(*x, *y);
            sleep(Duration::from_millis(500));
        }
        uia::release_key(uia::input::VK_CONTROL, 0.05);
    }

    pub fn multi_edit(&self, elements: &[(i32, i32, String)]) {
        for (x, y, text) in elements {
            self.type_text((*x, *y), text, "none", true, false);
        }
    }

    /// Fetches a page and converts its HTML to markdown.
    pub async fn scrape(&self, url: &str) -> anyhow::Result<String> {
        let response = reqwest::get(url).await?;
        let html = response.text().await?;
        Ok(crate::markdown::html_to_markdown(&html))
    }

    pub fn auto_minimize(&self) -> AutoMinimizeGuard {
        let handle = crate::win32::foreground_window();
        crate::win32::minimize_window(handle);
        AutoMinimizeGuard { handle }
    }
}

/// Restores the minimized window when dropped.
pub struct AutoMinimizeGuard {
    handle: i64,
}

impl Drop for AutoMinimizeGuard {
    fn drop(&mut self) {
        crate::win32::restore_window(self.handle);
    }
}

fn virtual_desktop_to_view(desktop: crate::vdm::DesktopInfo) -> VirtualDesktop {
    VirtualDesktop { id: desktop.id, name: desktop.name }
}

fn widget_window(element: &ElementHandle) -> Option<windows::Win32::UI::Accessibility::IUIAutomationWindowPattern> {
    unsafe {
        element
            .element
            .GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationWindowPattern>(
                windows::Win32::UI::Accessibility::UIA_WindowPatternId,
            )
            .ok()
    }
}

/// Reports `(CanMinimize, CanMaximize)` for an element's window pattern.
fn window_capabilities(element: &ElementHandle) -> Option<(bool, bool)> {
    let pattern = widget_window(element)?;
    unsafe {
        let can_minimize = pattern.CurrentCanMinimize().ok()?.as_bool();
        let can_maximize = pattern.CurrentCanMaximize().ok()?.as_bool();
        Some((can_minimize, can_maximize))
    }
}

/// Waits for a process to own a visible top-level window.
fn wait_for_process_window(process_id: u32, seconds: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        for handle in crate::win32::enum_windows() {
            if !crate::win32::is_window_visible(handle) {
                continue;
            }
            let mut owner = 0u32;
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                    windows::Win32::Foundation::HWND(handle as _),
                    Some(&mut owner),
                );
            }
            if owner == process_id {
                return true;
            }
        }
        sleep(Duration::from_millis(200));
    }
    false
}

fn wait_for_window_name(name: &str, seconds: u64) -> bool {
    let needle = name.to_lowercase();
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        for handle in crate::win32::enum_windows() {
            if !crate::win32::is_window_visible(handle) {
                continue;
            }
            if let Ok(element) = UiAutomation::new().and_then(|automation| automation.element_from_handle(handle)) {
                if element.name().to_lowercase().contains(&needle) {
                    return true;
                }
            }
        }
        sleep(Duration::from_millis(200));
    }
    false
}

/// Fuzzy best match using fuzzywuzzy's default `WRatio`, mirroring `process.extractOne`.
pub fn best_match(query: &str, choices: &[String], score_cutoff: u32) -> Option<usize> {
    let mut best: Option<(usize, u32)> = None;
    for (index, choice) in choices.iter().enumerate() {
        let score = weighted_ratio(query, choice);
        if score >= score_cutoff && best.map(|(_, best_score)| score > best_score).unwrap_or(true) {
            best = Some((index, score));
        }
    }
    best.map(|(index, _)| index)
}

/// `fuzzywuzzy.fuzz.ratio` — `difflib.SequenceMatcher.ratio()` on the raw strings.
///
/// Note: `fuzz.ratio` does **not** call `full_process`; only the token scorers do.
pub fn ratio(a: &str, b: &str) -> u32 {
    ratio_raw(a, b)
}

/// `difflib.SequenceMatcher.ratio()` for two strings, rounded like `utils.intr`.
fn ratio_raw(a: &str, b: &str) -> u32 {
    let left: Vec<char> = a.chars().collect();
    let right: Vec<char> = b.chars().collect();
    let total = left.len() + right.len();
    if total == 0 {
        return 100;
    }
    let matches = matching_blocks_size(&left, &right);
    ((2.0 * matches as f64 / total as f64) * 100.0).round() as u32
}

/// difflib's `SequenceMatcher.get_matching_blocks()` total matched length.
fn matching_blocks_size(left: &[char], right: &[char]) -> usize {
    if left.is_empty() || right.is_empty() {
        return 0;
    }
    // LCS table; difflib's block total equals the LCS length for equal elements.
    let mut previous = vec![0usize; right.len() + 1];
    let mut current = vec![0usize; right.len() + 1];
    for left_char in left {
        for (j, right_char) in right.iter().enumerate() {
            current[j + 1] = if left_char == right_char {
                previous[j] + 1
            } else {
                previous[j + 1].max(current[j])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// `fuzzywuzzy.fuzz.partial_ratio` — best ratio of the shorter string against
/// windows of the longer string, early-exiting above 99.5%.
pub fn partial_ratio(a: &str, b: &str) -> u32 {
    partial_ratio_raw(a, b)
}

fn partial_ratio_raw(a: &str, b: &str) -> u32 {
    let left: Vec<char> = a.chars().collect();
    let right: Vec<char> = b.chars().collect();
    if left.is_empty() && right.is_empty() {
        return 100;
    }
    if left.is_empty() || right.is_empty() {
        return 0;
    }
    let (shorter, longer) = if left.len() <= right.len() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    let mut best = 0.0f64;
    for start in 0..=(longer.len().saturating_sub(shorter.len())) {
        let window = &longer[start..start + shorter.len()];
        let window_text: String = window.iter().collect();
        let shorter_text: String = shorter.iter().collect();
        let value = ratio_raw(&shorter_text, &window_text) as f64 / 100.0;
        if value > 0.995 {
            return 100;
        }
        if value > best {
            best = value;
        }
    }
    (100.0 * best) as u32
}

/// `fuzzywuzzy.fuzz._process_and_sort`: full_process, split, sort, re-join.
fn process_and_sort(text: &str) -> String {
    let mut tokens: Vec<String> = full_process(text)
        .split_whitespace()
        .map(|token| token.to_string())
        .collect();
    tokens.sort();
    tokens.join(" ")
}

/// `fuzzywuzzy.fuzz.token_sort_ratio`.
pub fn token_sort_ratio(a: &str, b: &str) -> u32 {
    ratio_raw(&process_and_sort(a), &process_and_sort(b))
}

/// `fuzzywuzzy.fuzz._token_set`: three pairwise ratios over the token sets.
pub fn token_set_ratio(a: &str, b: &str) -> u32 {
    token_set_raw(a, b, false)
}

fn token_set_raw(a: &str, b: &str, partial: bool) -> u32 {
    let p1 = full_process(a);
    let p2 = full_process(b);
    if p1.trim().is_empty() || p2.trim().is_empty() {
        return 0;
    }

    let tokens1: std::collections::BTreeSet<String> = p1
        .split_whitespace()
        .map(|token| token.to_string())
        .collect();
    let tokens2: std::collections::BTreeSet<String> = p2
        .split_whitespace()
        .map(|token| token.to_string())
        .collect();

    let intersection: Vec<String> = tokens1
        .intersection(&tokens2)
        .map(|token| token.clone())
        .collect();
    let difference1: Vec<String> = tokens1
        .difference(&tokens2)
        .map(|token| token.clone())
        .collect();
    let difference2: Vec<String> = tokens2
        .difference(&tokens1)
        .map(|token| token.clone())
        .collect();

    let sorted_section = intersection.join(" ");
    let combined_1to2 = format!("{} {}", sorted_section, difference1.join(" "));
    let combined_2to1 = format!("{} {}", sorted_section, difference2.join(" "));

    let compare = |left: &str, right: &str| {
        if partial {
            partial_ratio_raw(left, right)
        } else {
            ratio_raw(left, right)
        }
    };

    [
        compare(sorted_section.trim(), combined_1to2.trim()),
        compare(sorted_section.trim(), combined_2to1.trim()),
        compare(combined_1to2.trim(), combined_2to1.trim()),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

/// `fuzzywuzzy.utils.full_process` — lowercase, replace non-alphanumerics with
/// spaces, and trim the ends while preserving internal spacing.
pub fn full_process(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for character in text.to_lowercase().chars() {
        if character.is_alphanumeric() || character.is_whitespace() {
            cleaned.push(character);
        } else {
            cleaned.push(' ');
        }
    }
    cleaned.trim().to_string()
}

/// `fuzzywuzzy.fuzz.WRatio` — the default scorer behind `process.extractOne`.
pub fn weighted_ratio(a: &str, b: &str) -> u32 {
    let p1 = full_process(a);
    let p2 = full_process(b);
    if p1.is_empty() || p2.is_empty() {
        return 0;
    }

    let unbase_scale = 0.95f64;
    let mut partial_scale = 0.90f64;

    let base = ratio(&p1, &p2) as f64;
    let max_length = p1.chars().count().max(p2.chars().count()) as f64;
    let min_length = p1.chars().count().min(p2.chars().count()).max(1) as f64;
    let len_ratio = max_length / min_length;

    let try_partial = len_ratio >= 1.5;
    if len_ratio > 8.0 {
        partial_scale = 0.60;
    }

    if try_partial {
        let partial = partial_ratio_raw(&p1, &p2) as f64 * partial_scale;
        let ptsor = partial_token_sort(&p1, &p2) as f64 * unbase_scale * partial_scale;
        let ptser = partial_token_set(&p1, &p2) as f64 * unbase_scale * partial_scale;
        return base.max(partial).max(ptsor).max(ptser) as u32;
    }

    let tsor = token_sort_raw(&p1, &p2) as f64 * unbase_scale;
    let tser = token_set_raw(&p1, &p2, false) as f64 * unbase_scale;
    base.max(tsor).max(tser) as u32
}

/// `fuzzywuzzy.fuzz.token_sort_ratio` with `full_process=False`.
fn token_sort_raw(a: &str, b: &str) -> u32 {
    let mut left: Vec<&str> = a.split_whitespace().collect();
    let mut right: Vec<&str> = b.split_whitespace().collect();
    left.sort();
    right.sort();
    ratio_raw(&left.join(" "), &right.join(" "))
}

/// `fuzzywuzzy.fuzz.partial_token_sort_ratio`.
fn partial_token_sort(a: &str, b: &str) -> u32 {
    let mut left: Vec<&str> = a.split_whitespace().collect();
    let mut right: Vec<&str> = b.split_whitespace().collect();
    left.sort();
    right.sort();
    partial_ratio_raw(&left.join(" "), &right.join(" "))
}

/// `fuzzywuzzy.fuzz.partial_token_set_ratio`.
fn partial_token_set(a: &str, b: &str) -> u32 {
    token_set_raw(a, b, true)
}

/// Parses the CSV emitted by `Get-StartApps`.
fn parse_start_apps(text: &str) -> std::collections::BTreeMap<String, String> {
    let mut apps = std::collections::BTreeMap::new();
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(header) = lines.next() else {
        return apps;
    };
    let headers = parse_csv_row(header);
    let name_index = headers.iter().position(|header| header.eq_ignore_ascii_case("Name"));
    let appid_index = headers.iter().position(|header| header.eq_ignore_ascii_case("AppID"));
    let (Some(name_index), Some(appid_index)) = (name_index, appid_index) else {
        return apps;
    };

    for line in lines {
        let row = parse_csv_row(line);
        let name = row.get(name_index).cloned().unwrap_or_default();
        let appid = row.get(appid_index).cloned().unwrap_or_default();
        if !name.is_empty() && !appid.is_empty() {
            apps.insert(name.to_lowercase(), appid);
        }
    }
    apps
}

/// Reads one column of a CSV response by header name.
fn parse_csv_column(text: &str, column: &str) -> Option<String> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header = lines.next()?;
    let headers = parse_csv_row(header);
    let index = headers.iter().position(|name| name == column)?;
    let mut result = String::new();
    for line in lines {
        let row = parse_csv_row(line);
        if let Some(value) = row.get(index) {
            result.push_str(value);
        }
    }
    Some(result)
}

fn parse_csv_row(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' if in_quotes && characters.peek() == Some(&'"') => {
                current.push('"');
                let _ = characters.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    fields.push(current);
    fields
}

/// One element the agent can act on, exposed for tool implementations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractiveElement {
    pub index: usize,
    pub name: String,
    pub control_type: String,
    pub center: (i32, i32),
    pub window_name: String,
}

impl Desktop {
    /// Flat list of interactive elements with their label indices.
    pub fn interactive_elements(&self) -> Vec<InteractiveElement> {
        let Some(state) = self.desktop_state.as_ref().and_then(|state| state.tree_state.as_ref())
        else {
            return Vec::new();
        };
        state
            .interactive_nodes
            .iter()
            .enumerate()
            .map(|(index, node)| InteractiveElement {
                index,
                name: node.name.clone(),
                control_type: node.control_type.clone(),
                center: (node.center.x, node.center.y),
                window_name: node.window_name.clone(),
            })
            .collect()
    }

    /// True when an element's control type is interactive (used by prompts).
    pub fn element_is_interactive(control_type: &str) -> bool {
        is_interactive_control_type(control_type)
    }
}

/// Writes text to a file, used by the memory and file tools.
pub fn write_text_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_scorers_match_python_reference() {
        // Values produced by fuzzywuzzy 0.18 with python-Levenshtein installed.
        let cases = [
            ("chrome", "Google Chrome", 53, 83, 63, 100, 90),
            ("notepad", "Untitled - Notepad", 56, 86, 61, 100, 90),
            ("firefox", "firefox", 100, 100, 100, 100, 100),
            ("edge", "Microsoft Edge", 33, 75, 44, 100, 90),
            ("calc", "Calculator", 43, 75, 57, 57, 90),
        ];
        for (a, b, expected_ratio, expected_partial, expected_sort, expected_set, expected_wratio) in cases {
            assert_eq!(ratio(a, b), expected_ratio, "ratio({a}, {b})");
            assert_eq!(partial_ratio(a, b), expected_partial, "partial_ratio({a}, {b})");
            assert_eq!(token_sort_ratio(a, b), expected_sort, "token_sort_ratio({a}, {b})");
            assert_eq!(token_set_ratio(a, b), expected_set, "token_set_ratio({a}, {b})");
            assert_eq!(weighted_ratio(a, b), expected_wratio, "weighted_ratio({a}, {b})");
        }
    }

    #[test]
    fn ratio_is_case_sensitive_like_fuzzywuzzy() {
        assert_eq!(ratio("chrome", "Chrome"), 83);
        assert_eq!(ratio("CHROME", "chrome"), 0);
        assert_eq!(ratio("same", "same"), 100);
    }

    #[test]
    fn best_match_picks_closest() {
        let choices = vec![
            "Untitled - Notepad".to_string(),
            "Google Chrome".to_string(),
            "File Explorer".to_string(),
        ];
        assert_eq!(best_match("chrome", &choices, 60), Some(1));
        assert_eq!(best_match("notepad", &choices, 70), Some(0));
        assert_eq!(best_match("zzz", &choices, 70), None);
    }

    #[test]
    fn start_apps_csv_parsing() {
        let csv = "\"Name\",\"AppID\"\r\n\"Notepad\",\"notepad.exe\"\r\n\"Calculator\",\"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App\"\r\n";
        let apps = parse_start_apps(csv);
        assert_eq!(apps.get("notepad").map(String::as_str), Some("notepad.exe"));
        assert_eq!(apps.len(), 2);
    }

    #[test]
    fn csv_quoted_commas() {
        let row = parse_csv_row("\"a,b\",\"c\"");
        assert_eq!(row, vec!["a,b".to_string(), "c".to_string()]);
    }

    #[test]
    fn key_alias_table() {
        assert_eq!(key_alias("backspace"), "Back");
        assert_eq!(key_alias("windows"), "Win");
        assert_eq!(key_alias("option"), "Alt");
        assert_eq!(key_alias("ctrl"), "ctrl");
    }

    #[test]
    fn language_column_concatenates_rows() {
        let csv = "\"Name\",\"DisplayName\"\r\n\"sv-SE\",\"svenska (Sverige)\"\r\n";
        assert_eq!(
            parse_csv_column(csv, "DisplayName").as_deref(),
            Some("svenska (Sverige)")
        );
    }
}
