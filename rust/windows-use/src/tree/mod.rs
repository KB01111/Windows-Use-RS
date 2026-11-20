//! Accessibility tree capture.

pub mod config;
pub mod service;
pub mod views;

pub use config::{
    is_document_control_type, is_informative_control_type, is_interactive_control_type,
    is_interactive_role, DEFAULT_ACTIONS, INTERACTIVE_CONTROL_TYPE_NAMES, INTERACTIVE_ROLES,
    THREAD_MAX_RETRIES,
};
pub use service::{
    capitalize, capture_tree_state, get_nodes, iou_bounding_box, random_point_within_bounding_box,
    title_case, tree_traversal, window_name_correction, TraversalContext, TreeCapture, TreeOptions,
};
pub use views::{
    BoundingBox, Center, ScrollElementNode, SelectorMap, SelectorNode, TextElementNode,
    TreeElementNode, TreeState,
};
