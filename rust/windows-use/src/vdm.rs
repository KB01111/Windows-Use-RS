//! Virtual Desktop Manager, mirroring `windows_use/vdm/core.py`.
//!
//! The internal COM interfaces are undocumented and their vtable layouts differ
//! between Windows builds, so the manager is defined with raw COM calls.

use std::sync::OnceLock;

use windows::core::{GUID, HRESULT};
use windows::Win32::System::Com::{CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};

const CLSID_VIRTUAL_DESKTOP_MANAGER: GUID =
    GUID::from_u128(0xaa509086_5ca9_4c25_8f95_589d3c07b48a);
const CLSID_IMMERSIVE_SHELL: GUID = GUID::from_u128(0xC2F03A33_21F5_47FA_B4BB_156362A2F239);
const CLSID_VIRTUAL_DESKTOP_MANAGER_INTERNAL: GUID =
    GUID::from_u128(0xC5E0CDCA_7B6E_41B2_9FC4_D93975CC467B);
const IID_VIRTUAL_DESKTOP_MANAGER: GUID = GUID::from_u128(0xa5cd92ff_29be_454c_8d04_d82879fb3f1b);
const IID_ISERVICE_PROVIDER: GUID = GUID::from_u128(0x6D5140C1_7436_11CE_8034_00AA006009FA);

/// Windows version group used to pick the internal interface layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionGroup {
    Win10,
    Win11_21H2,
    Win11_22H2Early,
    Win11_22H2Late,
    Win11_23H2,
    Win11_24H2,
}

impl VersionGroup {
    pub fn detect(build: u32, ubr: u32) -> Self {
        if build >= 26100 {
            VersionGroup::Win11_24H2
        } else if (build, ubr) >= (22631, 3085) {
            VersionGroup::Win11_23H2
        } else if (build, ubr) >= (22621, 2215) {
            VersionGroup::Win11_22H2Late
        } else if build >= 22483 {
            VersionGroup::Win11_22H2Early
        } else if build >= 22000 {
            VersionGroup::Win11_21H2
        } else {
            VersionGroup::Win10
        }
    }

    fn uses_hmonitor(&self) -> bool {
        matches!(self, VersionGroup::Win11_21H2 | VersionGroup::Win11_22H2Early)
    }

    fn has_set_name(&self) -> bool {
        !matches!(self, VersionGroup::Win10)
    }

    fn manager_internal_iid(&self) -> GUID {
        match self {
            VersionGroup::Win10 => GUID::from_u128(0xF31574D6_B682_4CDC_BD56_1827860ABEC6),
            VersionGroup::Win11_21H2 | VersionGroup::Win11_22H2Early => {
                GUID::from_u128(0xB2F925B9_5A0F_4D2E_9F4D_2B1507593C10)
            }
            VersionGroup::Win11_22H2Late => {
                GUID::from_u128(0x4970BA3D_FD4E_4647_BEA3_D89076EF4B9C)
            }
            VersionGroup::Win11_23H2 | VersionGroup::Win11_24H2 => {
                GUID::from_u128(0x53F5CA0B_158F_4124_900C_057158060B27)
            }
        }
    }

    fn desktop_iid(&self) -> GUID {
        match self {
            VersionGroup::Win10 => GUID::from_u128(0xFF72FFDD_BE7E_43FC_9C03_AD81681E88E4),
            VersionGroup::Win11_21H2 | VersionGroup::Win11_22H2Early => {
                GUID::from_u128(0x536D3495_B208_4CC9_AE26_DE8111275BF8)
            }
            VersionGroup::Win11_22H2Late => {
                GUID::from_u128(0xA3175F2D_239C_4BD2_8AA0_EEBA8B0B138E)
            }
            VersionGroup::Win11_23H2 | VersionGroup::Win11_24H2 => {
                GUID::from_u128(0x3F07F4BE_B107_441A_AF0F_39D82529072C)
            }
        }
    }
}

type QueryServiceFn =
    unsafe extern "system" fn(*mut std::ffi::c_void, *const GUID, *const GUID, *mut *mut std::ffi::c_void) -> HRESULT;
type IsWindowOnCurrentVirtualDesktopFn =
    unsafe extern "system" fn(*mut std::ffi::c_void, isize, *mut i32) -> HRESULT;
type GetWindowDesktopIdFn = unsafe extern "system" fn(*mut std::ffi::c_void, isize, *mut GUID) -> HRESULT;
type MoveWindowToDesktopFn =
    unsafe extern "system" fn(*mut std::ffi::c_void, isize, *const GUID) -> HRESULT;
type GetDesktopsFn = unsafe extern "system" fn(*mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> HRESULT;
type GetCurrentDesktopFn =
    unsafe extern "system" fn(*mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> HRESULT;
type CreateDesktopFn =
    unsafe extern "system" fn(*mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> HRESULT;
type SwitchDesktopFn = unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> HRESULT;
type RemoveDesktopFn =
    unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void) -> HRESULT;
type FindDesktopFn =
    unsafe extern "system" fn(*mut std::ffi::c_void, *const GUID, *mut *mut std::ffi::c_void) -> HRESULT;
type SetNameFn =
    unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void) -> HRESULT;
type GetDesktopIdFn = unsafe extern "system" fn(*mut std::ffi::c_void, *mut GUID) -> HRESULT;
type ObjectArrayGetCountFn = unsafe extern "system" fn(*mut std::ffi::c_void, *mut u32) -> HRESULT;
type ObjectArrayGetAtFn =
    unsafe extern "system" fn(*mut std::ffi::c_void, u32, *const GUID, *mut *mut std::ffi::c_void) -> HRESULT;

/// Safe wrapper around the documented `IVirtualDesktopManager` and the internal manager.
pub struct VirtualDesktopManager {
    pub version: VersionGroup,
    /// Documented COM interface for per-window queries.
    documented: Option<ComPointer>,
    /// Undocumented interface used for the desktop list and switching.
    internal: Option<ComPointer>,
    desktop_iid: GUID,
    uses_hmonitor: bool,
    has_set_name: bool,
}

/// A raw COM interface pointer with an explicit release.
pub struct ComPointer {
    pointer: *mut std::ffi::c_void,
}

// COM apartment-bound pointers are only used on the thread that created them.
unsafe impl Send for ComPointer {}
unsafe impl Sync for ComPointer {}

impl ComPointer {
    fn new(pointer: *mut std::ffi::c_void) -> Self {
        Self { pointer }
    }

    fn is_null(&self) -> bool {
        self.pointer.is_null()
    }

    fn vtable(&self) -> *const *const std::ffi::c_void {
        unsafe { *(self.pointer as *const *const *const std::ffi::c_void) }
    }

    /// Calls vtable slot `index` (0 = QueryInterface) through a typed function pointer.
    unsafe fn call<T>(&self, index: usize) -> T {
        let vtable = self.vtable();
        let function = *vtable.add(index);
        std::mem::transmute_copy(&function)
    }

    fn release(&mut self) {
        if !self.pointer.is_null() {
            unsafe {
                let release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
                    self.call(2);
                release(self.pointer);
            }
            self.pointer = std::ptr::null_mut();
        }
    }
}

impl Drop for ComPointer {
    fn drop(&mut self) {
        self.release();
    }
}

impl Clone for ComPointer {
    fn clone(&self) -> Self {
        if self.pointer.is_null() {
            return Self { pointer: std::ptr::null_mut() };
        }
        unsafe {
            let add_ref: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 = self.call(1);
            add_ref(self.pointer);
        }
        Self { pointer: self.pointer }
    }
}

fn com_desktop_info(id: String, name: String) -> DesktopInfo {
    DesktopInfo { id, name }
}

/// One virtual desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopInfo {
    pub id: String,
    pub name: String,
}

impl VirtualDesktopManager {
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }

        let build = crate::win32::windows_build();
        let ubr = crate::win32::read_registry_u32(
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
            "UBR",
        )
        .unwrap_or(0);
        let version = VersionGroup::detect(build, ubr);

        let mut manager = Self {
            version,
            documented: None,
            internal: None,
            desktop_iid: version.desktop_iid(),
            uses_hmonitor: version.uses_hmonitor(),
            has_set_name: version.has_set_name(),
        };

        manager.documented = create_documented_manager();
        manager.internal = create_internal_manager(version);
        Ok(manager)
    }

    /// True when the documented manager could be created.
    pub fn is_available(&self) -> bool {
        self.documented.as_ref().map(|pointer| !pointer.is_null()).unwrap_or(false)
    }

    /// True when the internal manager is available (needed for create/switch/remove).
    pub fn can_manage_desktops(&self) -> bool {
        self.internal.as_ref().map(|pointer| !pointer.is_null()).unwrap_or(false)
    }

    pub fn is_window_on_current_desktop(&self, hwnd: i64) -> bool {
        let Some(documented) = &self.documented else {
            return true;
        };
        let mut on_current: i32 = 0;
        unsafe {
            let function: IsWindowOnCurrentVirtualDesktopFn = documented.call(3);
            let status = function(documented.pointer, hwnd as isize, &mut on_current);
            if status.is_ok() {
                return on_current != 0;
            }
        }
        true
    }

    pub fn get_window_desktop_id(&self, hwnd: i64) -> String {
        let Some(documented) = &self.documented else {
            return String::new();
        };
        let mut guid = GUID::zeroed();
        unsafe {
            let function: GetWindowDesktopIdFn = documented.call(4);
            let status = function(documented.pointer, hwnd as isize, &mut guid);
            if status.is_ok() {
                return format_guid(&guid);
            }
        }
        String::new()
    }

    pub fn move_window_to_desktop(&self, hwnd: i64, desktop_name: &str) {
        let Some(documented) = &self.documented else {
            return;
        };
        let Some(guid) = self.resolve_to_guid(desktop_name) else {
            eprintln!("Desktop '{desktop_name}' not found.");
            return;
        };
        let target = parse_guid(&guid).unwrap_or_else(GUID::zeroed);
        unsafe {
            let function: MoveWindowToDesktopFn = documented.call(5);
            let _ = function(documented.pointer, hwnd as isize, &target);
        }
    }

    /// All desktops with names resolved from the registry.
    pub fn get_all_desktops(&self) -> Vec<DesktopInfo> {
        let Some(internal) = &self.internal else {
            return vec![com_desktop_info(
                "00000000-0000-0000-0000-000000000000".to_string(),
                "Default Desktop".to_string(),
            )];
        };
        let Some(array) = self.get_desktops_array(internal) else {
            return vec![com_desktop_info(
                "00000000-0000-0000-0000-000000000000".to_string(),
                "Default Desktop".to_string(),
            )];
        };

        let mut result = Vec::new();
        let count = object_array_count(&array);
        for index in 0..count {
            let Some(desktop) = object_array_at(&array, index, self.desktop_iid) else {
                continue;
            };
            let Some(guid) = desktop_id(&desktop) else {
                continue;
            };
            let guid_string = format_guid(&guid);
            let name = self
                .name_from_registry(&guid_string)
                .unwrap_or_else(|| format!("Desktop {}", index + 1));
            result.push(com_desktop_info(guid_string, name));
        }
        result
    }

    pub fn get_current_desktop(&self) -> DesktopInfo {
        let Some(internal) = &self.internal else {
            return com_desktop_info(
                "00000000-0000-0000-0000-000000000000".to_string(),
                "Default Desktop".to_string(),
            );
        };
        let Some(desktop) = self.current_desktop_raw(internal) else {
            return com_desktop_info("".to_string(), "Unknown".to_string());
        };
        let Some(guid) = desktop_id(&desktop) else {
            return com_desktop_info("".to_string(), "Unknown".to_string());
        };
        let guid_string = format_guid(&guid);
        let name = self
            .get_all_desktops()
            .into_iter()
            .find(|desktop| desktop.id == guid_string)
            .map(|desktop| desktop.name)
            .unwrap_or_else(|| "Unknown".to_string());
        com_desktop_info(guid_string, name)
    }

    /// Creates a desktop and returns its name.
    pub fn create_desktop(&self, name: Option<&str>) -> anyhow::Result<String> {
        let Some(internal) = &self.internal else {
            anyhow::bail!("Internal VDM not initialized");
        };
        let Some(desktop) = self.create_desktop_raw(internal) else {
            anyhow::bail!("CreateDesktop failed");
        };
        let Some(guid) = desktop_id(&desktop) else {
            anyhow::bail!("Could not read the new desktop id");
        };
        let guid_string = format_guid(&guid);

        if let Some(name) = name {
            if self.has_set_name {
                self.rename_desktop_by_guid(&guid_string, name);
                return Ok(name.to_string());
            }
        }
        Ok(self
            .get_all_desktops()
            .last()
            .map(|desktop| desktop.name.clone())
            .unwrap_or(guid_string))
    }

    pub fn remove_desktop(&self, desktop_name: &str) -> anyhow::Result<()> {
        let Some(internal) = &self.internal else {
            anyhow::bail!("Internal VDM not initialized");
        };
        let Some(target_guid_string) = self.resolve_to_guid(desktop_name) else {
            anyhow::bail!("Desktop '{desktop_name}' not found.");
        };
        let target_guid = parse_guid(&target_guid_string).unwrap_or_else(GUID::zeroed);
        let Some(target_desktop) = self.find_desktop(internal, &target_guid) else {
            anyhow::bail!("Could not find desktop with GUID {target_guid_string}");
        };

        let Some(array) = self.get_desktops_array(internal) else {
            anyhow::bail!("Could not enumerate desktops");
        };
        let count = object_array_count(&array);
        let mut fallback: Option<ComPointer> = None;
        for index in 0..count {
            let Some(candidate) = object_array_at(&array, index, self.desktop_iid) else {
                continue;
            };
            let Some(candidate_id) = desktop_id(&candidate) else {
                continue;
            };
            if format_guid(&candidate_id) != format_guid(&target_guid) {
                fallback = Some(candidate);
                break;
            }
        }
        let Some(fallback) = fallback else {
            anyhow::bail!("No fallback desktop found (cannot delete the only desktop)");
        };

        unsafe {
            let remove: RemoveDesktopFn = internal.call(self.remove_desktop_slot());
            let status = remove(internal.pointer, target_desktop.pointer, fallback.pointer);
            if status.is_err() {
                anyhow::bail!("RemoveDesktop failed: {status:?}");
            }
        }
        Ok(())
    }

    pub fn rename_desktop(&self, desktop_name: &str, new_name: &str) -> anyhow::Result<()> {
        if !self.has_set_name {
            anyhow::bail!("Rename is not supported on this Windows version.");
        }
        let Some(target_guid_string) = self.resolve_to_guid(desktop_name) else {
            anyhow::bail!("Desktop '{desktop_name}' not found.");
        };
        self.rename_desktop_by_guid(&target_guid_string, new_name);
        Ok(())
    }

    pub fn rename_desktop_by_guid(&self, guid_string: &str, new_name: &str) {
        let Some(internal) = &self.internal else {
            return;
        };
        if !self.has_set_name {
            return;
        }
        let target_guid = parse_guid(guid_string).unwrap_or_else(GUID::zeroed);
        let Some(target_desktop) = self.find_desktop(internal, &target_guid) else {
            return;
        };
        let Ok(hstring) = hstring_new(new_name) else {
            return;
        };
        let raw = hstring.as_ptr() as *mut std::ffi::c_void;
        unsafe {
            let set_name: SetNameFn = internal.call(self.set_name_slot());
            let status = set_name(internal.pointer, target_desktop.pointer, raw);
            if status.is_err() {
                eprintln!("Failed to rename desktop: {status:?}");
            }
        }
        drop(hstring);
    }

    pub fn switch_desktop(&self, desktop_name: &str) -> anyhow::Result<()> {
        let Some(internal) = &self.internal else {
            anyhow::bail!("Internal VDM not initialized");
        };
        let Some(target_guid_string) = self.resolve_to_guid(desktop_name) else {
            anyhow::bail!("Desktop '{desktop_name}' not found.");
        };
        let target_guid = parse_guid(&target_guid_string).unwrap_or_else(GUID::zeroed);
        let Some(target_desktop) = self.find_desktop(internal, &target_guid) else {
            anyhow::bail!("Desktop '{desktop_name}' not found.");
        };
        unsafe {
            let switch: SwitchDesktopFn = internal.call(self.switch_desktop_slot());
            let status = if self.uses_hmonitor {
                // hMon = 0 in the second slot, desktop pointer third.
                let switch_with_monitor: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    isize,
                    *mut std::ffi::c_void,
                ) -> HRESULT = std::mem::transmute_copy(&switch);
                switch_with_monitor(internal.pointer, 0, target_desktop.pointer)
            } else {
                switch(internal.pointer, target_desktop.pointer)
            };
            if status.is_err() {
                anyhow::bail!("SwitchDesktop failed: {status:?}");
            }
        }
        Ok(())
    }

    fn resolve_to_guid(&self, name: &str) -> Option<String> {
        if let Some(guid) = parse_guid(name) {
            let _ = guid;
            return Some(name.to_string());
        }
        self.get_all_desktops()
            .into_iter()
            .find(|desktop| desktop.name.eq_ignore_ascii_case(name))
            .map(|desktop| desktop.id)
    }

    fn name_from_registry(&self, guid_string: &str) -> Option<String> {
        let path = format!(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VirtualDesktops\\Desktops\\{guid_string}"
        );
        crate::win32::read_registry_string(&path, "Name")
    }

    fn get_desktops_array(&self, internal: &ComPointer) -> Option<ComPointer> {
        let mut array: *mut std::ffi::c_void = std::ptr::null_mut();
        unsafe {
            let status = if self.uses_hmonitor {
                let function: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    isize,
                    *mut *mut std::ffi::c_void,
                ) -> HRESULT = internal.call(self.get_desktops_slot());
                function(internal.pointer, 0, &mut array)
            } else {
                let function: GetDesktopsFn = internal.call(self.get_desktops_slot());
                function(internal.pointer, &mut array)
            };
            if status.is_err() || array.is_null() {
                return None;
            }
        }
        Some(ComPointer::new(array))
    }

    fn current_desktop_raw(&self, internal: &ComPointer) -> Option<ComPointer> {
        let mut desktop: *mut std::ffi::c_void = std::ptr::null_mut();
        unsafe {
            let status = if self.uses_hmonitor {
                let function: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    isize,
                    *mut *mut std::ffi::c_void,
                ) -> HRESULT = internal.call(self.get_current_desktop_slot());
                function(internal.pointer, 0, &mut desktop)
            } else {
                let function: GetCurrentDesktopFn = internal.call(self.get_current_desktop_slot());
                function(internal.pointer, &mut desktop)
            };
            if status.is_err() || desktop.is_null() {
                return None;
            }
        }
        Some(ComPointer::new(desktop))
    }

    fn create_desktop_raw(&self, internal: &ComPointer) -> Option<ComPointer> {
        let mut desktop: *mut std::ffi::c_void = std::ptr::null_mut();
        unsafe {
            let status = if self.uses_hmonitor {
                let function: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    isize,
                    *mut *mut std::ffi::c_void,
                ) -> HRESULT = internal.call(self.create_desktop_slot());
                function(internal.pointer, 0, &mut desktop)
            } else {
                let function: CreateDesktopFn = internal.call(self.create_desktop_slot());
                function(internal.pointer, &mut desktop)
            };
            if status.is_err() || desktop.is_null() {
                return None;
            }
        }
        Some(ComPointer::new(desktop))
    }

    fn find_desktop(&self, internal: &ComPointer, guid: &GUID) -> Option<ComPointer> {
        let mut desktop: *mut std::ffi::c_void = std::ptr::null_mut();
        unsafe {
            let function: FindDesktopFn = internal.call(self.find_desktop_slot());
            let status = function(internal.pointer, guid, &mut desktop);
            if status.is_err() || desktop.is_null() {
                return None;
            }
        }
        Some(ComPointer::new(desktop))
    }

    // Vtable slot indices (0-2 are IUnknown).
    fn get_desktops_slot(&self) -> usize {
        match self.version {
            VersionGroup::Win10 => 7,
            VersionGroup::Win11_21H2 => 7,
            VersionGroup::Win11_22H2Early => 8,
            VersionGroup::Win11_22H2Late | VersionGroup::Win11_23H2 | VersionGroup::Win11_24H2 => 7,
        }
    }

    fn get_current_desktop_slot(&self) -> usize {
        6
    }

    fn create_desktop_slot(&self) -> usize {
        match self.version {
            VersionGroup::Win10 => 10,
            VersionGroup::Win11_21H2 | VersionGroup::Win11_22H2Early => 10,
            VersionGroup::Win11_22H2Late | VersionGroup::Win11_23H2 => 10,
            VersionGroup::Win11_24H2 => 11,
        }
    }

    fn switch_desktop_slot(&self) -> usize {
        9
    }

    fn remove_desktop_slot(&self) -> usize {
        match self.version {
            VersionGroup::Win10 => 11,
            VersionGroup::Win11_21H2 => 12,
            VersionGroup::Win11_22H2Early => 13,
            VersionGroup::Win11_22H2Late | VersionGroup::Win11_23H2 => 12,
            VersionGroup::Win11_24H2 => 13,
        }
    }

    fn find_desktop_slot(&self) -> usize {
        match self.version {
            VersionGroup::Win10 => 12,
            VersionGroup::Win11_21H2 => 13,
            VersionGroup::Win11_22H2Early => 14,
            VersionGroup::Win11_22H2Late | VersionGroup::Win11_23H2 => 13,
            VersionGroup::Win11_24H2 => 14,
        }
    }

    fn set_name_slot(&self) -> usize {
        match self.version {
            VersionGroup::Win10 => 0,
            VersionGroup::Win11_21H2 => 15,
            VersionGroup::Win11_22H2Early => 16,
            VersionGroup::Win11_22H2Late | VersionGroup::Win11_23H2 => 15,
            VersionGroup::Win11_24H2 => 16,
        }
    }
}

fn create_documented_manager() -> Option<ComPointer> {
    unsafe {
        let mut pointer: *mut std::ffi::c_void = std::ptr::null_mut();
        let status = co_create_instance_raw(
            &CLSID_VIRTUAL_DESKTOP_MANAGER,
            &IID_VIRTUAL_DESKTOP_MANAGER,
            &mut pointer,
        );
        if status.is_err() || pointer.is_null() {
            eprintln!("Failed to initialize VirtualDesktopManager: {status:?}");
            return None;
        }
        Some(ComPointer::new(pointer))
    }
}

fn create_internal_manager(version: VersionGroup) -> Option<ComPointer> {
    unsafe {
        let mut shell: *mut std::ffi::c_void = std::ptr::null_mut();
        let status = co_create_instance_raw(
            &CLSID_IMMERSIVE_SHELL,
            &IID_ISERVICE_PROVIDER,
            &mut shell,
        );
        if status.is_err() || shell.is_null() {
            eprintln!("Failed to initialize ImmersiveShell: {status:?}");
            return None;
        }
        let provider = ComPointer::new(shell);

        let mut internal: *mut std::ffi::c_void = std::ptr::null_mut();
        let query_service: QueryServiceFn = provider.call(3);
        let iid = version.manager_internal_iid();
        let status = query_service(
            provider.pointer,
            &CLSID_VIRTUAL_DESKTOP_MANAGER_INTERNAL,
            &iid,
            &mut internal,
        );
        if status.is_err() || internal.is_null() {
            eprintln!("Failed to initialize VirtualDesktopManagerInternal: {status:?}");
            return None;
        }
        Some(ComPointer::new(internal))
    }
}

/// `CoCreateInstance` returning a raw interface pointer.
unsafe fn co_create_instance_raw(class: &GUID, interface: &GUID, out: &mut *mut std::ffi::c_void) -> HRESULT {
    let function: unsafe extern "system" fn(
        *const GUID,
        *mut std::ffi::c_void,
        u32,
        *const GUID,
        *mut *mut std::ffi::c_void,
    ) -> HRESULT = co_create_instance_pointer();
    function(class, std::ptr::null_mut(), CLSCTX_ALL.0, interface, out)
}

type CoCreateInstanceFn = unsafe extern "system" fn(
    *const GUID,
    *mut std::ffi::c_void,
    u32,
    *const GUID,
    *mut *mut std::ffi::c_void,
) -> HRESULT;

unsafe fn co_create_instance_pointer() -> CoCreateInstanceFn {
    // Resolve ole32!CoCreateInstance once; the raw signature keeps the vtable-driven
    // code free of typed `windows` interfaces.
    static POINTER: OnceLock<usize> = OnceLock::new();
    let address = *POINTER.get_or_init(|| {
        let library = windows::Win32::System::LibraryLoader::LoadLibraryW(windows::core::w!(
            "ole32.dll"
        ))
        .expect("ole32.dll");
        let function = windows::Win32::System::LibraryLoader::GetProcAddress(
            library,
            windows::core::s!("CoCreateInstance"),
        )
        .expect("CoCreateInstance");
        function as usize
    });
    std::mem::transmute::<usize, CoCreateInstanceFn>(address)
}

fn object_array_count(array: &ComPointer) -> u32 {
    let mut count: u32 = 0;
    unsafe {
        let function: ObjectArrayGetCountFn = array.call(3);
        let status = function(array.pointer, &mut count);
        if status.is_err() {
            return 0;
        }
    }
    count
}

fn object_array_at(array: &ComPointer, index: u32, iid: GUID) -> Option<ComPointer> {
    let mut element: *mut std::ffi::c_void = std::ptr::null_mut();
    unsafe {
        let function: ObjectArrayGetAtFn = array.call(4);
        let status = function(array.pointer, index, &iid, &mut element);
        if status.is_err() || element.is_null() {
            return None;
        }
    }
    Some(ComPointer::new(element))
}

fn desktop_id(desktop: &ComPointer) -> Option<GUID> {
    let mut guid = GUID::zeroed();
    unsafe {
        let function: GetDesktopIdFn = desktop.call(4);
        let status = function(desktop.pointer, &mut guid);
        if status.is_err() {
            return None;
        }
    }
    Some(guid)
}

fn format_guid(guid: &GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}

fn parse_guid(text: &str) -> Option<GUID> {
    let trimmed = text.trim().trim_start_matches('{').trim_end_matches('}');
    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() != 5 {
        return None;
    }
    let data1 = u32::from_str_radix(parts[0], 16).ok()?;
    let data2 = u16::from_str_radix(parts[1], 16).ok()?;
    let data3 = u16::from_str_radix(parts[2], 16).ok()?;
    if parts[3].len() != 4 || parts[4].len() != 12 {
        return None;
    }
    let mut data4 = [0u8; 8];
    let bytes3 = parts[3].as_bytes();
    for index in 0..2 {
        let high = (bytes3[index * 2] as char).to_digit(16)? as u8;
        let low = (bytes3[index * 2 + 1] as char).to_digit(16)? as u8;
        data4[index] = high << 4 | low;
    }
    let bytes4 = parts[4].as_bytes();
    for index in 0..6 {
        let high = (bytes4[index * 2] as char).to_digit(16)? as u8;
        let low = (bytes4[index * 2 + 1] as char).to_digit(16)? as u8;
        data4[index + 2] = high << 4 | low;
    }
    Some(GUID { data1, data2, data3, data4 })
}

fn hstring_new(text: &str) -> anyhow::Result<windows::core::HSTRING> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        windows::Win32::System::WinRT::WindowsCreateString(Some(&wide))
            .map_err(|error| anyhow::anyhow!("WindowsCreateString failed: {error}"))
    }
}

/// Process-wide manager used by the module-level helpers.
pub fn global_manager() -> &'static Option<VirtualDesktopManager> {
    static INSTANCE: OnceLock<Option<VirtualDesktopManager>> = OnceLock::new();
    INSTANCE.get_or_init(|| VirtualDesktopManager::new().ok())
}

/// True when the window sits on the active virtual desktop.
pub fn is_window_on_current_desktop(hwnd: i64) -> bool {
    match global_manager() {
        Some(manager) => manager.is_window_on_current_desktop(hwnd),
        None => true,
    }
}

pub fn get_window_desktop_id(hwnd: i64) -> String {
    match global_manager() {
        Some(manager) => manager.get_window_desktop_id(hwnd),
        None => String::new(),
    }
}

pub fn move_window_to_desktop(hwnd: i64, desktop_id: &str) {
    if let Some(manager) = global_manager() {
        manager.move_window_to_desktop(hwnd, desktop_id);
    }
}

pub fn create_desktop(name: Option<&str>) -> anyhow::Result<String> {
    match global_manager() {
        Some(manager) => manager.create_desktop(name),
        None => anyhow::bail!("Virtual desktop manager unavailable"),
    }
}

pub fn remove_desktop(desktop_name: &str) -> anyhow::Result<()> {
    match global_manager() {
        Some(manager) => manager.remove_desktop(desktop_name),
        None => anyhow::bail!("Virtual desktop manager unavailable"),
    }
}

pub fn rename_desktop(desktop_name: &str, new_name: &str) -> anyhow::Result<()> {
    match global_manager() {
        Some(manager) => manager.rename_desktop(desktop_name, new_name),
        None => anyhow::bail!("Virtual desktop manager unavailable"),
    }
}

pub fn switch_desktop(desktop_name: &str) -> anyhow::Result<()> {
    match global_manager() {
        Some(manager) => manager.switch_desktop(desktop_name),
        None => anyhow::bail!("Virtual desktop manager unavailable"),
    }
}

pub fn get_all_desktops() -> Vec<DesktopInfo> {
    match global_manager() {
        Some(manager) => manager.get_all_desktops(),
        None => vec![com_desktop_info(
            "00000000-0000-0000-0000-000000000000".to_string(),
            "Default Desktop".to_string(),
        )],
    }
}

pub fn get_current_desktop() -> DesktopInfo {
    match global_manager() {
        Some(manager) => manager.get_current_desktop(),
        None => com_desktop_info(
            "00000000-0000-0000-0000-000000000000".to_string(),
            "Default Desktop".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_groups_match_python_boundaries() {
        assert_eq!(VersionGroup::detect(19045, 0), VersionGroup::Win10);
        assert_eq!(VersionGroup::detect(22000, 0), VersionGroup::Win11_21H2);
        assert_eq!(VersionGroup::detect(22483, 0), VersionGroup::Win11_22H2Early);
        assert_eq!(VersionGroup::detect(22621, 2215), VersionGroup::Win11_22H2Late);
        assert_eq!(VersionGroup::detect(22631, 3085), VersionGroup::Win11_23H2);
        assert_eq!(VersionGroup::detect(26100, 0), VersionGroup::Win11_24H2);
    }

    #[test]
    fn guid_roundtrip() {
        let text = "{A3175F2D-239C-4BD2-8AA0-EEBA8B0B138E}";
        let guid = parse_guid(text).expect("parse");
        assert_eq!(format_guid(&guid), text);
    }

    #[test]
    fn non_guid_names_are_not_parsed() {
        assert!(parse_guid("Desktop 1").is_none());
        assert!(parse_guid("Work").is_none());
    }
}
