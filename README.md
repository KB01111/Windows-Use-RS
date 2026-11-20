# Windows-Use-RS

A complete Rust rewrite of [Windows-Use](https://github.com/Jeomon/Windows-Use) v0.8.1 — the AI
agent that drives Windows at the GUI layer through the UI Automation API. No Python runtime, no
vendor SDKs, no COM wrapper layer.

Give it a task in plain English and it clicks, types, scrolls, drags, runs PowerShell, reads web
pages through the accessibility tree, and manages virtual desktops.

```powershell
cargo build --release --manifest-path rust/windows-use/Cargo.toml
.\rust\target\release\windows-use.exe
```

## What it can do

- Open, switch, resize and close application windows
- Click, type, scroll, drag, and send keyboard shortcuts
- Run PowerShell commands and read their output
- Scrape web pages via the browser accessibility tree
- Read and write files, and keep persistent memory across steps
- Create, rename, switch and remove Windows virtual desktops
- Speak and listen through STT/TTS providers
- Reason with 16 LLM providers — Anthropic, OpenAI, Google, Groq, Mistral, Cerebras, OpenRouter,
  Azure OpenAI, LiteLLM, DeepSeek, NVIDIA, Ollama, vLLM, Perplexity, Atlas Cloud,
  Cheaper Inference

## CLI

```
-m, --model           LLM model ID (overrides config)
-p, --provider        LLM provider (overrides config)
    --max-steps       Maximum agent steps per task [default: 200]
-d, --debug           Enable debug logging
    --mode            flash (lightweight) | normal (full prompts) [default: normal]
    --use-vision      Send screenshots to the LLM
    --use-annotation  Annotate UI elements on screenshots
    --task            Run a single task and exit
```

In-session commands: `\llm` switch provider or model, `\key` change the API key, `\speech`
configure STT/TTS, `\voice` record voice input, `\clear`, `\quit`.

## Library use

```rust
use std::sync::Arc;
use windows_use::agent::{Agent, AgentMode};
use windows_use::desktop::views::Browser;
use windows_use::providers::ProviderConfig;

let llm = windows_use::providers::create_provider(&ProviderConfig::new(
    "anthropic",
    "claude-sonnet-4-5",
))?;
let mut agent = Agent::new(
    Arc::from(llm),
    AgentMode::Normal,
    Browser::Edge,
    /* use_vision */ false,
    /* use_annotation */ false,
    /* use_accessibility */ true,
    /* auto_minimize */ false,
    /* max_steps */ 25,
    /* max_consecutive_failures */ 3,
    Vec::new(),
    /* log_to_console */ true,
    /* experimental */ false,
    /* disable_loop_detection */ false,
)?;

let result = agent.invoke_blocking("Open Notepad and write a short poem");
println!("{:?}", result.content);
```

## How it works

**UIA is called directly.** The Python original goes through `comtypes` and a generated typelib.
Here `IUIAutomation`, `IUIAutomationElement`, the walkers and every pattern interface come from
the [`windows`](https://crates.io/crates/windows) crate. The cache request used for traversal
carries the same 35 properties and 3 patterns the Python factory does, so element metadata
matches field for field.

**Virtual desktops use raw vtable calls.** Those interfaces are undocumented and their layouts
shift between builds. `src/vdm.rs` picks one of six layouts from `(build, UBR)` and calls slots by
index, which keeps create/switch/rename/remove working on Windows 10 and every Windows 11 release
through 24H2+.

**Providers speak HTTP.** Every provider is a `reqwest` client. The twelve OpenAI-compatible
providers share one engine; Anthropic, Gemini, Ollama and Perplexity have their own converters
(thinking blocks and signatures, `functionCall` parts, NDJSON tool calls, citations).

**Config is Fernet-compatible.** API keys written by either implementation can be read by the
other: PBKDF2-HMAC-SHA256 with 480 000 iterations over the home path, AES-128-CBC, HMAC-SHA256,
urlsafe base64 — the exact `cryptography.fernet` construction.

## Layout

```
rust/
└── windows-use/
    ├── src/
    │   ├── uia/            UI Automation (UIA3) through the `windows` crate
    │   ├── tree/           accessibility tree capture, filtering and rendering
    │   ├── desktop/        desktop state, windows, input, PowerShell, fuzzy matching
    │   ├── screenshot.rs   GDI screen capture + annotated screenshots
    │   ├── vdm.rs          virtual desktop COM interfaces
    │   ├── win32.rs        window/process/screen/registry helpers
    │   ├── agent/          agent loop, prompts, tool registry, loop guard
    │   ├── tools.rs        tool parameters + pydantic-compatible validation
    │   ├── events.rs       event emitter and subscribers
    │   ├── messages/       message models shared with providers
    │   ├── providers/      all LLM, STT and TTS providers
    │   ├── speech/         microphone capture and audio playback
    │   ├── telemetry/      PostHog telemetry
    │   └── cli/            clap CLI, setup wizard, encrypted config
    └── src/bin/            parity and verification binaries
tools/parity/               scripts that diff this port against the Python original
```

## Behavioural parity

Deterministic outputs were diffed against the Python original. `tools/parity/parity_dump.py`
renders twenty categories from the upstream modules using the real `tabulate`, `fuzzywuzzy`,
`markdownify` and pydantic; `src/bin/dump_parity.rs` renders the same twenty from Rust. The
comparison reports **no differences** for:

- interactive and scrollable element tables, including metadata JSON formatting
- window and virtual desktop tables (`tabulate(tablefmt="simple")`, right-aligned numerics)
- all 15 tool parameter schemas, generated from the pydantic models
- fuzzy scores (`ratio`, `partial_ratio`, `token_sort_ratio`, `token_set_ratio`, `WRatio`),
  including `full_process` spacing and case sensitivity
- SendKeys parsing (`{Ctrl}a` → Ctrl↓ A↓ A↑ Ctrl↑, `{Shift}(ab)`, `{a 3}`, `{{}` escapes)
- HTML→Markdown conversion (`===` h1 underlines, `*`/`+` list markers, hard breaks)
- prompt templating, markdown stripping, tool-name formatting, loop-guard warnings

To re-run it:

```powershell
git clone --depth 1 https://github.com/Jeomon/Windows-Use.git upstream
python -m pip install tabulate fuzzywuzzy python-levenshtein markdownify pydantic

python tools/parity/parity_regen.py      # Python side  → parity_python.json
cargo run --bin dump_parity              # Rust side    → parity_rust.json
python tools/parity/parity_compare.py    # must print NO DIFFERENCES
```

The tool schemas are generated rather than hand-written, so the JSON sent to the model stays
byte-identical to pydantic's:

```powershell
python tools/parity/generate_tool_schemas.py
```

## Tests

```powershell
cargo test
```

255 tests cover the tree tables, tabulate rendering, tool schemas and validation, loop detection,
fuzzy matching against captured fuzzywuzzy values, SendKeys sequences traced from a stubbed run of
the Python parser, markdownify output, message conversion for every provider, provider
construction without network access, Fernet round-trips, and the CLI surface.

## Requirements

- Windows 10 (17763+) or Windows 11
- Rust 1.80+

## Security

This agent operates your computer: it clicks, types, runs commands, and modifies files. There is
no sandbox. Run it in a virtual machine or Windows Sandbox while you evaluate it.

## Telemetry

Lightweight usage telemetry goes to PostHog, as in the original. Disable it with
`ANONYMIZED_TELEMETRY=false`.

## License

MIT — see [LICENSE](LICENSE). This project is a Rust port of
[Windows-Use](https://github.com/Jeomon/Windows-Use) by Jeomon George, also MIT licensed.
