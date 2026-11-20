//! CLI provider and model registry, mirroring `windows_use/cli/registry.py`.
//!
//! Single source of truth for LLM providers and their supported models. The tables
//! are transcribed verbatim from the Python module: same order, same display names
//! and the same model ids, because the setup wizard and the CLI flags depend on
//! both the ordering and the exact strings.

/// `(display_name, provider_key)` pairs; the order drives the setup wizard order.
pub const PROVIDERS: &[(&str, &str)] = &[
    ("Groq", "groq"),
    ("OpenAI", "openai"),
    ("Atlas Cloud", "atlas_cloud"),
    ("Cheaper Inference", "cheaper_inference"),
    ("Anthropic", "anthropic"),
    ("Google", "google"),
    ("Ollama", "ollama"),
    ("Mistral", "mistral"),
    ("Cerebras", "cerebras"),
    ("OpenRouter", "open_router"),
    ("Azure OpenAI", "azure_openai"),
    ("LiteLLM", "litellm"),
    ("DeepSeek", "deepseek"),
    ("NVIDIA", "nvidia"),
    ("Perplexity", "perplexity"),
];

const GROQ_MODELS: &[(&str, &str)] = &[
    ("Llama 4 Scout 17B (recommended)", "meta-llama/llama-4-scout-17b-16e-instruct"),
    ("Llama 3.3 70B", "llama-3.3-70b-versatile"),
    ("Llama 3.1 8B", "llama-3.1-8b-instant"),
    ("GPT OSS 120B", "openai/gpt-oss-120b"),
    ("GPT OSS 20B", "openai/gpt-oss-20b"),
    ("Qwen3 32B", "qwen/qwen3-32b"),
    ("Kimi K2", "moonshotai/kimi-k2-instruct-0905"),
    ("Compound (agentic)", "groq/compound"),
    ("Compound Mini", "groq/compound-mini"),
];

const OPENAI_MODELS: &[(&str, &str)] = &[
    ("GPT-5.2 (recommended)", "gpt-5.2"),
    ("GPT-5 mini", "gpt-5-mini"),
    ("GPT-5 nano", "gpt-5-nano"),
    ("GPT-5.2 pro", "gpt-5.2-pro"),
    ("GPT-5", "gpt-5"),
    ("GPT-4.1", "gpt-4.1"),
    ("GPT-4.1 mini", "gpt-4.1-mini"),
    ("o3", "o3"),
    ("o4-mini", "o4-mini"),
    ("o1", "o1"),
    ("GPT-4o", "gpt-4o"),
    ("GPT-4o mini", "gpt-4o-mini"),
    ("GPT-4 Turbo", "gpt-4-turbo"),
    ("GPT-3.5 Turbo", "gpt-3.5-turbo"),
];

const ATLAS_CLOUD_MODELS: &[(&str, &str)] = &[("OpenAI GPT-4.1 mini", "openai/gpt-4.1-mini")];

const CHEAPER_INFERENCE_MODELS: &[(&str, &str)] = &[
    ("GPT-5.4 mini (recommended)", "gpt-5.4-mini"),
    ("GPT-5.4", "gpt-5.4"),
    ("Claude Sonnet 5", "claude-sonnet-5"),
    ("DeepSeek V4 Flash", "deepseek-v4-flash"),
    ("GLM-5.3", "glm-5.3"),
];

const PERPLEXITY_MODELS: &[(&str, &str)] = &[
    ("GPT-5.4 (recommended)", "openai/gpt-5.4"),
    ("Gemini 3.1 Pro Preview", "google/gemini-3.1-pro-preview"),
    ("Gemini 3 Flash Preview", "google/gemini-3-flash-preview"),
    ("Claude Sonnet 4.6", "anthropic/claude-sonnet-4-6"),
    ("Claude Opus 4.6", "anthropic/claude-opus-4-6"),
    ("Perplexity Sonar", "perplexity/sonar"),
];

const ANTHROPIC_MODELS: &[(&str, &str)] = &[
    ("Claude Opus 4.6", "claude-opus-4-6"),
    ("Claude Opus 4.5", "claude-opus-4-5"),
    ("Claude Haiku 4.6", "claude-haiku-4-6"),
    ("Claude Sonnet 4.6", "claude-sonnet-4-6"),
    ("Claude Sonnet 4.5 (recommended)", "claude-sonnet-4-5"),
    ("Claude Opus 4", "claude-opus-4-20250514"),
    ("Claude 3.5 Sonnet", "claude-3-5-sonnet-latest"),
    ("Claude 3.5 Haiku", "claude-3-5-haiku-latest"),
    ("Claude 3 Haiku", "claude-3-haiku-latest"),
    ("Claude 3 Opus", "claude-3-opus-latest"),
];

const GOOGLE_MODELS: &[(&str, &str)] = &[
    ("Gemini 3.1 Pro Preview", "gemini-3.1-pro-preview"),
    ("Gemini 3 Flash Preview", "gemini-3-flash-preview"),
    ("Gemini 3 Pro Preview", "gemini-3-pro-preview"),
    ("Gemini 2.5 Flash", "gemini-2.5-flash"),
    ("Gemini 2.5 Pro", "gemini-2.5-pro"),
    ("Gemini 2.0 Flash", "gemini-2.0-flash"),
    ("Gemini 1.5 Pro", "gemini-1.5-pro"),
    ("Gemini 1.5 Flash", "gemini-1.5-flash"),
];

const OLLAMA_MODELS: &[(&str, &str)] = &[
    ("Llama 3.3 70B", "llama3.3"),
    ("Llama 3.2", "llama3.2"),
    ("Llama 3.1", "llama3.1"),
    ("Llama 3.1 70B", "llama3.1:70b"),
    ("DeepSeek R1", "deepseek-r1"),
    ("Gemma 3", "gemma3"),
    ("Qwen 3.5", "qwen3.5"),
    ("Mistral", "mistral"),
    ("Mixtral", "mixtral"),
    ("Codellama", "codellama"),
    ("Qwen2.5", "qwen2.5"),
    ("Phi3", "phi3"),
];

const MISTRAL_MODELS: &[(&str, &str)] = &[
    ("Mistral Large 3", "mistral-large-2512"),
    ("Mistral Medium 3.1", "mistral-medium-2508"),
    ("Mistral Small 3.2", "mistral-small-2506"),
    ("Ministral 3 14B", "ministral-3-14b-2512"),
    ("Ministral 3 8B", "ministral-3-8b-2512"),
    ("Devstral 2", "devstral-2-2512"),
    ("Magistral Medium 1.2", "magistral-medium-2509"),
    ("Codestral", "codestral-2508"),
    ("Mistral Large (legacy)", "mistral-large-latest"),
    ("Mistral Small (legacy)", "mistral-small-latest"),
];

const CEREBRAS_MODELS: &[(&str, &str)] = &[
    ("Llama 3.1 8B", "llama3.1-8b"),
    ("GPT OSS 120B", "gpt-oss-120b"),
    ("Llama 3.3 70B", "llama-3.3-70b"),
    ("Llama 3.1 70B", "llama-3.1-70b"),
    ("Qwen 3 235B (preview)", "qwen-3-235b-a22b-instruct-2507"),
    ("Z.ai GLM 4.7 (preview)", "zai-glm-4.7"),
];

const OPEN_ROUTER_MODELS: &[(&str, &str)] = &[
    ("Llama 4 Scout (recommended)", "meta-llama/llama-4-scout-17b-16e-instruct"),
    ("GPT-5.3 Codex", "openai/gpt-5.3-codex"),
    ("Claude Opus 4.6", "anthropic/claude-opus-4-6"),
    ("Claude 3.5 Sonnet", "anthropic/claude-3.5-sonnet"),
    ("GPT-4o", "openai/gpt-4o"),
    ("Gemini 3", "google/gemini-3"),
    ("Gemini 2.0 Flash", "google/gemini-2.0-flash"),
    ("DeepSeek V3.2", "deepseek/deepseek-v3.2"),
    ("Llama 3.3 70B", "meta-llama/llama-3.3-70b-instruct"),
];

const AZURE_OPENAI_MODELS: &[(&str, &str)] = &[
    ("GPT-5.2", "gpt-5.2"),
    ("GPT-5 mini", "gpt-5-mini"),
    ("GPT-4.1", "gpt-4.1"),
    ("o3", "o3"),
    ("o4-mini", "o4-mini"),
    ("GPT-4o", "gpt-4o"),
    ("GPT-4o mini", "gpt-4o-mini"),
    ("GPT-4 Turbo", "gpt-4-turbo"),
    ("o1", "o1"),
];

const LITELLM_MODELS: &[(&str, &str)] = &[
    ("OpenAI GPT-5.2", "openai/gpt-5.2"),
    ("Anthropic Claude Opus 4.6", "anthropic/claude-opus-4-6"),
    ("Anthropic Claude 3.5 Sonnet", "anthropic/claude-3-5-sonnet-latest"),
    ("OpenAI GPT-4o", "openai/gpt-4o"),
    ("Google Gemini 3", "gemini/gemini-3"),
    ("Google Gemini 2.0 Flash", "gemini/gemini-2.0-flash"),
    ("Groq Llama 3.3 70B", "groq/llama-3.3-70b-versatile"),
    ("Ollama Llama 3.1", "ollama/llama3.1"),
];

const DEEPSEEK_MODELS: &[(&str, &str)] = &[
    ("DeepSeek V3 (Chat)", "deepseek-chat"),
    ("DeepSeek R1 (Reasoner)", "deepseek-reasoner"),
];

const NVIDIA_MODELS: &[(&str, &str)] = &[
    ("Llama 3.3 70B", "meta/llama-3.3-70b-instruct"),
    ("Llama 3.1 405B", "meta/llama-3.1-405b-instruct"),
    ("Llama 3.1 8B", "meta/llama-3.1-8b-instruct"),
    ("Nemotron 4 340B", "nvidia/nemotron-4-340b-instruct"),
    ("Nemotron 3 8B", "nvidia/nemotron-3-8b-instruct"),
    ("DeepSeek R1", "deepseek-ai/deepseek-r1"),
    ("DeepSeek V3", "deepseek-ai/deepseek-v3"),
    ("Mistral Large", "mistralai/mistral-large"),
    ("Mixtral 8x22B", "mistralai/mixtral-8x22b-instruct-v0.1"),
    ("Gemma 2 9B", "google/gemma-2-9b-it"),
    ("Phi-3 Mini", "microsoft/phi-3-mini-128k-instruct"),
    ("Qwen 3.5 122B", "qwen/qwen3.5-122b-a10b"),
];

/// Returns the `(display_name, model_id)` table for a provider key.
///
/// Unknown keys return the Python fallback `[(provider_key, provider_key)]`, so the
/// model picker always offers at least one usable entry.
pub fn for_provider(key: &str) -> &'static [(&'static str, &'static str)] {
    match key {
        "groq" => GROQ_MODELS,
        "openai" => OPENAI_MODELS,
        "atlas_cloud" => ATLAS_CLOUD_MODELS,
        "cheaper_inference" => CHEAPER_INFERENCE_MODELS,
        "perplexity" => PERPLEXITY_MODELS,
        "anthropic" => ANTHROPIC_MODELS,
        "google" => GOOGLE_MODELS,
        "ollama" => OLLAMA_MODELS,
        "mistral" => MISTRAL_MODELS,
        "cerebras" => CEREBRAS_MODELS,
        "open_router" => OPEN_ROUTER_MODELS,
        "azure_openai" => AZURE_OPENAI_MODELS,
        "litellm" => LITELLM_MODELS,
        "deepseek" => DEEPSEEK_MODELS,
        "nvidia" => NVIDIA_MODELS,
        _ => &[],
    }
}

/// `provider_key -> models`, mirroring the Python `MODELS` dict.
///
/// Kept as a function because Rust cannot build a `HashMap` in a `const`; entries
/// and their order match the Python dictionary.
pub fn models() -> &'static [(&'static str, &'static [(&'static str, &'static str)])] {
    &[
        ("groq", GROQ_MODELS),
        ("openai", OPENAI_MODELS),
        ("atlas_cloud", ATLAS_CLOUD_MODELS),
        ("cheaper_inference", CHEAPER_INFERENCE_MODELS),
        ("perplexity", PERPLEXITY_MODELS),
        ("anthropic", ANTHROPIC_MODELS),
        ("google", GOOGLE_MODELS),
        ("ollama", OLLAMA_MODELS),
        ("mistral", MISTRAL_MODELS),
        ("cerebras", CEREBRAS_MODELS),
        ("open_router", OPEN_ROUTER_MODELS),
        ("azure_openai", AZURE_OPENAI_MODELS),
        ("litellm", LITELLM_MODELS),
        ("deepseek", DEEPSEEK_MODELS),
        ("nvidia", NVIDIA_MODELS),
    ]
}

/// Providers that require an API key (Ollama is local and needs none).
pub const PROVIDERS_REQUIRING_API_KEY: &[&str] = &[
    "groq",
    "openai",
    "atlas_cloud",
    "cheaper_inference",
    "anthropic",
    "google",
    "mistral",
    "cerebras",
    "open_router",
    "azure_openai",
    "litellm",
    "deepseek",
    "nvidia",
    "perplexity",
];

/// `provider_key -> display name`, mirroring the Python `PROVIDER_DISPLAY` dict.
pub const PROVIDER_DISPLAY: &[(&str, &str)] = &[
    ("groq", "Groq"),
    ("openai", "OpenAI"),
    ("atlas_cloud", "Atlas Cloud"),
    ("cheaper_inference", "Cheaper Inference"),
    ("anthropic", "Anthropic"),
    ("google", "Google"),
    ("ollama", "Ollama"),
    ("mistral", "Mistral"),
    ("cerebras", "Cerebras"),
    ("open_router", "OpenRouter"),
    ("azure_openai", "Azure OpenAI"),
    ("litellm", "LiteLLM"),
    ("deepseek", "DeepSeek"),
    ("nvidia", "NVIDIA"),
    ("perplexity", "Perplexity"),
];

/// Returns `true` when the provider needs an API key.
pub fn provider_requires_api_key(provider_key: &str) -> bool {
    PROVIDERS_REQUIRING_API_KEY.contains(&provider_key)
}

/// Returns the `(display_name, provider_key)` list, in wizard order.
pub fn get_providers() -> &'static [(&'static str, &'static str)] {
    PROVIDERS
}

/// Returns the `(display_name, model_id)` list for a provider.
///
/// Unknown providers fall back to `[(provider_key, provider_key)]`, matching
/// Python's `MODELS.get(provider_key, [(provider_key, provider_key)])`; that is why
/// the result borrows from `provider_key` and is owned rather than `'static`.
pub fn get_models(provider_key: &str) -> Vec<(&str, &str)> {
    let models = for_provider(provider_key);
    if models.is_empty() {
        return vec![(provider_key, provider_key)];
    }
    models.to_vec()
}

/// Returns the display name for a provider key.
///
/// Unknown keys fall back to the key title-cased with spaces, matching Python's
/// `provider_key.replace("_", " ").title()`.
pub fn get_provider_display(provider_key: &str) -> String {
    if let Some((_, display)) = PROVIDER_DISPLAY
        .iter()
        .find(|(key, _)| *key == provider_key)
    {
        return (*display).to_string();
    }
    python_title_case(&provider_key.replace('_', " "))
}

/// Reproduces Python's `str.title()` for ASCII identifiers.
///
/// `str.title()` upper-cases the first letter of every word and lower-cases the
/// rest, which differs from Rust's `to_uppercase` on each word: `"OPENAI"` becomes
/// `"Openai"` in Python and must do the same here.
fn python_title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_is_letter = false;
    for character in text.chars() {
        if character.is_alphabetic() {
            if previous_is_letter {
                out.extend(character.to_lowercase());
            } else {
                out.extend(character.to_uppercase());
            }
            previous_is_letter = true;
        } else {
            out.push(character);
            previous_is_letter = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_keep_the_python_order() {
        let keys: Vec<&str> = get_providers().iter().map(|(_, key)| *key).collect();
        assert_eq!(
            keys,
            vec![
                "groq",
                "openai",
                "atlas_cloud",
                "cheaper_inference",
                "anthropic",
                "google",
                "ollama",
                "mistral",
                "cerebras",
                "open_router",
                "azure_openai",
                "litellm",
                "deepseek",
                "nvidia",
                "perplexity",
            ]
        );
        assert_eq!(get_providers().len(), 15);
        assert_eq!(get_providers()[0], ("Groq", "groq"));
        assert_eq!(get_providers()[14], ("Perplexity", "perplexity"));
    }

    #[test]
    fn every_provider_key_has_a_model_table() {
        for (display, key) in get_providers() {
            let models = for_provider(key);
            assert!(!models.is_empty(), "{key} ({display}) has no models");
            assert_eq!(get_models(key).len(), models.len());
        }
    }

    #[test]
    fn model_tables_match_the_python_entries() {
        assert_eq!(
            for_provider("groq"),
            &[
                ("Llama 4 Scout 17B (recommended)", "meta-llama/llama-4-scout-17b-16e-instruct"),
                ("Llama 3.3 70B", "llama-3.3-70b-versatile"),
                ("Llama 3.1 8B", "llama-3.1-8b-instant"),
                ("GPT OSS 120B", "openai/gpt-oss-120b"),
                ("GPT OSS 20B", "openai/gpt-oss-20b"),
                ("Qwen3 32B", "qwen/qwen3-32b"),
                ("Kimi K2", "moonshotai/kimi-k2-instruct-0905"),
                ("Compound (agentic)", "groq/compound"),
                ("Compound Mini", "groq/compound-mini"),
            ]
        );
        assert_eq!(for_provider("openai").len(), 14);
        assert_eq!(for_provider("openai")[0], ("GPT-5.2 (recommended)", "gpt-5.2"));
        assert_eq!(for_provider("atlas_cloud"), &[("OpenAI GPT-4.1 mini", "openai/gpt-4.1-mini")]);
        assert_eq!(for_provider("cheaper_inference").len(), 5);
        assert_eq!(for_provider("perplexity").len(), 6);
        assert_eq!(for_provider("anthropic").len(), 10);
        assert_eq!(for_provider("google").len(), 8);
        assert_eq!(for_provider("ollama").len(), 12);
        assert_eq!(for_provider("mistral").len(), 10);
        assert_eq!(for_provider("cerebras").len(), 6);
        assert_eq!(for_provider("open_router").len(), 9);
        assert_eq!(for_provider("azure_openai").len(), 9);
        assert_eq!(for_provider("litellm").len(), 8);
        assert_eq!(for_provider("deepseek"), &[
            ("DeepSeek V3 (Chat)", "deepseek-chat"),
            ("DeepSeek R1 (Reasoner)", "deepseek-reasoner"),
        ]);
        assert_eq!(for_provider("nvidia").len(), 12);
    }

    #[test]
    fn models_listing_covers_every_provider() {
        let listed: Vec<&str> = models().iter().map(|(key, _)| *key).collect();
        assert_eq!(listed.len(), 15);
        for (_, key) in get_providers() {
            assert!(listed.contains(key), "{key} missing from models()");
        }
        for (key, table) in models() {
            assert_eq!(*table, for_provider(key), "models() disagrees for {key}");
        }
    }

    #[test]
    fn unknown_provider_falls_back_to_the_key() {
        assert!(for_provider("unknown_provider").is_empty());
        assert_eq!(
            get_models("unknown_provider"),
            vec![("unknown_provider", "unknown_provider")]
        );
        assert_eq!(get_provider_display("unknown_provider"), "Unknown Provider");
        assert_eq!(get_provider_display("my_custom_llm"), "My Custom Llm");
    }

    #[test]
    fn ollama_does_not_require_an_api_key() {
        assert!(!provider_requires_api_key("ollama"));
        assert!(!provider_requires_api_key("unknown"));
        for (_, key) in get_providers() {
            if *key == "ollama" {
                continue;
            }
            assert!(provider_requires_api_key(key), "{key} should require a key");
        }
        assert_eq!(PROVIDERS_REQUIRING_API_KEY.len(), 14);
    }

    #[test]
    fn provider_display_covers_every_provider() {
        for (display, key) in get_providers() {
            assert_eq!(get_provider_display(key), *display);
        }
        assert_eq!(PROVIDER_DISPLAY.len(), PROVIDERS.len());
        assert_eq!(get_provider_display("azure_openai"), "Azure OpenAI");
        assert_eq!(get_provider_display("litellm"), "LiteLLM");
        assert_eq!(get_provider_display("open_router"), "OpenRouter");
    }

    #[test]
    fn python_title_case_matches_str_title() {
        assert_eq!(python_title_case("open ai"), "Open Ai");
        assert_eq!(python_title_case("OPENAI"), "Openai");
        assert_eq!(python_title_case(""), "");
        assert_eq!(python_title_case("x"), "X");
        assert_eq!(python_title_case("my-custom provider"), "My-Custom Provider");
    }

    #[test]
    fn get_models_returns_owned_copies() {
        let first = get_models("deepseek");
        assert_eq!(first.len(), 2);
        let second = get_models("deepseek");
        assert_eq!(first, second);
    }

    #[test]
    fn no_duplicate_provider_keys() {
        let mut keys: Vec<&str> = get_providers().iter().map(|(_, key)| *key).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "duplicate provider key in PROVIDERS");
    }

    #[test]
    fn model_ids_are_unique_per_provider() {
        for (key, table) in models() {
            let mut ids: Vec<&str> = table.iter().map(|(_, id)| *id).collect();
            ids.sort_unstable();
            let count = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), count, "duplicate model id for {key}");
        }
    }
}
