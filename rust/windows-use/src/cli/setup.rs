//! Interactive setup wizard, mirroring `windows_use/cli/setup.py`.

use std::io::{BufRead, Write};

use serde_json::Value;

use crate::cli::config::{
    get_active_config, get_api_key, get_stt_config, get_tts_config, save_speech_config,
    upsert_provider,
};
use crate::cli::registry::{
    get_models, get_provider_display, get_providers, provider_requires_api_key,
};
use crate::cli::speech_registry::{
    get_stt_models, get_stt_providers, get_tts_models, get_tts_providers,
    speech_provider_requires_api_key,
};
use crate::providers::{create_provider, ProviderConfig};

/// Reads one line from stdin, returning `None` on EOF.
pub fn read_line(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut buffer = String::new();
    match std::io::stdin().lock().read_line(&mut buffer) {
        Ok(0) => None,
        Ok(_) => Some(buffer.trim().to_string()),
        Err(_) => None,
    }
}

/// Reads a secret without echoing it.
pub fn read_secret(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    match rpassword::read_password() {
        Ok(value) => Some(value.trim().to_string()),
        Err(_) => {
            let mut buffer = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut buffer)
                .ok()
                .map(|_| buffer.trim().to_string())
        }
    }
}

/// Renders a numbered choice list and returns the selected zero-based index.
pub fn choose(prompt: &str, options: &[(&str, &str)], default_index: usize) -> Option<usize> {
    println!("\n{prompt}");
    for (index, (display, _)) in options.iter().enumerate() {
        let marker = if index == default_index { " (default)" } else { "" };
        println!("  {}. {display}{marker}", index + 1);
    }
    let input = read_line(&format!("Select [1-{}]: ", options.len()))?;
    if input.is_empty() {
        return Some(default_index);
    }
    input
        .parse::<usize>()
        .ok()
        .filter(|choice| *choice >= 1 && *choice <= options.len())
        .map(|choice| choice - 1)
}

/// Builds a provider from a stored config, falling back to environment variables.
pub fn create_llm(provider: &str, model: &str, base_url: Option<&str>) -> anyhow::Result<Box<dyn crate::providers::ChatLlm>> {
    let active = get_active_config();
    let api_key = active
        .as_ref()
        .filter(|config| config.get("provider").and_then(Value::as_str) == Some(provider))
        .and_then(|config| config.get("api_key").and_then(Value::as_str).map(str::to_string))
        .or_else(|| get_api_key(provider));

    let resolved_base_url = base_url
        .map(str::to_string)
        .or_else(|| {
            active
                .as_ref()
                .and_then(|config| config.get("base_url").and_then(Value::as_str))
                .map(str::to_string)
        });

    let config = ProviderConfig {
        provider: provider.to_string(),
        model: Some(model.to_string()),
        api_key,
        base_url: resolved_base_url,
        timeout: None,
        temperature: None,
        api_version: std::env::var("AZURE_OPENAI_API_VERSION").ok(),
    };
    create_provider(&config)
}

/// Runs the first-time setup wizard, returning the chosen provider and model.
pub fn run_setup() -> Option<(String, String)> {
    println!("\nWindows-Use setup");
    println!("Configure an LLM provider to get started.\n");

    let providers = get_providers();
    let index = choose("Choose a provider:", providers, 0)?;
    let (display, key) = providers[index];

    let models = get_models(key);
    let model_index = choose("Choose a model:", &models, 0)?;
    let (_, model) = models[model_index];

    let mut api_key = None;
    if provider_requires_api_key(key) {
        let entered = read_secret(&format!("Enter your {display} API key: "))?;
        if entered.is_empty() {
            println!("No API key entered; set it through the provider's environment variable instead.");
        } else {
            api_key = Some(entered);
        }
    }

    let base_url = read_line("Custom base URL (leave blank for default): ")
        .filter(|value| !value.is_empty());

    if let Err(error) = upsert_provider(
        key,
        model,
        api_key.as_deref(),
        base_url.as_deref(),
        true,
    ) {
        println!("Could not save the configuration: {error}");
        return None;
    }

    println!("\nConfigured {display} with model {model}.");
    Some((key.to_string(), model.to_string()))
}

/// Switches provider and model interactively.
pub fn run_llm_switch() -> Option<(String, String)> {
    let providers = get_providers();
    let index = choose("Choose a provider:", providers, 0)?;
    let (display, key) = providers[index];

    let models = get_models(key);
    let model_index = choose("Choose a model:", &models, 0)?;
    let (_, model) = models[model_index];

    let mut api_key = None;
    if provider_requires_api_key(key) {
        let entered = read_secret(&format!("Enter your {display} API key (blank to keep current): "))?;
        if !entered.is_empty() {
            api_key = Some(entered);
        }
    }

    if let Err(error) = upsert_provider(key, model, api_key.as_deref(), None, true) {
        println!("Could not save the configuration: {error}");
        return None;
    }
    Some((key.to_string(), model.to_string()))
}

/// Updates the API key of the active provider.
pub fn run_key_change() -> bool {
    let Some(active) = get_active_config() else {
        return false;
    };
    let Some(provider) = active.get("provider").and_then(Value::as_str) else {
        return false;
    };
    let display = get_provider_display(provider);
    let Some(key) = read_secret(&format!("Enter the new {display} API key: ")) else {
        return false;
    };
    match crate::cli::config::update_provider_api_key(provider, &key) {
        Ok(()) => true,
        Err(error) => {
            println!("Could not update the API key: {error}");
            false
        }
    }
}

/// Builds an STT provider from the saved speech config.
pub fn create_stt_provider() -> Option<Box<dyn crate::providers::SttProvider>> {
    let config = get_stt_config()?;
    let provider = config.get("provider")?.as_str()?.to_string();
    let model = config
        .get("llm")
        .or_else(|| config.get("model"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let api_key = config
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| get_api_key(&provider));
    crate::speech::service::create_stt(&provider, model, api_key)
}

/// Builds a TTS provider from the saved speech config.
pub fn create_tts_provider() -> Option<Box<dyn crate::providers::TtsProvider>> {
    let config = get_tts_config()?;
    let provider = config.get("provider")?.as_str()?.to_string();
    let model = config
        .get("llm")
        .or_else(|| config.get("model"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let voice = config.get("voice").and_then(Value::as_str).map(str::to_string);
    let api_key = config
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| get_api_key(&provider));
    crate::speech::service::create_tts(&provider, model, voice, api_key)
}

/// Configures speech-to-text and text-to-speech.
pub fn run_speech_setup(initial_setup: bool) {
    if initial_setup {
        println!("\nSpeech setup");
    }

    let stt_providers = get_stt_providers();
    if let Some(index) = choose("Choose a Speech-to-Text provider:", stt_providers, 0) {
        let (_, key) = stt_providers[index];
        let models = get_stt_models(key);
        if let Some(model_index) = choose("Choose an STT model:", &models, 0) {
            let (_, model) = models[model_index];
            let mut entry = serde_json::Map::new();
            entry.insert("provider".to_string(), Value::String(key.to_string()));
            entry.insert("llm".to_string(), Value::String(model.to_string()));
            entry.insert("enabled".to_string(), Value::Bool(true));
            if speech_provider_requires_api_key(key) {
                if let Some(secret) = read_secret("Enter the STT API key (blank to share the LLM key): ")
                {
                    if !secret.is_empty() {
                        entry.insert("api_key".to_string(), Value::String(secret));
                    }
                }
            }
            let _ = save_speech_config(Some(&Value::Object(entry)), None);
        }
    }

    let tts_providers = get_tts_providers();
    if let Some(index) = choose("Choose a Text-to-Speech provider:", tts_providers, 0) {
        let (_, key) = tts_providers[index];
        let models = get_tts_models(key);
        if let Some(model_index) = choose("Choose a TTS model:", &models, 0) {
            let (_, model) = models[model_index];
            let mut entry = serde_json::Map::new();
            entry.insert("provider".to_string(), Value::String(key.to_string()));
            entry.insert("llm".to_string(), Value::String(model.to_string()));
            entry.insert("enabled".to_string(), Value::Bool(true));
            if let Some(voice) = default_voice(key) {
                entry.insert("voice".to_string(), Value::String(voice));
            }
            if speech_provider_requires_api_key(key) {
                if let Some(secret) = read_secret("Enter the TTS API key (blank to share the LLM key): ")
                {
                    if !secret.is_empty() {
                        entry.insert("api_key".to_string(), Value::String(secret));
                    }
                }
            }
            let _ = save_speech_config(None, Some(&Value::Object(entry)));
        }
    }

    println!("Speech configuration saved.");
}

fn default_voice(provider: &str) -> Option<String> {
    match provider {
        "openai" => Some("alloy".to_string()),
        "groq" => Some("autumn".to_string()),
        "deepgram" => Some("aura-2-thalia-en".to_string()),
        _ => None,
    }
}
