# Windows-Use — Rust port

A complete Rust rewrite of [Windows-Use](https://github.com/Jeomon/Windows-Use) (v0.8.1), the AI
agent that drives Windows at the GUI layer. Everything is native Rust: no Python runtime, no
vendor SDKs, no COM wrapper layer.

```
rust/windows-use/
├── src/
│   ├── uia/            UI Automation (UIA3) through the `windows` crate
│   ├── tree/           accessibility tree capture, filtering and rendering
│   ├── desktop/        desktop state, windows, input, PowerShell, fuzzy matching
│   ├── screenshot.rs   GDI screen capture + annotated screenshots
│   ├── vdm.rs          virtual desktop COM interfaces (raw vtable calls)
│   ├── win32.rs        window/process/screen/registry helpers
│   ├── agent/          agent loop, prompts, tool registry, loop guard
│   ├── tools.rs        tool parameters + pydantic-compatible validation
│   ├── events.rs       event emitter and subscribers
│   ├── messages/       message models shared with providers
│   ├── providers/      all 16 LLM providers + 5 STT + 5 TTS providers
│   ├── speech/         microphone capture and audio playback
│   ├── telemetry/      PostHog telemetry
│   └── cli/            clap CLI, setup wizard, encrypted config
└── Cargo.toml
```

## Build and run

```powershell
cargo build --release --manifest-path rust/windows-use/Cargo.toml
rust/target/release/windows-use.exe
```

The CLI accepts the same options as the Python original:

```
-m, --model           LLM model ID (overrides config)
-p, --provider        LLM provider (overrides config)
    --max-steps       Maximum agent steps per task [default: 200]
-d, --debug           Enable debug logging
    --mode            flash | normal [default: normal]
    --use-vision      Send screenshots to the LLM
    --use-annotation  Annotate UI elements on screenshots
    --task            Run a single task and exit
```

Interactive commands: `\llm`, `\key`, `\speech`, `\voice`, `\clear`, `\quit`.

## Architecture notes

**UIA is called directly.** The Python version goes through `comtypes` and a generated typelib.
Here `IUIAutomation`, `IUIAutomationElement`, the tree walkers and every pattern interface come
from `windows` 0.58 bindings. The cache request used for traversal carries the same 35 properties
and 3 patterns the Python factory does, so element metadata matches field for field.

**VDM uses raw vtable calls.** Windows' virtual desktop interfaces are undocumented and their
vtable layouts shift between builds. `vdm.rs` selects one of six layouts from `(build, UBR)` and
calls slots by index, which keeps create/switch/rename/remove working across Windows 10 and every
Windows 11 release through 24H2+.

**Providers speak HTTP.** Every provider is a `reqwest` client. The twelve OpenAI-compatible
providers share one engine; Anthropic, Gemini, Ollama and Perplexity have their own converters
(thinking blocks and signatures, `functionCall` parts, NDJSON tool calls, citations).

**Config is Fernet-compatible.** API keys written by either implementation can be read by the
other: PBKDF2-HMAC-SHA256 with 480 000 iterations over the home path, AES-128-CBC, HMAC-SHA256,
urlsafe base64 — the exact `cryptography.fernet` construction.

## Behavioural parity

Deterministic outputs were diffed against the Python original. `parity_dump.py` renders twenty
categories from the upstream modules (with the real `tabulate`, `fuzzywuzzy`, `markdownify` and
pydantic models), and `rust/windows-use/src/bin/dump_parity.rs` renders the same twenty from the
Rust crate. The comparison reports **no differences** for:

- interactive/scrollable element tables, including metadata JSON formatting
- window and virtual desktop tables (`tabulate(tablefmt="simple")`, right-aligned numerics)
- all 15 tool parameter schemas, generated from the pydantic models by
  `generate_tool_schemas.py`
- fuzzy scores (`ratio`, `partial_ratio`, `token_sort_ratio`, `token_set_ratio`, `WRatio`),
  including `full_process` spacing and case sensitivity
- SendKeys parsing (`{Ctrl}a` → Ctrl↓ A↓ A↑ Ctrl↑, `{Shift}(ab)`, `{a 3}`, `{{}` escapes)
- HTML→Markdown conversion (`===` h1 underlines, `*`/`+` list markers, hard breaks)
- prompt templating, markdown stripping, tool-name formatting, loop-guard warnings

Regenerate and re-check at any time:

```powershell
.\.refenv\Scripts\python.exe parity_regen.py      # Python side → parity_python.json
cargo run --bin dump_parity -- parity_rust.json    # Rust side
.\.refenv\Scripts\python.exe parity_compare.py     # diff
```

## Tests

```powershell
cargo test --manifest-path rust/windows-use/Cargo.toml
```

255 tests cover the tree tables, tabulate rendering, tool schemas and validation, loop detection,
fuzzy matching against captured fuzzywuzzy values, SendKeys sequences against a traced run of the
Python parser, markdownify output, message conversion for every provider, provider construction
without network access, Fernet round-trips, and the CLI surface.

`src/bin/e2e_check.rs` runs the whole agent against the live desktop with a scripted LLM: it
captures real windows and elements, builds both prompts, validates a tool call, and completes an
agent turn.

## Feature coverage

| Python module | Rust | Notes |
| --- | --- | --- |
| `uia/*` (11k lines: comtypes wrappers) | `uia/` | Native COM bindings; only the surface the agent uses |
| `agent/tree` | `tree/` | Same filters, ordering, DOM correction, metadata |
| `agent/desktop` | `desktop/` | Windows, app launch/switch/resize, PowerShell, input |
| `vdm/core` | `vdm.rs` | Six build layouts, HSTRING rename support |
| `agent/service` | `agent/service.rs` | Sync + async loops, retries, compaction, events |
| `agent/tools` (15 tools) | `agent/tools.rs` | Identical messages and result strings |
| `providers/*` (16) | `providers/` | Direct HTTP, streaming, thinking signatures |
| STT/TTS providers (10) | `providers/speech.rs` | OpenAI, Groq, Google, ElevenLabs, Deepgram |
| `speech/service` | `speech/service.rs` | `cpal` capture, `hound` WAV, `rodio` playback |
| `telemetry` | `telemetry/` | PostHog, same event name and properties |
| `cli/*` | `cli/` | clap, setup wizard, Fernet config, REPL |

Two intentional deviations:

- **`Agent.watchdog`** is not ported. The Python watchdog spins a COM STA thread to pump focus
  events; in this port each capture builds its cache request directly, so the event pump it
  guarded adds cost without changing output.
- **Video/GUI front-ends** (none in upstream) are out of scope.

## Regenerating the tool schemas

`tool_schemas.rs` is generated so the JSON sent to the model stays byte-identical to pydantic's:

```powershell
.\.refenv\Scripts\python.exe generate_tool_schemas.py
```
