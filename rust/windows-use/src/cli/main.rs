//! Interactive CLI, mirroring `windows_use/cli/__main__.py`.

use std::io::Write;
use std::sync::Arc;

use clap::Parser;

use crate::agent::context::AgentMode;
use crate::agent::service::Agent;
use crate::cli::config::get_active_config;
use crate::cli::registry::get_provider_display;
use crate::cli::setup::{
    create_llm, create_stt_provider, create_tts_provider, read_line, run_key_change,
    run_llm_switch, run_setup, run_speech_setup,
};
use crate::cli::subscriber::{strip_markdown, CliEventSubscriber};
use crate::desktop::views::Browser;

/// Command-line options matching the Typer definitions.
#[derive(Debug, Parser)]
#[command(
    name = "windows-use",
    about = "Windows-Use: Computer-Use for Windows OS.",
    version = crate::VERSION
)]
pub struct Cli {
    /// LLM model ID to use (overrides config). Supports both predefined and custom models.
    #[arg(short = 'm', long)]
    pub model: Option<String>,

    /// LLM provider to use (overrides config).
    #[arg(short = 'p', long)]
    pub provider: Option<String>,

    /// Maximum agent steps per task.
    #[arg(long, default_value_t = 200)]
    pub max_steps: usize,

    /// Enable debug logging (e.g. [STT]/[TTS] messages).
    #[arg(short = 'd', long)]
    pub debug: bool,

    /// Agent mode: flash (lightweight) or normal (full prompts).
    #[arg(long, default_value = "normal")]
    pub mode: String,

    /// Send screenshots to the LLM.
    #[arg(long, default_value_t = false)]
    pub use_vision: bool,

    /// Annotate UI elements on screenshots.
    #[arg(long, default_value_t = false)]
    pub use_annotation: bool,

    /// Run a single task and exit instead of entering the interactive loop.
    #[arg(long)]
    pub task: Option<String>,
}

/// Commands available when typing `\`.
pub const CLI_COMMANDS: [(&str, &str); 6] = [
    ("\\quit", "Exit the session"),
    ("\\llm", "Switch provider or model"),
    ("\\key", "Change API key for a provider"),
    ("\\speech", "Configure Speech-to-Text / Text-to-Speech"),
    ("\\voice", "Use voice input (Speech-to-Text)"),
    ("\\clear", "Clear the screen"),
];

/// Clears the terminal.
pub fn clear_screen() {
    print!("\x1B[2J\x1B[H");
    let _ = std::io::stdout().flush();
}

fn print_banner(provider_display: &str, model: &str, version: &str, has_stt: bool) {
    println!("Windows-Use v{version} | {provider_display}: {model}");
    let mut hint = "Type \\quit, \\exit, or \\q to leave.".to_string();
    if has_stt {
        hint.push_str(" \\voice for voice input.");
    }
    println!("{hint}\n");
}

/// Shows the animated `•○○` progress indicator the Python CLI uses.
fn with_spinner<T>(label: &str, action: impl FnOnce() -> T) -> T {
    use std::sync::atomic::{AtomicBool, Ordering};
    const FRAMES: [&str; 3] = ["•○○", "○•○", "○○•"];
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();
    let label = label.to_string();
    let handle = std::thread::spawn(move || {
        let mut index = 0usize;
        while !stop_clone.load(Ordering::Relaxed) {
            print!("\r> {} {label}", FRAMES[index % 3]);
            let _ = std::io::stdout().flush();
            index += 1;
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        print!("\r{}\r", " ".repeat(6 + label.len()));
        let _ = std::io::stdout().flush();
    });

    let result = action();
    stop.store(true, Ordering::Relaxed);
    let _ = handle.join();
    result
}

/// Resolves provider and model from config or CLI options.
fn resolve_provider_model(
    model_option: Option<String>,
    provider_option: Option<String>,
) -> Option<(String, String)> {
    if let (Some(provider), Some(model)) = (&provider_option, &model_option) {
        return Some((normalize_provider(provider), model.clone()));
    }

    let active = get_active_config();
    let active = match active {
        Some(active) => Some(active),
        None => run_setup().map(|(provider, model)| {
            serde_json::json!({ "provider": provider, "llm": model, "base_url": null })
        }),
    }?;

    let provider = provider_option
        .map(|provider| normalize_provider(&provider))
        .or_else(|| active.get("provider").and_then(|value| value.as_str()).map(str::to_string))?;
    let model = model_option.or_else(|| {
        active
            .get("llm")
            .and_then(|value| value.as_str())
            .map(str::to_string)
    })?;
    Some((provider, model))
}

/// Normalizes a provider display name to its config key.
pub fn normalize_provider(name: &str) -> String {
    let key = name.to_lowercase().replace(' ', "_");
    if crate::cli::registry::PROVIDER_DISPLAY
        .iter()
        .any(|(provider, _)| *provider == key)
    {
        key
    } else {
        name.to_string()
    }
}

/// Builds the TTS callback used when speech output is configured.
fn make_tts_callback(
    tts_provider: &Option<Box<dyn crate::providers::TtsProvider>>,
) -> Option<Box<dyn Fn(&str) + Send + Sync>> {
    let provider = tts_provider.as_ref()?;
    // The provider is re-created per invocation because `Tts` owns it mutably.
    let model = provider.model().to_string();
    let provider_name = tts_provider_name(provider);
    Some(Box::new(move |content: &str| {
        let Some(provider) = crate::providers::speech::tts_provider(
            &provider_name,
            Some(model.clone()),
            None,
            None,
        ) else {
            return;
        };
        let mut tts = crate::speech::service::Tts::new(Some(provider), false, 1024);
        let cleaned = strip_markdown(content);
        let generated = with_spinner("Synthesizing...", || tts.generate_audio(&cleaned));
        if generated.is_ok() {
            let _ = with_spinner("Speaking...", || tts.play_audio());
        }
        tts.close();
    }))
}

fn tts_provider_name(provider: &Box<dyn crate::providers::TtsProvider>) -> String {
    // The model prefix is the only stable identifier exposed by the trait; match on it.
    let model = provider.model().to_lowercase();
    if model.starts_with("tts-") {
        "openai".to_string()
    } else if model.contains("orpheus") {
        "groq".to_string()
    } else if model.contains("gemini") {
        "google".to_string()
    } else if model.starts_with("eleven") {
        "elevenlabs".to_string()
    } else if model.starts_with("aura") {
        "deepgram".to_string()
    } else {
        "openai".to_string()
    }
}

/// Runs the CLI.
pub fn run(cli: Cli) -> anyhow::Result<()> {
    let Some((provider, model)) = resolve_provider_model(cli.model.clone(), cli.provider.clone())
    else {
        println!("No provider configured. Run the setup wizard and try again.");
        return Ok(());
    };

    let mode = AgentMode::parse(&cli.mode).unwrap_or(AgentMode::Normal);
    let browser = Browser::Edge;
    let provider_display = get_provider_display(&provider);

    let mut stt_provider = create_stt_provider();
    let mut tts_provider = create_tts_provider();
    let mut stt = stt_provider
        .take()
        .map(|provider| crate::speech::service::Stt::new(Some(provider), false, 1024, 44100, 1));
    let has_stt = stt.is_some();

    clear_screen();
    print_banner(&provider_display, &model, crate::VERSION, has_stt);

    // Run a single task when requested, matching `agent.invoke(task=...)`.
    if let Some(task) = cli.task.clone() {
        let llm = create_llm(&provider, &model, None)?;
        let mut agent = build_agent(
            llm,
            mode,
            browser,
            cli.max_steps,
            cli.use_vision,
            cli.use_annotation,
            &tts_provider,
        )?;
        let result = {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(agent.invoke(&task))
        };
        if let Some(error) = result.error {
            eprintln!("Error: {error}");
        }
        return Ok(());
    }

    loop {
        let Some(input) = read_line("> ") else {
            break;
        };
        let task = input.trim().to_string();
        if task.is_empty() {
            continue;
        }

        match task.to_lowercase().as_str() {
            "quit" | "exit" | "q" | "\\quit" | "\\exit" | "\\q" => break,
            "clear" | "\\clear" => {
                clear_screen();
                print_banner(&provider_display, &model, crate::VERSION, stt.is_some());
                continue;
            }
            "key" | "\\key" => {
                if run_key_change() {
                    println!("API key updated and applied.");
                } else {
                    println!("No providers with API keys configured. Use \\llm first.");
                }
                continue;
            }
            "speech" | "\\speech" => {
                run_speech_setup(false);
                stt_provider = create_stt_provider();
                tts_provider = create_tts_provider();
                stt = stt_provider
                    .take()
                    .map(|provider| crate::speech::service::Stt::new(Some(provider), false, 1024, 44100, 1));
                continue;
            }
            "voice" | "\\voice" => {
                let Some(instance) = stt.as_mut() else {
                    println!("Speech-to-Text not configured. Use \\speech to set it up.");
                    continue;
                };
                let transcribed = with_spinner("Listening... (press enter to stop)", || {
                    instance.start_recording().map_err(|error| error.to_string())?;
                    let mut buffer = String::new();
                    let _ = std::io::stdin().read_line(&mut buffer);
                    instance
                        .stop_recording()
                        .map_err(|error| error.to_string())?;
                    let text = instance
                        .process_audio()
                        .map_err(|error| error.to_string())?;
                    instance.close();
                    Ok::<String, String>(text)
                });
                let transcribed = match transcribed {
                    Ok(text) => text,
                    Err(error) => {
                        println!("Voice input failed: {error}");
                        continue;
                    }
                };
                print!("\r> {transcribed}");
                let _ = std::io::stdout().flush();
                println!();
                continue;
            }
            "llm" | "\\llm" => continue,
            _ => {}
        }

        let Some((current_provider, current_model)) = resolve_provider_model(None, None) else {
            continue;
        };
        let llm = match create_llm(&current_provider, &current_model, None) {
            Ok(llm) => llm,
            Err(error) => {
                println!("Could not create the LLM client: {error}");
                continue;
            }
        };
        let mut agent = build_agent(
            llm,
            mode,
            browser,
            cli.max_steps,
            cli.use_vision,
            cli.use_annotation,
            &tts_provider,
        )?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let _ = runtime.block_on(agent.invoke(&task));
    }

    if let Some(instance) = stt.as_mut() {
        instance.close();
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_agent(
    llm: Box<dyn crate::providers::ChatLlm>,
    mode: AgentMode,
    browser: Browser,
    max_steps: usize,
    use_vision: bool,
    use_annotation: bool,
    tts_provider: &Option<Box<dyn crate::providers::TtsProvider>>,
) -> anyhow::Result<Agent> {
    let subscriber = CliEventSubscriber::new(make_tts_callback(tts_provider));
    let mut agent = Agent::new(
        Arc::from(llm),
        mode,
        browser,
        use_vision,
        use_annotation,
        true,
        false,
        max_steps,
        3,
        Vec::new(),
        false,
        false,
        false,
    )?;
    agent.add_subscriber(Box::new(subscriber));
    Ok(agent)
}

/// Entry point used by `main.rs`.
pub fn main() {
    let cli = Cli::parse();
    if cli.debug {
        std::env::set_var("RUST_LOG", "debug");
    }
    if let Err(error) = run(cli) {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

/// Runs the LLM switch command, exposed for the REPL.
pub fn switch_llm() -> Option<(String, String)> {
    match run_llm_switch() {
        Some(result) => {
            clear_screen();
            print_banner(
                &get_provider_display(&result.0),
                &result.1,
                crate::VERSION,
                false,
            );
            Some(result)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_names_normalize() {
        assert_eq!(normalize_provider("Groq"), "groq");
        assert_eq!(normalize_provider("Open Router"), "open_router");
        assert_eq!(normalize_provider("Azure OpenAI"), "azure_openai");
        // Display names that are not keys pass through unchanged.
        assert_eq!(normalize_provider("Custom"), "Custom");
    }

    #[test]
    fn cli_parses_short_flags() {
        let cli = Cli::try_parse_from(["windows-use", "-p", "openai", "-m", "gpt-4o", "--max-steps", "10"])
            .expect("parse");
        assert_eq!(cli.provider.as_deref(), Some("openai"));
        assert_eq!(cli.model.as_deref(), Some("gpt-4o"));
        assert_eq!(cli.max_steps, 10);
    }

    #[test]
    fn cli_defaults_match_python() {
        let cli = Cli::try_parse_from(["windows-use"]).expect("parse");
        assert_eq!(cli.max_steps, 200);
        assert_eq!(cli.mode, "normal");
        assert!(!cli.use_vision);
    }
}
