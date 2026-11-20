//! Speech services, mirroring `windows_use/speech/`.
//!
//! [`Stt`] records from the default microphone and hands the audio to a
//! speech-to-text provider; [`Tts`] synthesizes speech and plays it back.
//! Providers live in [`crate::providers`] and are used through the
//! [`crate::providers::base::SttProvider`] / [`crate::providers::base::TtsProvider`]
//! traits.

pub mod service;

pub use service::{Stt, Tts, STT, TTS};
