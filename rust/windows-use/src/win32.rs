//! Win32 window, process and screen helpers used by the desktop module.

use windows::core::BSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, MAX_PATH};
use windows::Win32::Graphics::Gdi::{
    GetDC, GetDeviceCaps, ReleaseDC, DESKTOPHORZRES, DESKTOPVERTRES, HORZRES, LOGPIXELSX, VERTRES,
};
use windows::Win32::System::ProcessStatus::K32GetModuleBaseNameW;
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, FindWindowW, GetForegroundWindow, GetParent,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed, SetForegroundWindow,
    SetWindowPos, ShowWindow, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SW_MINIMIZE, SW_RESTORE, SW_SHOW,
};

/// True when the window has no parent (top-level).
pub fn is_top_level_window(handle: i64) -> bool {
    unsafe { GetParent(HWND(handle as _)).unwrap_or_default().is_invalid() }
}

pub fn is_iconic(handle: i64) -> bool {
    unsafe { IsIconic(HWND(handle as _)).as_bool() }
}

pub fn is_zoomed(handle: i64) -> bool {
    unsafe { IsZoomed(HWND(handle as _)).as_bool() }
}

pub fn is_window_visible(handle: i64) -> bool {
    unsafe { IsWindowVisible(HWND(handle as _)).as_bool() }
}

pub fn is_window(handle: i64) -> bool {
    unsafe { IsWindow(HWND(handle as _)).as_bool() }
}

pub fn show_window(handle: i64, command: i32) -> bool {
    unsafe {
        ShowWindow(
            HWND(handle as _),
            windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD(command),
        )
        .as_bool()
    }
}

pub fn minimize_window(handle: i64) {
    show_window(handle, SW_MINIMIZE.0);
}

pub fn restore_window(handle: i64) {
    show_window(handle, SW_RESTORE.0);
}

pub fn show_window_show(handle: i64) {
    show_window(handle, SW_SHOW.0);
}

pub fn foreground_window() -> i64 {
    unsafe { GetForegroundWindow().0 as i64 }
}

/// Enumerates all top-level windows in z-order (visibility is not filtered).
pub fn enum_windows() -> Vec<i64> {
    unsafe extern "system" fn callback(handle: HWND, parameter: LPARAM) -> BOOL {
        let windows = unsafe { &mut *(parameter.0 as *mut Vec<i64>) };
        windows.push(handle.0 as i64);
        BOOL(1)
    }

    let mut windows: Vec<i64> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut windows as *mut _ as isize));
    }
    windows
}

/// `FindWindow(class_name, title)` returning the handle when found.
pub fn find_window(class_name: &str, title: Option<&str>) -> Option<i64> {
    let class = BSTR::from(class_name);
    let window_name = title.map(BSTR::from);
    unsafe {
        let handle = FindWindowW(
            windows::core::PCWSTR(class.as_ptr()),
            window_name
                .as_ref()
                .map(|name| windows::core::PCWSTR(name.as_ptr()))
                .unwrap_or(windows::core::PCWSTR::null()),
        )
        .ok()?;
        Some(handle.0 as i64)
    }
}

/// The process executable name for a window's owning process.
pub fn process_name_for_window(handle: i64) -> Option<String> {
    let mut process_id = 0u32;
    unsafe {
        GetWindowThreadProcessId(HWND(handle as _), Some(&mut process_id));
    }
    if process_id == 0 {
        return None;
    }
    process_name_for_pid(process_id)
}

pub fn process_name_for_pid(process_id: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, process_id)
            .ok()?;
        let mut buffer = [0u16; MAX_PATH as usize];
        let length = K32GetModuleBaseNameW(process, None, &mut buffer);
        let _ = windows::Win32::Foundation::CloseHandle(process);
        if length == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

/// Brings a window to the foreground, attaching thread input like the Python port.
pub fn bring_window_to_top(target_handle: i64) -> anyhow::Result<()> {
    if !is_window(target_handle) {
        anyhow::bail!("Invalid window handle");
    }
    unsafe {
        if is_iconic(target_handle) {
            restore_window(target_handle);
        }

        let foreground = foreground_window();
        let mut foreground_process = 0u32;
        let foreground_thread =
            GetWindowThreadProcessId(HWND(foreground as _), Some(&mut foreground_process));
        let mut target_process = 0u32;
        let target_thread =
            GetWindowThreadProcessId(HWND(target_handle as _), Some(&mut target_process));

        if foreground_thread == 0 || target_thread == 0 || foreground_thread == target_thread {
            let _ = SetForegroundWindow(HWND(target_handle as _));
            let _ = BringWindowToTop(HWND(target_handle as _));
            return Ok(());
        }

        let attached = AttachThreadInput(foreground_thread, target_thread, true).as_bool();
        let _ = GetCurrentThreadId();
        let _ = SetForegroundWindow(HWND(target_handle as _));
        let _ = BringWindowToTop(HWND(target_handle as _));
        let _ = SetWindowPos(
            HWND(target_handle as _),
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        if attached {
            let _ = AttachThreadInput(foreground_thread, target_thread, false);
        }
    }
    Ok(())
}

/// Moves and resizes a window, matching `Control.MoveWindow`.
pub fn move_window(handle: i64, x: i32, y: i32, width: i32, height: i32) -> bool {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::MoveWindow(
            HWND(handle as _),
            x,
            y,
            width,
            height,
            true,
        )
        .is_ok()
    }
}

/// DPI scale factor of the primary display (1.0 at 96 DPI).
pub fn dpi_scaling() -> f64 {
    let dc = unsafe { GetDC(None) };
    if dc.is_invalid() {
        return 1.0;
    }
    let dpi = unsafe { GetDeviceCaps(dc, LOGPIXELSX) } as f64;
    unsafe {
        ReleaseDC(None, dc);
    }
    dpi / 96.0
}

/// Primary screen size in physical pixels.
pub fn primary_screen_size() -> (i32, i32) {
    let dc = unsafe { GetDC(None) };
    if dc.is_invalid() {
        return (0, 0);
    }
    let width = unsafe { GetDeviceCaps(dc, DESKTOPHORZRES) };
    let height = unsafe { GetDeviceCaps(dc, DESKTOPVERTRES) };
    unsafe {
        ReleaseDC(None, dc);
    }
    if width > 0 && height > 0 {
        (width, height)
    } else {
        (0, 0)
    }
}

/// Logical screen size reported by `GetSystemMetrics`.
pub fn system_screen_size() -> (i32, i32) {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
        };
        (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
    }
}

/// Monitor work-area dimensions.
pub fn monitor_screen_size() -> (i32, i32) {
    let dc = unsafe { GetDC(None) };
    if dc.is_invalid() {
        return (0, 0);
    }
    let width = unsafe { GetDeviceCaps(dc, HORZRES) };
    let height = unsafe { GetDeviceCaps(dc, VERTRES) };
    unsafe {
        ReleaseDC(None, dc);
    }
    (width, height)
}

#[repr(C)]
struct OsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}

fn rtl_version() -> (u32, u32, u32) {
    let mut info = OsVersionInfo {
        size: std::mem::size_of::<OsVersionInfo>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        service_pack: [0; 128],
    };

    if let Ok(ntdll) = unsafe {
        windows::Win32::System::LibraryLoader::LoadLibraryW(windows::core::w!("ntdll.dll"))
    } {
        unsafe {
            let address = windows::Win32::System::LibraryLoader::GetProcAddress(
                ntdll,
                windows::core::s!("RtlGetVersion"),
            );
            if let Some(function) = address {
                let function: unsafe extern "system" fn(*mut OsVersionInfo) -> i32 =
                    std::mem::transmute(function);
                let _ = function(&mut info);
            }
        }
    }
    (info.major, info.minor, info.build)
}

/// Windows build number, used by the virtual desktop layer.
pub fn windows_build() -> u32 {
    rtl_version().2
}

/// Windows version caption, matching the PowerShell `Win32_OperatingSystem.Caption`.
pub fn windows_version_caption() -> String {
    let (_, _, build) = rtl_version();
    let name = if build >= 22000 { "Windows 11" } else { "Windows 10" };
    format!("Microsoft {name}")
}

/// Reads a `REG_SZ` value from HKCU.
pub fn read_registry_string(path: &str, name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    };
    let mut key = HKEY::default();
    let path_wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(path_wide.as_ptr()),
            0,
            KEY_READ,
            &mut key,
        );
        if status.is_err() {
            return None;
        }
        let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buffer = vec![0u8; 512];
        let mut size = buffer.len() as u32;
        let mut value_type = Default::default();
        let status = RegQueryValueExW(
            key,
            windows::core::PCWSTR(name_wide.as_ptr()),
            None,
            Some(&mut value_type),
            Some(buffer.as_mut_ptr()),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);
        if status.is_err() {
            return None;
        }
        let wide: Vec<u16> = buffer[..size as usize]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        let text = String::from_utf16_lossy(&wide);
        Some(text.trim_end_matches('\0').to_string())
    }
}

/// Reads a `REG_DWORD` value from HKLM.
pub fn read_registry_u32(path: &str, name: &str) -> Option<u32> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    };
    let mut key = HKEY::default();
    let path_wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let status = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(path_wide.as_ptr()),
            0,
            KEY_READ,
            &mut key,
        );
        if status.is_err() {
            return None;
        }
        let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut value_type = Default::default();
        let status = RegQueryValueExW(
            key,
            windows::core::PCWSTR(name_wide.as_ptr()),
            None,
            Some(&mut value_type),
            Some(&mut value as *mut u32 as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);
        if status.is_err() {
            return None;
        }
        Some(value)
    }
}

/// The current user's name.
pub fn current_user() -> String {
    std::env::var("USERNAME").unwrap_or_else(|_| "User".to_string())
}

/// The user's home directory.
pub fn home_dir() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("C:\\Users\\Public"))
}
