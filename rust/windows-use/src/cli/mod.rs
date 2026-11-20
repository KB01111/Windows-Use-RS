//! CLI support modules, mirroring `windows_use/cli/`.
//!
//! [`config`] stores the provider configuration with Fernet-encrypted API keys,
//! [`registry`] holds the LLM provider/model tables and [`speech_registry`] the
//! STT/TTS tables used by the setup wizard.

pub mod config;
pub mod main;
pub mod registry;
pub mod setup;
pub mod speech_registry;
pub mod subscriber;

pub use config::{
    config_dir, config_file, decrypt_secret, encrypt_secret, get_active_config, get_api_key,
    get_providers_config, get_speech_config, get_stt_config, get_tts_config, is_configured,
    save_providers_config, save_speech_config, speech_config_file, update_provider_api_key,
    update_provider_base_url, upsert_provider,
};
pub use main::{Cli, run};
pub use registry::{
    for_provider, get_models, get_provider_display, get_providers, provider_requires_api_key,
    PROVIDERS, PROVIDERS_REQUIRING_API_KEY, PROVIDER_DISPLAY,
};
pub use speech_registry::{
    get_stt_models, get_stt_providers, get_tts_models, get_tts_providers,
    speech_provider_requires_api_key, GROQ_TTS_VOICES, OPENAI_TTS_VOICES,
    SPEECH_PROVIDERS_OWN_KEY, SPEECH_PROVIDERS_SHARING_LLM_KEY, STT_MODELS, STT_PROVIDERS,
    TTS_MODELS, TTS_PROVIDERS,
};
pub use subscriber::{strip_markdown, truncate, CliEventSubscriber};
