//! Desktop state, windows and virtual desktop access.

pub mod service;
pub mod views;

pub use service::{
    best_match, key_alias, ratio, AppMode, AppRequest, AutoMinimizeGuard, Desktop,
    InteractiveElement, AVOIDED_APPS, EXCLUDED_APPS,
};
pub use views::{
    tabulate_simple, Browser, DesktopState, Size, Status, VirtualDesktop, Window,
};
