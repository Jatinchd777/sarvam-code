# sarvam-code

A minimal, production-grade coding agent written in Rust. It connects to the Sarvam AI chat-completions API, runs a loop of tool calls against your workspace, and presents the interaction through either a terminal UI or a headless print mode.

The core logic is UI-agnostic: the same agent loop works in the interactive TUI and when run with `--print`.

## Features

- **Streaming TUI** — live transcript, token output, reasoning blocks, tool call cards, scrolling, and a permission modal
- **Markdown rendering** — headings, bold, italic, strikethrough, inline and fenced code, nested lists, task lists, quotes, tables, and links
- **Five sandboxed tools** — `list_dir`, `read_file`, `edit_file`, `write_file`, `bash`
- **Permission modes** — `ask` (interactive approval), `read-only` (deny mutations), `yolo` (auto-approve)
- **Workspace sandbox** — realpath-based, symlink-aware, prevents path traversal
- **Context management** — tool-output truncation, per-turn step cap, automatic continuation on token limits
- **Persistent transcript** — `history.json` used as model memory, rewritten atomically after every step
- **Headless mode** — `--print` for scripting, CI, and testing
- **Offline mock server** — `scripts/mock-sarvam.py` for demos without an API key

## Quick Start

### Prerequisites

- Rust 1.85+ (edition 2024)
- An API key from [Sarvam AI](https://sarvam.ai) (or any dummy value with the mock server)

### Build

```bash
git clone https://github.com/yourorg/sarvam-code.git
cd sarvam-code
cargo build --release
```

### Configure your API key

```bash
mkdir -p ~/.config/sarvam-code
printf 'api-key = "your-key"\n' > ~/.config/sarvam-code/config.toml
```

### Run

Without a prompt argument, the interactive TUI opens. Type a task and press `Enter`.

```bash
./target/release/sarvam-code
```

## Interactive TUI Mode

The Ratatui interface provides:

- A live chat transcript with streaming tokens
- A minimal welcome screen with session configuration
- Reasoning blocks (when supported by the model)
- Tool call cards with expandable output
- A permission modal for mutating operations
- Slash commands with a fuzzy-filtered palette above the input

## Headless Mode (`--print`)

Run a single prompt without opening a TUI — useful for scripts and CI:

```bash
cargo run -- --print "list the files and summarize this repo"
```

> **Note:** Headless `ask` mode denies mutating tools because there is no TTY for the modal. Use `PERMISSION_MODE=yolo` for automation:

```bash
PERMISSION_MODE=yolo cargo run -- --print "run the tests and fix failures"
```

## Offline / Mock Server (No API Key Needed)

Start the mock server:

```bash
python3 scripts/mock-sarvam.py &
```

Then run the harness against it:

```bash
printf 'api-key = "dummy"\n' > /tmp/sarvam-code-mock.toml
SARVAM_CONFIG=/tmp/sarvam-code-mock.toml \
SARVAM_API_URL=http://127.0.0.1:8877/v2/chat/completions \
PERMISSION_MODE=read-only \
cargo run -- --print "test the loop"
```

The mock returns a scripted 3-step conversation:

1. `read_file` → `README.md`
2. `bash` → `echo hello-from-bash`
3. Final text answer

Every request body is logged to `/tmp/mock-sarvam-requests.jsonl` (override with `MOCK_LOG`).

> The mock's request counter is global — restart it before each demo run.

## TUI Controls

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
| `Up` / `Down` | Select slash command suggestion, otherwise input history |
| `Tab` | Complete the selected slash command |
| `Ctrl+W` / `Ctrl+U` | Delete word / clear input |
| `Ctrl+D` | Quit when input is empty |

### Slash Command Palette

Type `/` to list every command in a popup above the input. Continue typing to fuzzy-filter (`/tls` matches `/tools`). The highlighted row is always visible.

Type `/model ` to open the fuzzy-filtered model list. Use the arrow keys to choose a model and press `Enter` to switch, or `Tab` to complete it.

| Key | Action |
|-----|--------|
| `Up` / `Down` | Move the selection |
| `Tab` | Insert the selected command |
| `Enter` | Complete a partial command, or run an exact one |
| `Esc` | Clear the input and close the palette |

### Permission Modal

| Key | Action |
|-----|--------|
| `y` / `Enter` | Allow this call |
| `a` | Always allow (sets session to `yolo`) |
| `n` / `Esc` | Deny |

## Slash Commands

| Command | Action |
|---------|--------|
| `/help` | Show this help |
| `/model <name>` | Switch model |
| `/mode <ask|read-only|yolo>` | Switch permission mode |
| `/reset` | Clear conversation + transcript |
| `/clear` | Clear screen only |
| `/tools` | List available tools |
| `/transcript` | Show transcript path |
| `/exit`, `/quit` | Quit |

Start typing `/` to browse these with fuzzy matching and arrow-key selection.

## Configuration

Precedence for configurable options: **CLI flags > environment > config file > defaults**.

The API key is read only from the config file and is never read from an environment variable.

### Environment Variables

| Variable | Default | Meaning |
|----------|---------|---------|
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
api-key = "your-key"
model = "sarvam-105b-conversations"
permission_mode = "ask"
max_steps = 20
max_tokens = 8000
bash_timeout_secs = 60
max_output = 8000
http_timeout_secs = 300
temperature = 0.2
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

## Architecture

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
user prompt → append history → POST /v2/chat/completions → SSE deltas
                                                    ↓
                                            assistant message with tool_calls
                                                    ↓
                                            permission gate → execute → truncate → tool result
                                                    ↓
                                            model called again
                                                    ↓
                                            turn ends when no tool calls, max steps, or cancellation
```

1. **User message** is appended to `history.json`.
2. The **whole history**, system prompt, and five tool schemas are POSTed to `/v2/chat/completions` with `stream: true`.
3. **SSE deltas** update the TUI in real time. Accumulated `tool_calls` are collected by index.
4. The assistant message is appended **verbatim**, including `tool_calls` and their IDs. This is mandatory: a `tool` message must follow the assistant message that requested it.
5. Each call is checked against the **path sandbox**, passed through the **permission gate** if it mutates state, executed, **truncated**, and appended as a `{"role":"tool","tool_call_id":…}` message.
6. The model is called again. The turn ends when it returns no tool calls, or when `MAX_STEPS` / the user cancels. `finish_reason: "length"` triggers an automatic "continue" message.

> **The transcript is the only agent state.** The API is stateless.

## Tools

| Tool | Approval | Notes |
|------|----------|-------|
| `list_dir` | No | Capped at 200 entries |
| `read_file` | No | Numbered lines, `offset`/`limit` |
| `edit_file` | Yes | Exact replacement; unique unless `replace_all` |
| `write_file` | Yes | Exact bytes, creates parents |
| `bash` | Yes | `bash -c`, workspace cwd, timeout, output cap, `ulimit -f`, API key stripped from child env |

Tool errors are returned to the model as text so it can self-correct. Errors never crash the harness.

## Permissions

| Mode | Behavior |
|------|----------|
| `ask` | Mutating tools open a modal; `a` upgrades the whole session to `yolo` |
| `read-only` | Reads work; writes/edits/bash are denied |
| `yolo` | No prompts — full automation |
| **Per-call** | TUI permissions are per call; a denial is fed back to the model |
| **Headless `ask`** | Denies rather than silently approving |

## Security & Sandboxing

- **Path sandbox**: All file tool paths are resolved with `realpath` and verified to stay inside the workspace directory. Symlinks are followed safely.
- **Bash isolation**: Commands run with the workspace as cwd, a timeout, an output cap, and `ulimit -f` to prevent fork bombs. The `SARVAM_API_KEY` is stripped from the child environment.
- **No destructive commands by default**: The system prompt explicitly prohibits `rm -rf`, `git reset --hard`, and force pushes unless explicitly asked.
- **Permission gate**: Every mutating operation requires explicit approval in `ask` mode.
- **Tool error resilience**: Errors are returned as text to the model — never a panic.

## Project Layout

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

## Testing

### Unit / Integration Tests

```bash
cargo test
```

### Offline Mock Demo

```bash
# 1. Start mock
python3 scripts/mock-sarvam.py &

# 2. Configure a dummy API key and run against it
printf 'api-key = "dummy"\n' > /tmp/sarvam-code-mock.toml
SARVAM_CONFIG=/tmp/sarvam-code-mock.toml \
SARVAM_API_URL=http://127.0.0.1:8877/v2/chat/completions \
PERMISSION_MODE=read-only \
cargo run -- --print "test the loop"
```

### Headless Automation

```bash
PERMISSION_MODE=yolo cargo run -- --print "list files and summarize this repo"
```

## Development

### Build in debug mode with verbose output

```bash
cargo build
```

### Run tests

```bash
cargo test -- --nocapture
```

### Format code

```bash
cargo fmt
```

### Lint

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

## Troubleshooting

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| API key configuration error | Missing `api-key` in config | Add `api-key = "your-key"` to `~/.config/sarvam-code/config.toml` |
| `cannot reach the Sarvam API` | Network / firewall | Check `SARVAM_API_URL` and connectivity |
| Permission modal never appears | Headless mode | Use `PERMISSION_MODE=yolo` or run without `--print` |
| Mock server returns wrong step | Mock not restarted | Kill and restart `scripts/mock-sarvam.py` |
| `ulimit` or timeout errors | Bash command too long | Reduce `BASH_TIMEOUT` or simplify the command |
| Context window exceeded | Too much tool output | Lower `MAX_OUTPUT` or use `read_file` with `offset`/`limit` |

## License

MIT — see [LICENSE](LICENSE) for details.
