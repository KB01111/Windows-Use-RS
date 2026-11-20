//! Message models exchanged with LLM providers.
//!
//! Mirrors `windows_use/messages/` from the Python project: a `Role` enum, the
//! `Usage` token accounting model and the `BaseMessage` variants that every
//! provider converts into its own wire format.

pub mod service;

pub use service::{
    compress_image, BaseMessage, CompressedImage, Role, Usage, DEFAULT_MIME_TYPE, MAX_IMAGE_BYTES,
    MIN_IMAGE_HEIGHT, MIN_IMAGE_WIDTH,
};
