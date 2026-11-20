//! CLI speech provider and model registry, mirroring
//! `windows_use/cli/speech_registry.py`.
//!
//! Providers that share an API key with the LLM config (`openai`, `google`, `groq`)
//! can reuse the key from the main config; speech-only providers (`elevenlabs`,
//! `deepgram`) need their own. Tables are transcribed verbatim from the Python
//! module, including order and display names.

/// `(display_name, provider_key)` pairs for speech-to-text providers.
pub const STT_PROVIDERS: &[(&str, &str)] = &[
    ("OpenAI (Whisper)", "openai"),
    ("Google (Gemini)", "google"),
    ("Groq (Whisper)", "groq"),
    ("ElevenLabs (Scribe)", "elevenlabs"),
    ("Deepgram (Nova)", "deepgram"),
];

/// `(display_name, provider_key)` pairs for text-to-speech providers.
pub const TTS_PROVIDERS: &[(&str, &str)] = &[
    ("OpenAI", "openai"),
    ("Google (Gemini)", "google"),
    ("Groq (Orpheus)", "groq"),
    ("ElevenLabs", "elevenlabs"),
    ("Deepgram (Aura)", "deepgram"),
];

const OPENAI_STT_MODELS: &[(&str, &str)] = &[("Whisper 1 (recommended)", "whisper-1")];

const GOOGLE_STT_MODELS: &[(&str, &str)] = &[
    ("Gemini 2.5 Flash", "gemini-2.5-flash"),
    ("Gemini 2.0 Flash", "gemini-2.0-flash"),
];

const GROQ_STT_MODELS: &[(&str, &str)] = &[
    ("Whisper Large V3 Turbo", "whisper-large-v3-turbo"),
    ("Whisper Large V3", "whisper-large-v3"),
];

const ELEVENLABS_STT_MODELS: &[(&str, &str)] = &[("Scribe v2 (recommended)", "scribe_v2")];

const DEEPGRAM_STT_MODELS: &[(&str, &str)] = &[
    ("Nova 2 (recommended)", "nova-2"),
    ("Nova 2 General", "nova-2-general"),
    ("Nova 2 Meeting", "nova-2-meeting"),
];

const OPENAI_TTS_MODELS: &[(&str, &str)] = &[
    ("TTS 1 (recommended)", "tts-1"),
    ("TTS 1 HD", "tts-1-hd"),
];

const GOOGLE_TTS_MODELS: &[(&str, &str)] = &[
    ("Gemini 2.5 Flash TTS", "gemini-2.5-flash-preview-tts"),
    ("Gemini 2.5 Pro TTS", "gemini-2.5-pro-preview-tts"),
];

const GROQ_TTS_MODELS: &[(&str, &str)] = &[("Orpheus English", "canopylabs/orpheus-v1-english")];

const ELEVENLABS_TTS_MODELS: &[(&str, &str)] = &[
    ("Multilingual V2", "eleven_multilingual_v2"),
    ("Turbo V2.5", "eleven_turbo_v2_5"),
];

const DEEPGRAM_TTS_MODELS: &[(&str, &str)] = &[
    ("Aura 2 Thalia", "aura-2-thalia-en"),
    ("Aura 2 Luna", "aura-2-luna-en"),
    ("Aura 2 Stella", "aura-2-stella-en"),
];

/// `provider_key -> STT models`, mirroring the Python `STT_MODELS` dict.
pub const STT_MODELS: &[(&str, &[(&str, &str)])] = &[
    ("openai", OPENAI_STT_MODELS),
    ("google", GOOGLE_STT_MODELS),
    ("groq", GROQ_STT_MODELS),
    ("elevenlabs", ELEVENLABS_STT_MODELS),
    ("deepgram", DEEPGRAM_STT_MODELS),
];

/// `provider_key -> TTS models`, mirroring the Python `TTS_MODELS` dict.
pub const TTS_MODELS: &[(&str, &[(&str, &str)])] = &[
    ("openai", OPENAI_TTS_MODELS),
    ("google", GOOGLE_TTS_MODELS),
    ("groq", GROQ_TTS_MODELS),
    ("elevenlabs", ELEVENLABS_TTS_MODELS),
    ("deepgram", DEEPGRAM_TTS_MODELS),
];

/// Voices accepted by the OpenAI TTS models.
pub const OPENAI_TTS_VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "coral", "echo", "fable", "onyx", "nova", "sage", "shimmer",
];

/// Voices accepted by the Groq Orpheus TTS model.
pub const GROQ_TTS_VOICES: &[&str] = &["autumn", "diana", "hannah", "austin", "daniel", "troy"];

/// Speech providers that reuse the main LLM config's API key.
pub const SPEECH_PROVIDERS_SHARING_LLM_KEY: &[&str] = &["openai", "google", "groq"];

/// Speech-only providers that require their own API key.
pub const SPEECH_PROVIDERS_OWN_KEY: &[&str] = &["elevenlabs", "deepgram"];

/// Returns `(display_name, provider_key)` pairs for STT, in wizard order.
pub fn get_stt_providers() -> &'static [(&'static str, &'static str)] {
    STT_PROVIDERS
}

/// Returns `(display_name, provider_key)` pairs for TTS, in wizard order.
pub fn get_tts_providers() -> &'static [(&'static str, &'static str)] {
    TTS_PROVIDERS
}

/// Returns the `(display_name, model_id)` list for an STT provider.
///
/// Unknown providers fall back to `[(provider_key, provider_key)]`, matching the
/// Python `STT_MODELS.get(provider_key, [(provider_key, provider_key)])`.
pub fn get_stt_models(provider_key: &str) -> Vec<(&str, &str)> {
    match stt_models(provider_key) {
        Some(models) => models.to_vec(),
        None => vec![(provider_key, provider_key)],
    }
}

/// Returns the `(display_name, model_id)` list for a TTS provider.
pub fn get_tts_models(provider_key: &str) -> Vec<(&str, &str)> {
    match tts_models(provider_key) {
        Some(models) => models.to_vec(),
        None => vec![(provider_key, provider_key)],
    }
}

/// Returns `true` when the speech provider needs an API key.
///
/// True for both the providers sharing the LLM key and the speech-only ones, which
/// is exactly the Python `speech_provider_requires_api_key` rule.
pub fn speech_provider_requires_api_key(provider_key: &str) -> bool {
    SPEECH_PROVIDERS_SHARING_LLM_KEY.contains(&provider_key)
        || SPEECH_PROVIDERS_OWN_KEY.contains(&provider_key)
}

/// Looks up the STT model table without the Python fallback.
pub fn stt_models(provider_key: &str) -> Option<&'static [(&'static str, &'static str)]> {
    STT_MODELS
        .iter()
        .find(|(key, _)| *key == provider_key)
        .map(|(_, models)| *models)
}

/// Looks up the TTS model table without the Python fallback.
pub fn tts_models(provider_key: &str) -> Option<&'static [(&'static str, &'static str)]> {
    TTS_MODELS
        .iter()
        .find(|(key, _)| *key == provider_key)
        .map(|(_, models)| *models)
}

/// The voices valid for a provider's TTS model, if the provider has a fixed set.
pub fn tts_voices(provider_key: &str) -> Option<&'static [&'static str]> {
    match provider_key {
        "openai" => Some(OPENAI_TTS_VOICES),
        "groq" => Some(GROQ_TTS_VOICES),
        _ => None,
    }
}

/// Whether the provider reuses the API key stored for the matching LLM provider.
pub fn shares_llm_key(provider_key: &str) -> bool {
    SPEECH_PROVIDERS_SHARING_LLM_KEY.contains(&provider_key)
}

/// Whether the provider stores its own API key in the speech config.
pub fn requires_own_key(provider_key: &str) -> bool {
    SPEECH_PROVIDERS_OWN_KEY.contains(&provider_key)
}

/// The default STT model for a provider, when the table is non-empty.
pub fn default_stt_model(provider_key: &str) -> Option<&'static str> {
    stt_models(provider_key).and_then(|models| models.first().map(|(_, id)| *id))
}

/// The default TTS model for a provider, when the table is non-empty.
pub fn default_tts_model(provider_key: &str) -> Option<&'static str> {
    tts_models(provider_key).and_then(|models| models.first().map(|(_, id)| *id))
}

/// The default TTS voice for a provider, when it advertises one.
pub fn default_tts_voice(provider_key: &str) -> Option<&'static str> {
    tts_voices(provider_key).and_then(|voices| voices.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_providers_match_the_python_order() {
        assert_eq!(
            get_stt_providers(),
            &[
                ("OpenAI (Whisper)", "openai"),
                ("Google (Gemini)", "google"),
                ("Groq (Whisper)", "groq"),
                ("ElevenLabs (Scribe)", "elevenlabs"),
                ("Deepgram (Nova)", "deepgram"),
            ]
        );
        assert_eq!(get_stt_providers().len(), 5);
    }

    #[test]
    fn tts_providers_match_the_python_order() {
        assert_eq!(
            get_tts_providers(),
            &[
                ("OpenAI", "openai"),
                ("Google (Gemini)", "google"),
                ("Groq (Orpheus)", "groq"),
                ("ElevenLabs", "elevenlabs"),
                ("Deepgram (Aura)", "deepgram"),
            ]
        );
        assert_eq!(get_tts_providers().len(), 5);
    }

    #[test]
    fn stt_model_tables_match_the_python_entries() {
        assert_eq!(
            stt_models("openai"),
            Some(&[("Whisper 1 (recommended)", "whisper-1")][..])
        );
        assert_eq!(
            stt_models("google"),
            Some(
                &[
                    ("Gemini 2.5 Flash", "gemini-2.5-flash"),
                    ("Gemini 2.0 Flash", "gemini-2.0-flash"),
                ][..]
            )
        );
        assert_eq!(
            stt_models("groq"),
            Some(
                &[
                    ("Whisper Large V3 Turbo", "whisper-large-v3-turbo"),
                    ("Whisper Large V3", "whisper-large-v3"),
                ][..]
            )
        );
        assert_eq!(
            stt_models("elevenlabs"),
            Some(&[("Scribe v2 (recommended)", "scribe_v2")][..])
        );
        assert_eq!(
            stt_models("deepgram"),
            Some(
                &[
                    ("Nova 2 (recommended)", "nova-2"),
                    ("Nova 2 General", "nova-2-general"),
                    ("Nova 2 Meeting", "nova-2-meeting"),
                ][..]
            )
        );
    }

    #[test]
    fn tts_model_tables_match_the_python_entries() {
        assert_eq!(
            tts_models("openai"),
            Some(&[("TTS 1 (recommended)", "tts-1"), ("TTS 1 HD", "tts-1-hd")][..])
        );
        assert_eq!(
            tts_models("google"),
            Some(
                &[
                    ("Gemini 2.5 Flash TTS", "gemini-2.5-flash-preview-tts"),
                    ("Gemini 2.5 Pro TTS", "gemini-2.5-pro-preview-tts"),
                ][..]
            )
        );
        assert_eq!(
            tts_models("groq"),
            Some(&[("Orpheus English", "canopylabs/orpheus-v1-english")][..])
        );
        assert_eq!(
            tts_models("elevenlabs"),
            Some(
                &[
                    ("Multilingual V2", "eleven_multilingual_v2"),
                    ("Turbo V2.5", "eleven_turbo_v2_5"),
                ][..]
            )
        );
        assert_eq!(
            tts_models("deepgram"),
            Some(
                &[
                    ("Aura 2 Thalia", "aura-2-thalia-en"),
                    ("Aura 2 Luna", "aura-2-luna-en"),
                    ("Aura 2 Stella", "aura-2-stella-en"),
                ][..]
            )
        );
    }

    #[test]
    fn voice_tables_match_the_python_entries() {
        assert_eq!(
            OPENAI_TTS_VOICES,
            &["alloy", "ash", "ballad", "coral", "echo", "fable", "onyx", "nova", "sage", "shimmer"]
        );
        assert_eq!(OPENAI_TTS_VOICES.len(), 10);
        assert_eq!(
            GROQ_TTS_VOICES,
            &["autumn", "diana", "hannah", "austin", "daniel", "troy"]
        );
        assert_eq!(GROQ_TTS_VOICES.len(), 6);
    }

    #[test]
    fn key_ownership_sets_match_python() {
        assert_eq!(SPEECH_PROVIDERS_SHARING_LLM_KEY, &["openai", "google", "groq"]);
        assert_eq!(SPEECH_PROVIDERS_OWN_KEY, &["elevenlabs", "deepgram"]);

        assert!(shares_llm_key("openai"));
        assert!(shares_llm_key("google"));
        assert!(shares_llm_key("groq"));
        assert!(!shares_llm_key("elevenlabs"));

        assert!(requires_own_key("elevenlabs"));
        assert!(requires_own_key("deepgram"));
        assert!(!requires_own_key("openai"));

        for (_, key) in get_stt_providers() {
            assert!(speech_provider_requires_api_key(key), "{key} should need a key");
        }
        assert!(!speech_provider_requires_api_key("unknown"));
    }

    #[test]
    fn every_speech_provider_has_stt_and_tts_models() {
        for (_, key) in get_stt_providers() {
            assert!(stt_models(key).is_some(), "missing STT models for {key}");
            assert!(!get_stt_models(key).is_empty());
        }
        for (_, key) in get_tts_providers() {
            assert!(tts_models(key).is_some(), "missing TTS models for {key}");
            assert!(!get_tts_models(key).is_empty());
        }
    }

    #[test]
    fn stt_and_tts_providers_cover_the_same_keys() {
        let mut stt: Vec<&str> = get_stt_providers().iter().map(|(_, key)| *key).collect();
        let mut tts: Vec<&str> = get_tts_providers().iter().map(|(_, key)| *key).collect();
        stt.sort_unstable();
        tts.sort_unstable();
        assert_eq!(stt, tts);
    }

    #[test]
    fn unknown_speech_provider_falls_back_to_the_key() {
        assert_eq!(stt_models("nope"), None);
        assert_eq!(tts_models("nope"), None);
        assert_eq!(get_stt_models("nope"), vec![("nope", "nope")]);
        assert_eq!(get_tts_models("nope"), vec![("nope", "nope")]);
        assert_eq!(tts_voices("nope"), None);
        assert_eq!(default_stt_model("nope"), None);
        assert_eq!(default_tts_voice("nope"), None);
    }

    #[test]
    fn defaults_are_the_first_table_entry() {
        assert_eq!(default_stt_model("openai"), Some("whisper-1"));
        assert_eq!(default_stt_model("deepgram"), Some("nova-2"));
        assert_eq!(default_tts_model("openai"), Some("tts-1"));
        assert_eq!(default_tts_model("groq"), Some("canopylabs/orpheus-v1-english"));
        assert_eq!(default_tts_voice("openai"), Some("alloy"));
        assert_eq!(default_tts_voice("groq"), Some("autumn"));
        assert_eq!(default_tts_voice("google"), None);
    }

    #[test]
    fn model_ids_are_unique_within_each_table() {
        for (key, table) in STT_MODELS {
            let mut ids: Vec<&str> = table.iter().map(|(_, id)| *id).collect();
            ids.sort_unstable();
            let count = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), count, "duplicate STT model id for {key}");
        }
        for (key, table) in TTS_MODELS {
            let mut ids: Vec<&str> = table.iter().map(|(_, id)| *id).collect();
            ids.sort_unstable();
            let count = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), count, "duplicate TTS model id for {key}");
        }
    }

    #[test]
    fn model_tables_cover_every_provider_key() {
        let mut stt_keys: Vec<&str> = STT_MODELS.iter().map(|(key, _)| *key).collect();
        let mut tts_keys: Vec<&str> = TTS_MODELS.iter().map(|(key, _)| *key).collect();
        stt_keys.sort_unstable();
        tts_keys.sort_unstable();
        assert_eq!(stt_keys, vec!["deepgram", "elevenlabs", "google", "groq", "openai"]);
        assert_eq!(tts_keys, vec!["deepgram", "elevenlabs", "google", "groq", "openai"]);
    }
}
