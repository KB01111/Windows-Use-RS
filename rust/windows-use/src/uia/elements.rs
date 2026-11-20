//! Safe wrappers over `IUIAutomation` and `IUIAutomationElement`.

use std::sync::OnceLock;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationTreeWalker, TreeScope, TreeScope_Children, TreeScope_Element,
    UIA_AcceleratorKeyPropertyId, UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId,
    UIA_ClassNamePropertyId, UIA_ControlTypePropertyId, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_HasKeyboardFocusPropertyId, UIA_HelpTextPropertyId, UIA_IsControlElementPropertyId,
    UIA_IsEnabledPropertyId, UIA_IsKeyboardFocusablePropertyId, UIA_IsOffscreenPropertyId,
    UIA_IsPasswordPropertyId, UIA_LegacyIAccessibleDefaultActionPropertyId,
    UIA_LegacyIAccessiblePatternId, UIA_LegacyIAccessibleRolePropertyId,
    UIA_LegacyIAccessibleStatePropertyId, UIA_LegacyIAccessibleValuePropertyId,
    UIA_LocalizedControlTypePropertyId, UIA_NamePropertyId, UIA_NativeWindowHandlePropertyId,
    UIA_ProcessIdPropertyId, UIA_RangeValueMaximumPropertyId, UIA_RangeValueMinimumPropertyId,
    UIA_RangeValueValuePropertyId, UIA_ScrollHorizontalScrollPercentPropertyId,
    UIA_ScrollHorizontallyScrollablePropertyId, UIA_ScrollPatternId,
    UIA_ScrollVerticalScrollPercentPropertyId, UIA_ScrollVerticallyScrollablePropertyId,
    UIA_SelectionCanSelectMultiplePropertyId, UIA_SelectionIsSelectionRequiredPropertyId,
    UIA_SelectionItemIsSelectedPropertyId, UIA_SelectionItemSelectionContainerPropertyId,
    UIA_SelectionSelectionPropertyId, UIA_ToggleToggleStatePropertyId, UIA_WindowIsModalPropertyId,
    UIA_WindowPatternId,
};

#[derive(Debug, Clone)]
pub struct UiaError {
    pub message: String,
    pub dead_element: bool,
}

impl UiaError {
    fn new(context: &str, error: windows::core::Error) -> Self {
        let code = error.code().0 as u32;
        // UIA_E_ELEMENTNOTAVAILABLE, UIA_E_NOTSUPPORTED, UIA_E_ELEMENTNOTFOUND
        let dead_element = matches!(code, 0x8004_0201 | 0x8004_0200 | 0x8004_0204);
        Self { message: format!("{context}: {error}"), dead_element }
    }

    fn is_dead(error: &windows::core::Error) -> bool {
        let code = error.code().0 as u32;
        // UIA_E_ELEMENTNOTAVAILABLE, UIA_E_NOTSUPPORTED, UIA_E_ELEMENTNOTFOUND.
        matches!(code, 0x8004_0201 | 0x8004_0200 | 0x8004_0204)
    }

    /// True when the failure actually means "no elements", which UIA reports as a
    /// null out-pointer with an `S_OK`-family HRESULT.
    fn is_empty(error: &windows::core::Error) -> bool {
        let code = error.code().0 as u32;
        code == 0 || code == 1 || Self::is_dead(error)
    }
}

impl std::fmt::Display for UiaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for UiaError {}

pub type UiaResult<T> = std::result::Result<T, UiaError>;

fn map_err(context: &str) -> impl Fn(windows::core::Error) -> UiaError + '_ {
    move |e| UiaError::new(context, e)
}

thread_local! {
    static COM_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Initializes COM (STA) once per thread.
pub fn ensure_com_initialized() {
    COM_READY.with(|ready| {
        if !ready.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            ready.set(true);
        }
    });
}

/// A UIA element handle, mirroring the Python `Control` wrapper.
#[derive(Clone)]
pub struct ElementHandle {
    pub element: IUIAutomationElement,
}

/// A UIA client bound to the calling apartment.
pub struct UiAutomation {
    automation: IUIAutomation,
}

impl UiAutomation {
    pub fn new() -> UiaResult<Self> {
        ensure_com_initialized();
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
                .map_err(map_err("CoCreateInstance(CUIAutomation)"))?;
        Ok(Self { automation })
    }

    pub fn raw(&self) -> &IUIAutomation {
        &self.automation
    }

    pub fn root(&self) -> UiaResult<ElementHandle> {
        let element = unsafe {
            self.automation
                .GetRootElement()
                .map_err(map_err("GetRootElement"))?
        };
        Ok(ElementHandle { element })
    }

    pub fn element_from_handle(&self, hwnd: i64) -> UiaResult<ElementHandle> {
        let element = unsafe {
            self.automation
                .ElementFromHandle(HWND(hwnd as _))
                .map_err(map_err("ElementFromHandle"))?
        };
        Ok(ElementHandle { element })
    }

    pub fn element_from_point(&self, x: i32, y: i32) -> UiaResult<ElementHandle> {
        let point = windows::Win32::Foundation::POINT { x, y };
        let element = unsafe {
            self.automation
                .ElementFromPoint(point)
                .map_err(map_err("ElementFromPoint"))?
        };
        Ok(ElementHandle { element })
    }

    pub fn focused_element(&self) -> UiaResult<ElementHandle> {
        let element = unsafe {
            self.automation
                .GetFocusedElement()
                .map_err(map_err("GetFocusedElement"))?
        };
        Ok(ElementHandle { element })
    }

    pub fn raw_view_walker(&self) -> UiaResult<IUIAutomationTreeWalker> {
        unsafe {
            self.automation
                .RawViewWalker()
                .map_err(map_err("RawViewWalker"))
        }
    }
}

static AUTOMATION_POINTER: OnceLock<usize> = OnceLock::new();

/// Shared automation client for internal helpers.
pub fn automation_client() -> UiaResult<&'static IUIAutomation> {
    if let Some(pointer) = AUTOMATION_POINTER.get() {
        return Ok(unsafe { &*(*pointer as *const IUIAutomation) });
    }
    ensure_com_initialized();
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(map_err("CoCreateInstance(CUIAutomation)"))?;
    let boxed = Box::into_raw(Box::new(automation)) as usize;
    let _ = AUTOMATION_POINTER.set(boxed);
    Ok(unsafe { &*(boxed as *const IUIAutomation) })
}

fn walker() -> UiaResult<IUIAutomationTreeWalker> {
    unsafe {
        automation_client()?
            .RawViewWalker()
            .map_err(map_err("RawViewWalker"))
    }
}

/// Screen-space rectangle, matching the Python `Rect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BoundingRectangle {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl BoundingRectangle {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    pub fn intersect(&self, other: &Self) -> Self {
        let left = self.left.max(other.left);
        let top = self.top.max(other.top);
        let right = self.right.min(other.right);
        let bottom = self.bottom.min(other.bottom);
        if right <= left || bottom <= top {
            return Self::default();
        }
        Self { left, top, right, bottom }
    }
}

/// Cached property snapshot of one element.
#[derive(Debug, Clone, Default)]
pub struct CachedElement {
    pub name: String,
    pub automation_id: String,
    pub localized_control_type: String,
    pub accelerator_key: String,
    pub class_name: String,
    pub control_type: i32,
    pub is_enabled: bool,
    pub is_offscreen: bool,
    pub is_control_element: bool,
    pub has_keyboard_focus: bool,
    pub is_keyboard_focusable: bool,
    pub is_password: bool,
    pub bounding_rectangle: BoundingRectangle,
    pub help_text: String,
    pub legacy_role: Option<i32>,
    pub legacy_value: Option<String>,
    pub legacy_default_action: Option<String>,
    pub process_id: i32,
    pub native_window_handle: i64,
    pub is_cached: bool,
}

fn variant_to_string(value: &windows::core::VARIANT) -> String {
    value.to_string()
}

fn variant_to_i32(value: &windows::core::VARIANT) -> Option<i32> {
    <&windows::core::VARIANT as TryInto<i32>>::try_into(value).ok()
}

fn variant_to_bool(value: &windows::core::VARIANT) -> bool {
    <&windows::core::VARIANT as TryInto<bool>>::try_into(value).unwrap_or(false)
}

// The Python port reads cached properties for traversal and live ones rarely; the
// accessor names differ only by the `Cached`/`Current` prefix, so the two
// snapshots are written out explicitly below.

impl ElementHandle {
    pub fn cached_snapshot(&self) -> CachedElement {
        let mut snapshot = CachedElement { is_cached: true, ..Default::default() };
        unsafe {
            if let Ok(value) = self.element.CachedName() {
                snapshot.name = value.to_string();
            }
            if let Ok(value) = self.element.CachedAutomationId() {
                snapshot.automation_id = value.to_string();
            }
            if let Ok(value) = self.element.CachedLocalizedControlType() {
                snapshot.localized_control_type = value.to_string();
            }
            if let Ok(value) = self.element.CachedAcceleratorKey() {
                snapshot.accelerator_key = value.to_string();
            }
            if let Ok(value) = self.element.CachedClassName() {
                snapshot.class_name = value.to_string();
            }
            if let Ok(value) = self.element.CachedHelpText() {
                snapshot.help_text = value.to_string();
            }
            if let Ok(value) = self.element.CachedControlType() {
                snapshot.control_type = value.0;
            }
            if let Ok(value) = self.element.CachedProcessId() {
                snapshot.process_id = value;
            }
            if let Ok(value) = self.element.CachedIsEnabled() {
                snapshot.is_enabled = value.as_bool();
            }
            if let Ok(value) = self.element.CachedIsOffscreen() {
                snapshot.is_offscreen = value.as_bool();
            }
            if let Ok(value) = self.element.CachedIsControlElement() {
                snapshot.is_control_element = value.as_bool();
            }
            if let Ok(value) = self.element.CachedHasKeyboardFocus() {
                snapshot.has_keyboard_focus = value.as_bool();
            }
            if let Ok(value) = self.element.CachedIsKeyboardFocusable() {
                snapshot.is_keyboard_focusable = value.as_bool();
            }
            if let Ok(value) = self.element.CachedIsPassword() {
                snapshot.is_password = value.as_bool();
            }
            if let Ok(value) = self.element.CachedNativeWindowHandle() {
                snapshot.native_window_handle = value.0 as i64;
            }
            if let Ok(rect) = self.element.CachedBoundingRectangle() {
                snapshot.bounding_rectangle = BoundingRectangle {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                };
            }
            if let Ok(value) = self
                .element
                .GetCachedPropertyValue(UIA_LegacyIAccessibleRolePropertyId)
            {
                snapshot.legacy_role = variant_to_i32(&value);
            }
            if let Ok(value) = self
                .element
                .GetCachedPropertyValue(UIA_LegacyIAccessibleValuePropertyId)
            {
                snapshot.legacy_value = Some(variant_to_string(&value));
            }
            if let Ok(value) = self
                .element
                .GetCachedPropertyValue(UIA_LegacyIAccessibleDefaultActionPropertyId)
            {
                snapshot.legacy_default_action = Some(variant_to_string(&value));
            }
        }
        snapshot
    }

    pub fn live_snapshot(&self) -> CachedElement {
        let mut snapshot = CachedElement { is_cached: false, ..Default::default() };
        unsafe {
            if let Ok(value) = self.element.CurrentName() {
                snapshot.name = value.to_string();
            }
            if let Ok(value) = self.element.CurrentAutomationId() {
                snapshot.automation_id = value.to_string();
            }
            if let Ok(value) = self.element.CurrentLocalizedControlType() {
                snapshot.localized_control_type = value.to_string();
            }
            if let Ok(value) = self.element.CurrentAcceleratorKey() {
                snapshot.accelerator_key = value.to_string();
            }
            if let Ok(value) = self.element.CurrentClassName() {
                snapshot.class_name = value.to_string();
            }
            if let Ok(value) = self.element.CurrentHelpText() {
                snapshot.help_text = value.to_string();
            }
            if let Ok(value) = self.element.CurrentControlType() {
                snapshot.control_type = value.0;
            }
            if let Ok(value) = self.element.CurrentProcessId() {
                snapshot.process_id = value;
            }
            if let Ok(value) = self.element.CurrentIsEnabled() {
                snapshot.is_enabled = value.as_bool();
            }
            if let Ok(value) = self.element.CurrentIsOffscreen() {
                snapshot.is_offscreen = value.as_bool();
            }
            if let Ok(value) = self.element.CurrentIsControlElement() {
                snapshot.is_control_element = value.as_bool();
            }
            if let Ok(value) = self.element.CurrentHasKeyboardFocus() {
                snapshot.has_keyboard_focus = value.as_bool();
            }
            if let Ok(value) = self.element.CurrentIsKeyboardFocusable() {
                snapshot.is_keyboard_focusable = value.as_bool();
            }
            if let Ok(value) = self.element.CurrentIsPassword() {
                snapshot.is_password = value.as_bool();
            }
            if let Ok(value) = self.element.CurrentNativeWindowHandle() {
                snapshot.native_window_handle = value.0 as i64;
            }
            if let Ok(rect) = self.element.CurrentBoundingRectangle() {
                snapshot.bounding_rectangle = BoundingRectangle {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                };
            }
            if let Ok(value) = self.element.GetCurrentPropertyValue(UIA_LegacyIAccessibleRolePropertyId)
            {
                snapshot.legacy_role = variant_to_i32(&value);
            }
            if let Ok(value) = self.element.GetCurrentPropertyValue(UIA_LegacyIAccessibleValuePropertyId)
            {
                snapshot.legacy_value = Some(variant_to_string(&value));
            }
        }
        snapshot
    }

    pub fn name(&self) -> String {
        unsafe {
            self.element
                .CurrentName()
                .map(|value| value.to_string())
                .unwrap_or_default()
        }
    }

    pub fn class_name(&self) -> String {
        unsafe {
            self.element
                .CurrentClassName()
                .map(|value| value.to_string())
                .unwrap_or_default()
        }
    }

    pub fn process_id(&self) -> i32 {
        unsafe { self.element.CurrentProcessId().unwrap_or(0) }
    }

    pub fn native_window_handle(&self) -> i64 {
        unsafe {
            self.element
                .CurrentNativeWindowHandle()
                .map(|handle| handle.0 as i64)
                .unwrap_or(0)
        }
    }

    pub fn bounding_rectangle(&self) -> UiaResult<BoundingRectangle> {
        unsafe {
            self.element
                .CurrentBoundingRectangle()
                .map(|rect| BoundingRectangle {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                })
                .map_err(map_err("CurrentBoundingRectangle"))
        }
    }

    pub fn first_child(&self) -> UiaResult<Option<ElementHandle>> {
        let walker = walker()?;
        unsafe {
            match walker.GetFirstChildElement(&self.element) {
                Ok(element) => Ok(Some(ElementHandle { element })),
                Err(error) if UiaError::is_empty(&error) => Ok(None),
                Err(error) => Err(map_err("GetFirstChildElement")(error)),
            }
        }
    }

    pub fn parent(&self) -> UiaResult<Option<ElementHandle>> {
        let walker = walker()?;
        unsafe {
            match walker.GetParentElement(&self.element) {
                Ok(element) => Ok(Some(ElementHandle { element })),
                Err(error) if UiaError::is_empty(&error) => Ok(None),
                Err(error) => Err(map_err("GetParentElement")(error)),
            }
        }
    }

    /// Children retrieved with the children-scope cache request from the Python port.
    pub fn children(&self) -> UiaResult<Vec<ElementHandle>> {
        self.children_with_scope(TreeScope_Element.0 | TreeScope_Children.0)
    }

    pub fn children_with_scope(&self, scope: i32) -> UiaResult<Vec<ElementHandle>> {
        let request = cached_children_request(scope)?;
        unsafe {
            let updated = self
                .element
                .BuildUpdatedCache(&request)
                .map_err(map_err("BuildUpdatedCache"))?;
            let array = match updated.GetCachedChildren() {
                Ok(array) => array,
                Err(error) if UiaError::is_empty(&error) => return Ok(Vec::new()),
                Err(error) => return Err(map_err("GetCachedChildren")(error)),
            };
            let length = array.Length().unwrap_or(0);
            let mut children = Vec::with_capacity(length.max(0) as usize);
            for index in 0..length {
                if let Ok(element) = array.GetElement(index) {
                    children.push(ElementHandle { element });
                }
            }
            Ok(children)
        }
    }

    /// True when the element still resolves, used for dead-element checks.
    pub fn is_alive(&self) -> bool {
        unsafe { self.element.CurrentControlType().is_ok() }
    }

    /// Element-scope cache build, mirroring `CachedControlHelper.build_cached_control`.
    pub fn build_cached(&self) -> UiaResult<ElementHandle> {
        let request = cached_children_request(TreeScope_Element.0)?;
        let element = unsafe {
            self.element
                .BuildUpdatedCache(&request)
                .map_err(map_err("BuildUpdatedCache"))?
        };
        Ok(ElementHandle { element })
    }
}

/// Plain-text typed into an element via the value pattern, used by tools.
pub fn set_value(element: &ElementHandle, text: &str) -> UiaResult<()> {
    use windows::Win32::UI::Accessibility::IUIAutomationValuePattern;
    let pattern = unsafe {
        element
            .element
            .GetCachedPatternAs::<IUIAutomationValuePattern>(
                windows::Win32::UI::Accessibility::UIA_ValuePatternId,
            )
    };
    if let Ok(pattern) = pattern {
        unsafe {
            pattern
                .SetValue(&windows::core::BSTR::from(text))
                .map_err(map_err("SetValue"))?;
        }
        return Ok(());
    }
    Err(UiaError {
        message: "element does not support the value pattern".to_string(),
        dead_element: false,
    })
}

/// The cache request used by `children()`, mirroring `CacheRequestFactory`.
pub fn cached_children_request(scope: i32) -> UiaResult<IUIAutomationCacheRequest> {
    let request = unsafe {
        automation_client()?
            .CreateCacheRequest()
            .map_err(map_err("CreateCacheRequest"))?
    };
    unsafe {
        request
            .SetTreeScope(TreeScope(scope))
            .map_err(map_err("SetTreeScope"))?;
        for property in cached_properties() {
            request
                .AddProperty(property)
                .map_err(map_err("AddProperty"))?;
        }
        for pattern in [
            UIA_LegacyIAccessiblePatternId,
            UIA_ScrollPatternId,
            UIA_WindowPatternId,
        ] {
            request.AddPattern(pattern).map_err(map_err("AddPattern"))?;
        }
    }
    Ok(request)
}

fn cached_properties() -> [windows::Win32::UI::Accessibility::UIA_PROPERTY_ID; 35] {
    [
        UIA_NamePropertyId,
        UIA_AutomationIdPropertyId,
        UIA_LocalizedControlTypePropertyId,
        UIA_AcceleratorKeyPropertyId,
        UIA_ClassNamePropertyId,
        UIA_ControlTypePropertyId,
        UIA_IsEnabledPropertyId,
        UIA_IsOffscreenPropertyId,
        UIA_IsControlElementPropertyId,
        UIA_HasKeyboardFocusPropertyId,
        UIA_IsKeyboardFocusablePropertyId,
        UIA_IsPasswordPropertyId,
        UIA_BoundingRectanglePropertyId,
        UIA_HelpTextPropertyId,
        UIA_LegacyIAccessibleRolePropertyId,
        UIA_LegacyIAccessibleValuePropertyId,
        UIA_LegacyIAccessibleDefaultActionPropertyId,
        UIA_LegacyIAccessibleStatePropertyId,
        UIA_ScrollHorizontallyScrollablePropertyId,
        UIA_ScrollVerticallyScrollablePropertyId,
        UIA_ScrollHorizontalScrollPercentPropertyId,
        UIA_ScrollVerticalScrollPercentPropertyId,
        UIA_ExpandCollapseExpandCollapseStatePropertyId,
        UIA_SelectionCanSelectMultiplePropertyId,
        UIA_SelectionIsSelectionRequiredPropertyId,
        UIA_SelectionSelectionPropertyId,
        UIA_SelectionItemIsSelectedPropertyId,
        UIA_SelectionItemSelectionContainerPropertyId,
        UIA_WindowIsModalPropertyId,
        UIA_ToggleToggleStatePropertyId,
        UIA_RangeValueValuePropertyId,
        UIA_RangeValueMinimumPropertyId,
        UIA_RangeValueMaximumPropertyId,
        UIA_ProcessIdPropertyId,
        UIA_NativeWindowHandlePropertyId,
    ]
}

/// Raw tree-scope constants used by callers.
pub const SCOPE_ELEMENT: i32 = TreeScope_Element.0;
pub const SCOPE_CHILDREN: i32 = TreeScope_Children.0;
pub const SCOPE_SUBTREE: i32 = windows::Win32::UI::Accessibility::TreeScope_Subtree.0;

/// Reads a cached property as an i32 (`None` when absent or of another type).
pub fn cached_property_i32(element: &ElementHandle, property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID) -> Option<i32> {
    unsafe { element.element.GetCachedPropertyValue(property) }
        .ok()
        .and_then(|value| variant_to_i32(&value))
}

/// Reads a cached property as a bool.
pub fn cached_property_bool(
    element: &ElementHandle,
    property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
) -> Option<bool> {
    unsafe { element.element.GetCachedPropertyValue(property) }
        .ok()
        .map(|value| variant_to_bool(&value))
}

/// Reads a cached property as a string.
pub fn cached_property_string(
    element: &ElementHandle,
    property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
) -> Option<String> {
    unsafe { element.element.GetCachedPropertyValue(property) }
        .ok()
        .map(|value| variant_to_string(&value))
}

/// Reads a cached property as an f64.
pub fn cached_property_f64(
    element: &ElementHandle,
    property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
) -> Option<f64> {
    unsafe { element.element.GetCachedPropertyValue(property) }
        .ok()
        .and_then(|value| <&windows::core::VARIANT as TryInto<f64>>::try_into(&value).ok())
}

/// Reads an element from a cached property holding an element array.
pub fn cached_property_elements(
    element: &ElementHandle,
    property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
) -> Vec<ElementHandle> {
    use windows::core::Interface;
    use windows::Win32::UI::Accessibility::IUIAutomationElementArray;
    let Ok(value) = (unsafe { element.element.GetCachedPropertyValue(property) }) else {
        return Vec::new();
    };
    let Ok(unknown) = <&windows::core::VARIANT as TryInto<windows::core::IUnknown>>::try_into(&value) else {
        return Vec::new();
    };
    let Ok(array) = unknown.cast::<IUIAutomationElementArray>() else {
        return Vec::new();
    };
    let length = unsafe { array.Length() }.unwrap_or(0);
    let mut elements = Vec::new();
    for index in 0..length {
        if let Ok(element) = unsafe { array.GetElement(index) } {
            elements.push(ElementHandle { element });
        }
    }
    elements
}
