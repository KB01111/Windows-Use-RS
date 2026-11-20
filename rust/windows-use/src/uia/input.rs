//! Mouse and keyboard simulation, mirroring `windows_use/uia/core.py` behavior.

use std::thread::sleep;
use std::time::Duration;

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, mouse_event, SendInput, GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_HWHEEL,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SetCursorPos, SM_CXSCREEN, SM_CXVIRTUALSCREEN,
    SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Python's `OPERATION_WAIT_TIME`.
pub const OPERATION_WAIT_TIME: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

fn sleep_seconds(seconds: f64) {
    if seconds > 0.0 {
        sleep(Duration::from_secs_f64(seconds));
    }
}

pub fn get_cursor_pos() -> (i32, i32) {
    let mut point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut point);
    }
    (point.x, point.y)
}

pub fn set_cursor_pos(x: i32, y: i32) -> bool {
    unsafe { SetCursorPos(x, y).is_ok() }
}

pub fn screen_size() -> (i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
        )
    }
}

pub fn virtual_screen_size() -> (i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// `(left, top, width, height)` of the virtual screen.
pub fn virtual_screen_rect() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

pub fn double_click_time() -> i32 {
    unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() as i32
    }
}

fn send_mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) {
    unsafe {
        mouse_event(flags, dx, dy, data, 0);
    }
}

/// Python's `Click`: absolute mouse events at x,y scaled to the primary screen.
fn click_button(x: i32, y: i32, down: MOUSE_EVENT_FLAGS, up: MOUSE_EVENT_FLAGS) {
    set_cursor_pos(x, y);
    let (width, height) = screen_size();
    let scaled_x = x * 65535 / width.max(1);
    let scaled_y = y * 65535 / height.max(1);
    send_mouse(down | MOUSEEVENTF_ABSOLUTE, scaled_x, scaled_y, 0);
    sleep_seconds(0.05);
    send_mouse(up | MOUSEEVENTF_ABSOLUTE, scaled_x, scaled_y, 0);
    sleep_seconds(OPERATION_WAIT_TIME);
}

pub fn click_at(x: i32, y: i32) {
    click_button(x, y, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP);
}

pub fn right_click_at(x: i32, y: i32) {
    click_button(x, y, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP);
}

pub fn middle_click_at(x: i32, y: i32) {
    click_button(x, y, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP);
}

pub fn double_click_at(x: i32, y: i32) {
    let inter_click = double_click_time() as f64 / 2000.0;
    click_with_wait(x, y, inter_click);
    click_with_wait(x, y, OPERATION_WAIT_TIME);
}

fn click_with_wait(x: i32, y: i32, wait: f64) {
    set_cursor_pos(x, y);
    let (width, height) = screen_size();
    let scaled_x = x * 65535 / width.max(1);
    let scaled_y = y * 65535 / height.max(1);
    send_mouse(MOUSEEVENTF_LEFTDOWN | MOUSEEVENTF_ABSOLUTE, scaled_x, scaled_y, 0);
    sleep_seconds(0.05);
    send_mouse(MOUSEEVENTF_LEFTUP | MOUSEEVENTF_ABSOLUTE, scaled_x, scaled_y, 0);
    sleep_seconds(wait);
}

/// Python's `PressMouse`.
pub fn press_mouse(x: i32, y: i32, wait_time: f64) {
    set_cursor_pos(x, y);
    let (width, height) = screen_size();
    send_mouse(
        MOUSEEVENTF_LEFTDOWN | MOUSEEVENTF_ABSOLUTE,
        x * 65535 / width.max(1),
        y * 65535 / height.max(1),
        0,
    );
    sleep_seconds(wait_time);
}

/// Python's `ReleaseMouse`.
pub fn release_mouse(wait_time: f64) {
    let (x, y) = get_cursor_pos();
    let (width, height) = screen_size();
    send_mouse(
        MOUSEEVENTF_LEFTUP | MOUSEEVENTF_ABSOLUTE,
        x * 65535 / width.max(1),
        y * 65535 / height.max(1),
        0,
    );
    sleep_seconds(wait_time);
}

/// Python's `MoveTo`: interpolated pointer movement.
pub fn move_to(x: i32, y: i32, move_speed: f64) {
    const MAX_MOVE_SECOND: f64 = 1.0;
    let move_time = if move_speed <= 0.0 {
        0.0
    } else {
        MAX_MOVE_SECOND / move_speed
    };
    let (current_x, current_y) = get_cursor_pos();
    let x_count = (x - current_x).abs();
    let y_count = (y - current_y).abs();
    let mut max_point = x_count.max(y_count);
    let (screen_width, screen_height) = screen_size();
    let max_side = screen_width.max(screen_height) as f64;
    let min_side = screen_width.min(screen_height);
    let mut move_time = move_time;

    if max_point > min_side {
        max_point = min_side;
    }
    if max_point < max_side as i32 {
        max_point = 100 + ((max_side - 100.0) / max_side * max_point as f64) as i32;
        move_time = move_time * max_point as f64 / max_side;
    }

    let step_count = max_point / 20;
    if step_count > 1 {
        let x_step = (x - current_x) as f64 / step_count as f64;
        let y_step = (y - current_y) as f64 / step_count as f64;
        let interval = move_time / step_count as f64;
        for index in 0..step_count {
            let cx = current_x + (x_step * index as f64) as i32;
            let cy = current_y + (y_step * index as f64) as i32;
            set_cursor_pos(cx, cy);
            sleep_seconds(interval);
        }
    }
    set_cursor_pos(x, y);
    sleep_seconds(OPERATION_WAIT_TIME);
}

/// Python's `DragTo`.
pub fn drag_to(start_x: i32, start_y: i32, end_x: i32, end_y: i32, duration: f64, steps: i32) {
    press_mouse(start_x, start_y, 0.05);
    let step_delay = if steps > 0 {
        duration / steps as f64
    } else {
        0.0
    };
    for index in 1..=steps {
        let progress = index as f64 / steps as f64;
        let cx = start_x + ((end_x - start_x) as f64 * progress) as i32;
        let cy = start_y + ((end_y - start_y) as f64 * progress) as i32;
        set_cursor_pos(cx, cy);
        sleep_seconds(step_delay);
    }
    release_mouse(0.05);
}

pub fn wheel_up(wheel_times: i32, interval: f64) {
    for _ in 0..wheel_times {
        send_mouse(MOUSEEVENTF_WHEEL, 0, 0, 120);
        sleep_seconds(interval);
    }
    sleep_seconds(OPERATION_WAIT_TIME);
}

pub fn wheel_down(wheel_times: i32, interval: f64) {
    for _ in 0..wheel_times {
        send_mouse(MOUSEEVENTF_WHEEL, 0, 0, -120);
        sleep_seconds(interval);
    }
    sleep_seconds(OPERATION_WAIT_TIME);
}

pub fn wheel_left(wheel_times: i32, interval: f64) {
    for _ in 0..wheel_times {
        press_key(VK_SHIFT, 0.05);
        send_mouse(MOUSEEVENTF_WHEEL, 0, 0, 120);
        sleep_seconds(interval);
        release_key(VK_SHIFT, 0.05);
    }
    sleep_seconds(OPERATION_WAIT_TIME);
}

pub fn wheel_right(wheel_times: i32, interval: f64) {
    for _ in 0..wheel_times {
        press_key(VK_SHIFT, 0.05);
        send_mouse(MOUSEEVENTF_WHEEL, 0, 0, -120);
        sleep_seconds(interval);
        release_key(VK_SHIFT, 0.05);
    }
    sleep_seconds(OPERATION_WAIT_TIME);
}

/// Horizontal wheel without Shift, used by `scroll_tool`'s horizontal mode.
pub fn scroll_vertical(direction: &str, wheel_times: i32) -> Option<String> {
    match direction {
        "up" => {
            wheel_up(wheel_times, 0.05);
            None
        }
        "down" => {
            wheel_down(wheel_times, 0.05);
            None
        }
        _ => Some("Invalid direction. Use \"up\" or \"down\".".to_string()),
    }
}

pub fn scroll_horizontal(direction: &str, wheel_times: i32) -> Option<String> {
    match direction {
        "left" => {
            wheel_left(wheel_times, 0.05);
            None
        }
        "right" => {
            wheel_right(wheel_times, 0.05);
            None
        }
        _ => Some("Invalid direction. Use \"left\" or \"right\".".to_string()),
    }
}

pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_LWIN: u16 = 0x5B;

fn key_down(vk: u16) {
    unsafe {
        keybd_event(vk as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
    }
}

fn key_up(vk: u16) {
    unsafe {
        keybd_event(vk as u8, 0, KEYEVENTF_KEYUP, 0);
    }
}

/// Public key-down for the SendKeys executor.
pub fn key_down_public(vk: u16) {
    key_down(vk);
}

/// Public key-up for the SendKeys executor.
pub fn key_up_public(vk: u16) {
    key_up(vk);
}

/// Python's `PressKey`.
pub fn press_key(vk: u16, wait_time: f64) {
    key_down(vk);
    sleep_seconds(wait_time);
}

/// Python's `ReleaseKey`.
pub fn release_key(vk: u16, wait_time: f64) {
    key_up(vk);
    sleep_seconds(wait_time);
}

/// Sends one Unicode character as a key press/release pair.
pub fn type_char(character: char) {
    let mut buffer = [0u16; 2];
    for unit in character.encode_utf16(&mut buffer) {
        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: *unit,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: *unit,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

/// Sends a virtual-key press/release pair.
pub fn tap_key(vk: u16) {
    key_down(vk);
    sleep_seconds(0.01);
    key_up(vk);
}

pub fn is_key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// Name → virtual-key mapping used by `shortcut_tool` and SendKeys translation.
pub fn virtual_key_for_name(name: &str) -> Option<u16> {
    let upper = name.to_uppercase();
    let key = match upper.as_str() {
        "BACK" | "BACKSPACE" => 0x08,
        "TAB" => 0x09,
        "CLEAR" => 0x0C,
        "RETURN" | "ENTER" => 0x0D,
        "SHIFT" => VK_SHIFT,
        "CTRL" | "CONTROL" => VK_CONTROL,
        "ALT" | "MENU" => VK_MENU,
        "PAUSE" => 0x13,
        "CAPITAL" | "CAPSLOCK" => 0x14,
        "ESC" | "ESCAPE" => 0x1B,
        "SPACE" => 0x20,
        "PAGEUP" | "PRIOR" => 0x21,
        "PAGEDOWN" | "NEXT" => 0x22,
        "END" => 0x23,
        "HOME" => 0x24,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        "SELECT" => 0x29,
        "PRINT" | "PRINTSCREEN" => 0x2A,
        "EXECUTE" => 0x2B,
        "SNAPSHOT" => 0x2C,
        "INSERT" => 0x2D,
        "DELETE" | "DEL" => 0x2E,
        "HELP" => 0x2F,
        "WIN" | "LWIN" | "WINDOWS" => VK_LWIN,
        "RWIN" => 0x5C,
        "APPS" => 0x5D,
        "SLEEP" => 0x5F,
        "NUMPAD0" => 0x60,
        "NUMPAD1" => 0x61,
        "NUMPAD2" => 0x62,
        "NUMPAD3" => 0x63,
        "NUMPAD4" => 0x64,
        "NUMPAD5" => 0x65,
        "NUMPAD6" => 0x66,
        "NUMPAD7" => 0x67,
        "NUMPAD8" => 0x68,
        "NUMPAD9" => 0x69,
        "MULTIPLY" => 0x6A,
        "ADD" => 0x6B,
        "SEPARATOR" => 0x6C,
        "SUBTRACT" => 0x6D,
        "DECIMAL" => 0x6E,
        "DIVIDE" => 0x6F,
        "F1" => 0x70,
        "F2" => 0x71,
        "F3" => 0x72,
        "F4" => 0x73,
        "F5" => 0x74,
        "F6" => 0x75,
        "F7" => 0x76,
        "F8" => 0x77,
        "F9" => 0x78,
        "F10" => 0x79,
        "F11" => 0x7A,
        "F12" => 0x7B,
        "F13" => 0x7C,
        "F14" => 0x7D,
        "F15" => 0x7E,
        "F16" => 0x7F,
        "F17" => 0x80,
        "F18" => 0x81,
        "F19" => 0x82,
        "F20" => 0x83,
        "F21" => 0x84,
        "F22" => 0x85,
        "F23" => 0x86,
        "F24" => 0x87,
        "NUMLOCK" => 0x90,
        "SCROLL" => 0x91,
        "LSHIFT" => 0xA0,
        "RSHIFT" => 0xA1,
        "LCTRL" | "LCONTROL" => 0xA2,
        "RCTRL" | "RCONTROL" => 0xA3,
        "LALT" => 0xA4,
        "RALT" => 0xA5,
        "OEM_1" | ";" => 0xBA,
        "OEM_PLUS" | "=" => 0xBB,
        "OEM_COMMA" | "," => 0xBC,
        "OEM_MINUS" | "-" => 0xBD,
        "OEM_PERIOD" | "." => 0xBE,
        "OEM_2" | "/" => 0xBF,
        "OEM_3" | "`" => 0xC0,
        "OEM_4" | "[" => 0xDB,
        "OEM_5" | "\\" => 0xDC,
        "OEM_6" | "]" => 0xDD,
        "OEM_7" | "'" => 0xDE,
        _ => {
            let mut characters = upper.chars();
            if let (Some(character), None) = (characters.next(), characters.next()) {
                if character.is_ascii_alphanumeric() {
                    return Some(character as u16);
                }
            }
            return None;
        }
    };
    Some(key)
}

/// Modifier virtual keys for shortcut strings.
pub fn modifiers_for(name: &str) -> Option<u16> {
    match name.to_lowercase().as_str() {
        "ctrl" | "control" => Some(VK_CONTROL),
        "shift" => Some(VK_SHIFT),
        "alt" | "option" => Some(VK_MENU),
        "win" | "windows" | "command" => Some(VK_LWIN),
        _ => None,
    }
}

/// A parsed key combination: modifiers held, then the key tapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChord {
    pub modifiers: Vec<u16>,
    pub key: u16,
    /// True when the input was a single literal character rather than a key name.
    pub unicode: Option<char>,
}

/// Simulates a key chord.
pub fn send_chord(chord: &KeyChord) {
    for modifier in &chord.modifiers {
        key_down(*modifier);
        sleep_seconds(0.01);
    }
    if let Some(character) = chord.unicode {
        type_char(character);
    } else {
        tap_key(chord.key);
    }
    for modifier in chord.modifiers.iter().rev() {
        key_up(*modifier);
        sleep_seconds(0.01);
    }
}

/// `show_window` wrapper for SW_RESTORE / SW_MINIMIZE.
pub fn show_window(hwnd: i64, command: i32) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
            windows::Win32::Foundation::HWND(hwnd as _),
            windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD(command),
        );
    }
}

/// Reads a scancode-key state, used for modifier-aware typing.
pub fn scan_pressed(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

pub use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY as EXTENDED_FLAG;

/// Sends a scancode-based key event (extended key support for SendKeys parity).
pub fn send_scancode(vk: u16, key_up: bool, extended: bool) {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    unsafe {
        keybd_event(vk as u8, 0, flags, 0);
    }
}

/// Convenience: horizontal wheel message flag (unused by the scroll tool but part of UIA).
pub const MOUSEEVENTF_HWHEEL_FLAG: MOUSE_EVENT_FLAGS = MOUSEEVENTF_HWHEEL;

/// Represents `KEYEVENTF_SCANCODE` for callers that need scancode injection.
pub const SCANCODE_FLAG: KEYBD_EVENT_FLAGS = KEYEVENTF_SCANCODE;
