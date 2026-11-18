# 🦀 sarvam-code

> A minimal, production-grade **Sarvam AI coding agent** written in **Rust** — built with a **Ratatui TUI**, an **SSE streaming client**, a **permission gate**, and **five sandboxed tools**. No frameworks, no magic, just clean async code that you can read, audit, and extend.

[![Rust](https://img.shields.io/badge/rust-1.85%2B-blue)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Build](https://img.shields.io/badge/build-release-success)](https://github.com/yourorg/sarvam-code/actions)

---

## 🧠 What is this?

`sarvam-code` is a **loop**:

```
user prompt → model → tool_calls → permission gate → execute → tool result → model → …
```

The **agent core** is completely UI-agnostic:

- An **OpenAI-compatible message history** (`history.json`)
- An **SSE streaming client** for Sarvam's chat-completions API
- **Five tools**: `list_dir`, `read_file`, `edit_file`, `write_file`, `bash`
- A **permission gate** (`ask` / `read-only` / `yolo`)
- **Context management**: truncation, per-turn step caps, auto-continuation on token limits

The **Ratatui layer** only renders events and forwards keys — the same core runs **headlessly** with `--print`.

---

## ✨ Features

| Feature | Details |
|---------|---------|
| **Streaming TUI** | Live chat transcript, token output, reasoning blocks, tool call cards, scrolling, permission modal |
| **Markdown Rendering** | Headings, bold/italic/strikethrough, inline & fenced code with language labels, nested lists, task lists, quotes, rules, links, aligned tables |
| **5 Tools** | `list_dir`, `read_file`, `edit_file`, `write_file`, `bash` — all workspace-sandboxed |
| **Permission Modes** | `ask` (interactive approval), `read-only` (deny mutations), `yolo` (auto-approve) |
| **Workspace Sandbox** | `realpath`-based, symlink-aware, prevents path traversal |
| **Context Management** | Tool-output truncation, per-turn step cap, automatic continuation on token limit |
| **Persistent Transcript** | `history.json` used as model memory, rewritten **atomically** after every step |
| **Headless Mode** | `--print` for scripting, CI, and testing |
| **Offline Mock Server** | `scripts/mock-sarvam.py` for demos without an API key |

---

## 🚀 Quick Start

### Prerequisites

- **Rust 1.85+** (edition 2024)
- An **API key** from [Sarvam AI](https://sarvam.ai) (or any dummy value with the mock server)

```bash
# 1. Clone & build
git clone https://github.com/yourorg/sarvam-code.git
cd sarvam-code
cargo build --release

# 2. Export your API key
export SARVAM_API_KEY="your-key"

# 3. Run
./target/release/sarvam-code
```

With **no prompt argument**, it opens the interactive TUI. Type a task and press `Enter`.

---

### 🖥️ Interactive TUI Mode

The Ratatui interface gives you:

- A **live chat transcript** with streaming tokens
- **Reasoning blocks** (when supported by the model)
- **Tool call cards** with expandable output
- A **permission modal** for mutating operations
- **Slash commands** for quick control

---

### 🤖 Headless Mode (`--print`)

Run a single prompt without opening a TUI — perfect for scripts and CI:

```bash
cargo run -- --print "list the files and summarize this repo"
# stdout: final answer
# stderr: tool activity
```

> **Heads up:** Headless `ask` mode **denies** mutating tools (no TTY for the modal). Use `PERMISSION_MODE=yolo` for automation:

```bash
PERMISSION_MODE=yolo cargo run -- --print "run the tests and fix failures"
```

---

### 🔌 Offline / Mock Server (No API Key Needed)

Start the mock server:

```bash
python3 scripts/mock-sarvam.py &
```

Then run the harness against it:

```bash
SARVAM_API_KEY=dummy \
SARVAM_API_URL=http://127.0.0.1:8877/v2/chat/completions \
PERMISSION_MODE=read-only \
cargo run -- --print "test the loop"
```

The mock returns a **scripted 3-step conversation**:
1. `read_file` → `README.md`
2. `bash` → `echo hello-from-bash`
3. Final text answer

Every request body is logged to `/tmp/mock-sarvam-requests.jsonl` (override with `MOCK_LOG`).

> ⚠️ The mock's request counter is **global** — restart it before each demo run.

---

## ⌨️ TUI Controls

### General

| Key | Action |
|-----|--------|
| `Enter` | Send prompt |
| `Esc` | Cancel current turn, or clear input |
| `Ctrl+C` | Quit |
| `PgUp` / `PgDn` | Scroll a page |
| Mouse wheel | Scroll |
| `Shift+Up` / `Shift+Down` | Scroll 3 lines |
| `Ctrl+Home` / `Ctrl+End` | Jump to top / bottom |
| `Ctrl+O` | Expand / collapse tool output |
| `Up` / `Down` | Input history |
| `Ctrl+W` / `Ctrl+U` | Delete word / clear input |
| `Ctrl+D` | Quit when input is empty |

### Permission Modal

| Key | Action |
|-----|--------|
| `y` / `Enter` | Allow this call |
| `a` | Always allow (sets session to `yolo`) |
| `n` / `Esc` | Deny |

---

## ⚡ Slash Commands

| Command | Action |
|---------|--------|
| `/help` | Show this help |
| `/model <name>` | Switch model |
| `/mode <ask\|read-only\|yolo>` | Switch permission mode |
| `/reset` | Clear conversation + transcript |
| `/clear` | Clear screen only |
| `/tools` | List available tools |
| `/transcript` | Show transcript path |
| `/exit`, `/quit` | Quit |

---

## ⚙️ Configuration

Precedence: **CLI flags > environment > config file > defaults**

### Environment Variables

| Variable | Default | Meaning |
|----------|---------|---------|
| `SARVAM_API_KEY` | — (required) | Sent as `api-subscription-key` header |
| `SARVAM_API_URL` | `https://api.sarvam.ai/v2/chat/completions` | Endpoint |
| `SARVAM_MODEL` | `sarvam-105b-conversations` | Model name |
| `SARVAM_EFFORT` | *(omitted)* | `low`, `high`, `max` |
| `PERMISSION_MODE` | `ask` | `ask`, `read-only`, `yolo` |
| `MAX_STEPS` | `20` | Round-trips per user turn |
| `MAX_TOKENS` | `8000` | `max_tokens` per request |
| `BASH_TIMEOUT` | `60` | Seconds per bash command |
| `MAX_OUTPUT` | `8000` | Bytes of tool output to model |
| `HTTP_TIMEOUT` | `300` | Request + SSE idle timeout |
| `SARVAM_TEMPERATURE` | `0.2` | Sampling temperature |
| `SARVAM_STATE_DIR` | `~/.local/state/sarvam-code` | Transcript + scratch files |
| `SARVAM_CONFIG` | `~/.config/sarvam-code/config.toml` | Config file path |

### Config File (`~/.config/sarvam-code/config.toml`)

```toml
model = "sarvam-105b-conversations"
permission_mode = "ask"
max_steps = 20
max_tokens = 8000
bash_timeout_secs = 60
max_output = 8000
http_timeout_secs = 300
temperature = 0.2
# api_url = "https://api.sarvam.ai/v2/chat/completions"
# effort = "low"
```

### CLI Flags

| Flag | Short | Meaning |
|------|-------|---------|
| `--model <MODEL>` | | Override model |
| `--api-url <URL>` | | Override endpoint |
| `--permission-mode <MODE>` | | Override permission mode |
| `--effort <EFFORT>` | | Override reasoning effort |
| `--max-steps <N>` | | Override step cap |
| `-p, --print <PROMPT>` | | Run headlessly and exit |

---

## 🏗️ Architecture

```
src/
├── main.rs       CLI, terminal lifecycle, TUI event loop, headless mode
├── config.rs     Env/config-file/CLI precedence, system prompt, paths
├── agent.rs      Background agent task: turn loop, history, approvals, cancellation
├── api.rs        Sarvam client, SSE decoder, streaming/non-streaming reply parsing
├── tools.rs      Tool schemas, sandboxed execution, truncation, bash runner
├── session.rs    Atomic transcript persistence
├── app.rs        UI state machine, chat items, input editor, slash commands
├── markdown.rs   CommonMark/GFM renderer: inline styles, lists, code, tables
└── ui.rs         Ratatui rendering: header, transcript, tool cards, modal
```

### How It Works (The Loop)

```
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│  User    │────▶│  Append  │────▶│  POST to │────▶│  SSE     │
│  Prompt  │     │ history  │     │ /v2/chat/│     │  Deltas  │
└──────────┘     └──────────┘     │ completions│    └────┬─────┘
                                    └──────────┘         │
                                                          ▼
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│  Final   │◀────│  Append  │◀────│  Tool    │◀────│  Execute │
│  Answer  │     │ verbatim │     │  Calls   │     │  & Trunc.│
└──────────┘     └──────────┘     └──────────┘     └──────────┘
```

1. **User message** is appended to `history.json`.
2. The **whole history**, system prompt, and five tool schemas are POSTed to `/v2/chat/completions` with `stream: true`.
3. **SSE deltas** update the TUI in real time. Accumulated `tool_calls` are collected by index.
4. The assistant message is appended **verbatim**, including `tool_calls` and their IDs. This is mandatory: a `tool` message must follow the assistant message that requested it.
5. Each call is checked against the **path sandbox**, passed through the **permission gate** if it mutates state, executed, **truncated**, and appended as a `{"role":"tool","tool_call_id":…}` message.
6. The model is called again. The turn ends when it returns no tool calls, or when `MAX_STEPS` / the user cancels. `finish_reason: "length"` triggers an automatic "continue" message.

> **The transcript is the only agent state.** The API is stateless.

---

### 🔧 Tools

| Tool | Approval | Notes |
|------|----------|-------|
| `list_dir` | ❌ No | Capped at 200 entries |
| `read_file` | ❌ No | Numbered lines, `offset`/`limit` |
| `edit_file` | ✅ Yes | Exact replacement; unique unless `replace_all` |
| `write_file` | ✅ Yes | Exact bytes, creates parents |
| `bash` | ✅ Yes | `bash -c`, workspace cwd, timeout, output cap, `ulimit -f`, API key stripped from child env |

**Tool errors are returned to the model as text** so it can self-correct. Errors **never crash** the harness.

---

### 🛡️ Permissions

| Mode | Behavior |
|------|----------|
| `ask` | Mutating tools open a modal; `a` upgrades the whole session to `yolo` |
| `read-only` | Reads work; writes/edits/bash are denied |
| `yolo` | No prompts — full automation |
| **Per-call** | TUI permissions are per call; a denial is fed back to the model |
| **Headless `ask`** | Denies rather than silently approving |

---

## 📦 Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `anyhow` | `1.0.104` | Error handling |
| `clap` | `4.6.8` | CLI argument parsing |
| `dirs` | `7.0.0` | Platform dirs for config/state |
| `futures-util` | `0.3.34` | Async stream utilities |
| `pulldown-cmark` | `0.13.6` | CommonMark/GFM parsing for TUI markdown |
| `ratatui` | `0.29` | TUI rendering |
| `reqwest` | `0.13.5` | HTTP client (json, stream, rustls) |
| `serde` | `1.0.229` | Serialization |
| `serde_json` | `1.0.154` | JSON |
| `textwrap` | `0.16.4` | Text wrapping |
| `tokio` | `1.54.1` | Async runtime |
| `toml` | `1.2.1` | Config file parsing |
| `unicode-width` | `0.2.0` | Width-aware text rendering |

---

## 🧪 Testing

### Unit / Integration Tests

```bash
cargo test
```

### Offline Mock Demo

```bash
# 1. Start mock
python3 scripts/mock-sarvam.py &

# 2. Run harness against it
SARVAM_API_KEY=dummy \
SARVAM_API_URL=http://127.0.0.1:8877/v2/chat/completions \
PERMISSION_MODE=read-only \
cargo run -- --print "test the loop"
```

### Headless Automation

```bash
PERMISSION_MODE=yolo cargo run -- --print "list files and summarize this repo"
```

---

## 🛡️ Security & Sandboxing

- **Path sandbox**: All file tool paths are resolved with `realpath` and verified to stay inside the workspace directory. Symlinks are followed safely.
- **Bash isolation**: Commands run with the workspace as cwd, a timeout, an output cap, and `ulimit -f` to prevent fork bombs. The `SARVAM_API_KEY` is **stripped** from the child environment.
- **No destructive commands by default**: The system prompt explicitly prohibits `rm -rf`, `git reset --hard`, and force pushes unless explicitly asked.
- **Permission gate**: Every mutating operation requires explicit approval in `ask` mode.
- **Tool error resilience**: Errors are returned as text to the model — never a panic.

---

## 📁 Project Layout

```
sarvam-code/
├── Cargo.toml                 # Package manifest & dependencies
├── Cargo.lock                 # Locked dependency tree
├── README.md                  # You are here
├── .gitignore
├── scripts/
│   ├── mock-sarvam.py          # Offline mock server (OpenAI-compatible)
│   └── sarvam-loop.sh          # Bash reference implementation of the same loop
├── src/
│   ├── main.rs                 # Entry point, CLI, TUI/headless dispatch
│   ├── config.rs               # Env/config/CLI precedence, system prompt
│   ├── agent.rs                # Background agent task, turn loop, approvals
│   ├── api.rs                  # Sarvam client, SSE decoder, reply parsing
│   ├── tools.rs                # Tool schemas, sandboxed execution, truncation
│   ├── session.rs              # Atomic transcript persistence
│   ├── app.rs                  # UI state machine, chat items, input, slash cmds
│   ├── markdown.rs             # CommonMark/GFM → styled terminal lines
│   └── ui.rs                   # Ratatui rendering (header, transcript, cards, modal)
└── target/                     # Build output (gitignored)
```

---

## 🤝 Contributing

1. Fork the repo.
2. Create a feature branch (`git checkout -b feat/amazing-feature`).
3. Commit your changes (`git commit -m 'Add some amazing feature'`).
4. Push to the branch (`git push origin feat/amazing-feature`).
5. Open a Pull Request.

### Development Tips

```bash
# Build in debug mode with verbose output
cargo build

# Run tests
cargo test -- --nocapture

# Format code
cargo fmt

# Lint
cargo clippy --all-targets --all-features -- -D warnings
```

---

## 📄 License

MIT — see [LICENSE](LICENSE) for details.

---

## 🙏 Acknowledgments

- **Sarvam AI** — for the chat completions API and model access.
- **Ratatui** — for the beautiful terminal UI.
- **Tokio** — for the async runtime that makes it all possible.
- **Reqwest** — for the HTTP client with SSE streaming support.
- **Serde** — for JSON serialization.

---

## 🆘 Troubleshooting

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| `SARVAM_API_KEY is not set` | Missing env var | `export SARVAM_API_KEY="your-key"` |
| `cannot reach the Sarvam API` | Network / firewall | Check `SARVAM_API_URL` and connectivity |
| Permission modal never appears | Headless mode | Use `PERMISSION_MODE=yolo` or run without `--print` |
| Mock server returns wrong step | Mock not restarted | Kill and restart `scripts/mock-sarvam.py` |
| `ulimit` or timeout errors | Bash command too long | Reduce `BASH_TIMEOUT` or simplify the command |
| Context window exceeded | Too much tool output | Lower `MAX_OUTPUT` or use `read_file` with `offset`/`limit` |

---

> **"The best code is the code you can read."** — *sarvam-code philosophy*
