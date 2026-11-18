#!/usr/bin/env bash
# sarvam-loop.sh — the smallest honest agent harness: bash + curl + jq.
#
# It demonstrates the complete loop:
#   request -> assistant.tool_calls -> permission gate -> execute -> feed result back -> repeat
#
# Usage:
#   export SARVAM_API_KEY=YOUR_KEY
#   ./scripts/sarvam-loop.sh                       # interactive REPL
#   ./scripts/sarvam-loop.sh "list files and summarize this repo"
#
# Offline test (no API key needed):
#   python3 scripts/mock-sarvam.py &               # fake API on :8877
#   SARVAM_API_KEY=dummy \
#   SARVAM_API_URL=http://127.0.0.1:8877/v2/chat/completions \
#   PERMISSION_MODE=read-only ./scripts/sarvam-loop.sh "test the loop"
#
# Knobs:
#   SARVAM_MODEL      default deepseekv4.1-flash-1017  (also: glm5.3, gemma4, sarvam-105b)
#   SARVAM_EFFORT     default none                     ("low"|"high"|"max"; glm5.3 rejects "none")
#   SARVAM_API_URL    default https://api.sarvam.ai/v2/chat/completions
#   PERMISSION_MODE   ask (default) | read-only | yolo
#   MAX_STEPS=20  MAX_TOKENS=8000  BASH_TIMEOUT=60  MAX_OUTPUT=8000  HTTP_TIMEOUT=300
#
# Requires: bash 4+, curl, jq.

set -uo pipefail

# ---------------------------------------------------------------- config

: "${SARVAM_API_KEY:?set SARVAM_API_KEY (use any dummy value with scripts/mock-sarvam.py)}"

API_URL="${SARVAM_API_URL:-https://api.sarvam.ai/v2/chat/completions}"
MODEL="${SARVAM_MODEL:-sarvam-105b-conversations}"
EFFORT="${SARVAM_EFFORT:-none}"
PERMISSION_MODE="${PERMISSION_MODE:-ask}"
MAX_STEPS="${MAX_STEPS:-20}"
MAX_TOKENS="${MAX_TOKENS:-8000}"
BASH_TIMEOUT="${BASH_TIMEOUT:-60}"
MAX_OUTPUT="${MAX_OUTPUT:-8000}"
HTTP_TIMEOUT="${HTTP_TIMEOUT:-300}"

WORKSPACE="$(pwd -P)"
STATE_DIR="${SARVAM_STATE_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/sarvam-loop.XXXXXX")}"
mkdir -p -- "$STATE_DIR" || {
    echo "cannot create state dir: $STATE_DIR" >&2
    exit 1
}
HIST="$STATE_DIR/history.json"
TOOLS="$STATE_DIR/tools.json"
REQ="$STATE_DIR/request.json"
RESP="$STATE_DIR/response.json"
TMPFILE="$STATE_DIR/tmp"

for c in curl jq; do
    command -v "$c" >/dev/null || {
        echo "missing dependency: $c" >&2
        exit 1
    }
done

say() { printf '%s\n' "$*" >&2; }

# ---------------------------------------------------------------- tools

cat >"$TOOLS" <<'JSON'
[
  {
    "type": "function",
    "function": {
      "name": "list_dir",
      "description": "List entries in a directory inside the workspace. Use it to orient yourself before guessing paths.",
      "parameters": {
        "type": "object",
        "properties": {
          "path": { "type": "string", "description": "Directory relative to the workspace. Defaults to '.'." }
        }
      }
    }
  },
  {
    "type": "function",
    "function": {
      "name": "read_file",
      "description": "Read a text file from the workspace. Returns numbered lines. Read a file before editing it. If output is truncated, call again with a larger offset.",
      "parameters": {
        "type": "object",
        "properties": {
          "path":   { "type": "string",  "description": "File path relative to the workspace." },
          "offset": { "type": "integer", "description": "First line to return, 1-based. Defaults to 1." },
          "limit":  { "type": "integer", "description": "Maximum lines to return. Defaults to 400." }
        },
        "required": ["path"]
      }
    }
  },
  {
    "type": "function",
    "function": {
      "name": "write_file",
      "description": "Create or overwrite a file with exact content. Use for new files or full rewrites. The user is asked for permission each time.",
      "parameters": {
        "type": "object",
        "properties": {
          "path":    { "type": "string", "description": "File path relative to the workspace." },
          "content": { "type": "string", "description": "Full file content." }
        },
        "required": ["path", "content"]
      }
    }
  },
  {
    "type": "function",
    "function": {
      "name": "bash",
      "description": "Run a shell command with the workspace as the current directory. Use for builds, tests, git status and inspection. The user is asked for permission each time. Avoid destructive commands.",
      "parameters": {
        "type": "object",
        "properties": {
          "command": { "type": "string", "description": "Command line to run with bash -c." }
        },
        "required": ["command"]
      }
    }
  }
]
JSON

SYSTEM_PROMPT="You are sarvam-code, a minimal coding agent working in $WORKSPACE on $(uname -s).
You can inspect and modify files, and run shell commands, through the provided tools.
Rules:
- Inspect before you change: read a file before editing it.
- Prefer write_file for new files; keep changes small and focused.
- Never run destructive commands (rm -rf, git reset --hard, force push) unless explicitly asked.
- If a tool result says it was truncated, request a narrower slice instead of guessing.
- Every write and every bash command requires user permission; if denied, adapt instead of retrying blindly.
- When the task is done, reply with a short summary of what changed and what you verified.
- Answer in the language the user writes in."

# ---------------------------------------------------------------- helpers

# Resolve a model-supplied path and reject anything outside the workspace.
within_workspace() { # $1 = raw path; prints absolute path on success
    local resolved
    resolved="$(realpath -m -- "$1")" || return 1
    case "$resolved" in
    "$WORKSPACE" | "$WORKSPACE"/*)
        printf '%s' "$resolved"
        return 0
        ;;
    *) return 1 ;;
    esac
}

# Permission gate. Returns 0 if allowed.
confirm() { # $1 = short label, $2 = details to show
    [[ "$PERMISSION_MODE" == "yolo" ]] && return 0

    if [[ "$PERMISSION_MODE" == "read-only" ]]; then
        say "⛔ read-only mode: denied $1"
        return 1
    fi
    if [[ ! -t 0 ]]; then
        say "⛔ no TTY, default-deny: $1"
        return 1
    fi

    {
        printf '\n⚠  %s\n' "$1"
        printf '%s\n' "$2" | head -c 1000 | sed 's/^/   | /'
        printf '\n'
    } >&2

    local ans
    read -r -p "   allow? [y]es / [n]o / [a]lways: " ans || return 1
    case "${ans:-n}" in
    y | Y | yes | YES) return 0 ;;
    a | A | always)
        PERMISSION_MODE="yolo"
        say "   (permissions set to yolo for this session)"
        return 0
        ;;
    *) return 1 ;;
    esac
}

# Cap what the model gets to see. Tool output is the #1 way to blow up context.
truncate_output() {
    local s
    s="$(cat)"
    if ((${#s} > MAX_OUTPUT)); then
        printf '%s\n[... truncated %d of %d chars — request a narrower slice]\n' \
            "${s:0:MAX_OUTPUT}" "$((${#s} - MAX_OUTPUT))" "${#s}"
    else
        printf '%s' "$s"
    fi
}

# Execute one tool call. Prints the observation that goes back to the model.
# Errors are returned as text on purpose: the model gets to self-correct.
run_tool() { # $1 = tool name, $2 = arguments JSON
    local name="$1" args="$2"
    local rawpath path offset limit content cmd rc total outfile

    case "$name" in
    list_dir)
        rawpath="$(jq -r '.path // "."' <<<"$args")"
        if ! path="$(within_workspace "$rawpath")"; then
            echo "error: path escapes the workspace: $rawpath"
            return 0
        fi
        if [[ ! -d "$path" ]]; then
            echo "error: not a directory: $rawpath"
            return 0
        fi
        (cd -- "$path" && ls -la) 2>&1 | head -200
        ;;

    read_file)
        rawpath="$(jq -r '.path // empty' <<<"$args")"
        [[ -n "$rawpath" ]] || {
            echo "error: missing path"
            return 0
        }
        if ! path="$(within_workspace "$rawpath")"; then
            echo "error: path escapes the workspace: $rawpath"
            return 0
        fi
        if [[ ! -f "$path" ]]; then
            echo "error: no such file: $rawpath"
            return 0
        fi
        offset="$(jq -r '.offset // 1' <<<"$args")"
        limit="$(jq -r '.limit // 400' <<<"$args")"
        awk -v o="$offset" -v l="$limit" \
            'NR >= o && NR < o + l { printf "%d: %s\n", NR, $0 }' "$path"
        total="$(wc -l <"$path")"
        if ((offset + limit - 1 < total)); then
            echo "[showed lines $offset..$((offset + limit - 1)) of $total lines]"
        fi
        ;;

    write_file)
        rawpath="$(jq -r '.path // empty' <<<"$args")"
        [[ -n "$rawpath" ]] || {
            echo "error: missing path"
            return 0
        }
        if ! path="$(within_workspace "$rawpath")"; then
            echo "error: path escapes the workspace: $rawpath"
            return 0
        fi
        content="$(jq -r '.content // empty' <<<"$args")"
        if ! confirm "write_file → $rawpath" "$(printf '%s' "$content" | head -c 800)"; then
            echo "error: user denied write_file"
            return 0
        fi
        mkdir -p -- "$(dirname -- "$path")" || {
            echo "error: cannot create parent directory"
            return 0
        }
        if jq -j '.content // ""' <<<"$args" >"$path"; then
            echo "ok: wrote $(wc -c <"$path") bytes to $rawpath"
        else
            echo "error: write failed: $rawpath"
        fi
        ;;

    bash)
        cmd="$(jq -r '.command // empty' <<<"$args")"
        [[ -n "$cmd" ]] || {
            echo "error: missing command"
            return 0
        }
        if ! confirm "bash → $cmd" "cwd=$WORKSPACE  timeout=${BASH_TIMEOUT}s  SARVAM_API_KEY removed from child env"; then
            echo "error: user denied bash"
            return 0
        fi
        outfile="$STATE_DIR/last-bash.out"
        # ulimit -f caps the output file (~1 MiB) so one runaway command can't fill the disk.
        (
            ulimit -f 2048 2>/dev/null || true
            cd -- "$WORKSPACE" &&
                env -u SARVAM_API_KEY -u SARVAM_API_URL \
                    timeout --signal=TERM "$BASH_TIMEOUT" bash -c "$cmd"
        ) >"$outfile" 2>&1
        rc=$?
        head -c 12000 "$outfile"
        echo
        echo "[exit code: $rc, output: $(wc -c <"$outfile") bytes]"
        ;;

    *)
        echo "error: unknown tool: $name"
        ;;
    esac
}

# ---------------------------------------------------------------- API

api_call() {
    jq -n \
        --arg model "$MODEL" \
        --argjson max_tokens "$MAX_TOKENS" \
        --arg effort "$EFFORT" \
        --slurpfile messages "$HIST" \
        --slurpfile tools "$TOOLS" \
        '{model: $model,
      messages: $messages[0],
      tools: $tools[0],
      tool_choice: "auto",
      parallel_tool_calls: false,
      max_tokens: $max_tokens,
      temperature: 0.2,
      stream: false}
     + (if $effort == "" then {} else {reasoning_effort: $effort} end)' >"$REQ"

    local http
    http="$(curl -sS --max-time "$HTTP_TIMEOUT" \
        -o "$RESP" -w '%{http_code}' \
        -H "api-subscription-key: $SARVAM_API_KEY" \
        -H "Content-Type: application/json" \
        --data-binary @"$REQ" "$API_URL")" || {
        say "curl failed (network/timeout)"
        return 1
    }

    if [[ "$http" != 2* ]]; then
        say "API error: HTTP $http"
        jq . "$RESP" >&2 2>/dev/null || cat "$RESP" >&2
        return 1
    fi
}

# Append a JSON object to the history array safely.
hist_append() { # $1 = jq filter producing one object
    jq --argjson x "$1" '. + [$x]' "$HIST" >"$TMPFILE" && mv "$TMPFILE" "$HIST"
}

# ---------------------------------------------------------------- loop

agent_turn() {
    local step=0 finish msg text n i id name rawargs result

    while :; do
        step=$((step + 1))
        if ((step > MAX_STEPS)); then
            say "… step limit ($MAX_STEPS) reached, stopping this turn"
            return 0
        fi

        say "[step $step]"
        api_call || return 1

        finish="$(jq -r '.choices[0].finish_reason // "stop"' "$RESP")"
        msg="$(jq -c '.choices[0].message | del(.reasoning_content)' "$RESP")"
        text="$(jq -r '.choices[0].message.content // empty' "$RESP")"
        n="$(jq '.choices[0].message.tool_calls // [] | length' "$RESP")"

        # Replay the assistant turn verbatim — tool_calls included. Dropping it is
        # the classic way to get a 400 "tool message without a parent" error.
        jq --argjson m "$msg" '. + [$m]' "$HIST" >"$TMPFILE" && mv "$TMPFILE" "$HIST"

        [[ -n "$text" ]] && printf '\n%s\n' "$text"

        if ((n == 0)); then
            if [[ "$finish" == "length" ]]; then
                say "… hit max_tokens; asking the model to continue"
                jq '. + [{role: "user", content: "Your previous reply was cut off by the token limit. Continue from where you stopped."}]' \
                    "$HIST" >"$TMPFILE" && mv "$TMPFILE" "$HIST"
                continue
            fi
            return 0
        fi

        for ((i = 0; i < n; i++)); do
            id="$(jq -r ".choices[0].message.tool_calls[$i].id" "$RESP")"
            name="$(jq -r ".choices[0].message.tool_calls[$i].function.name" "$RESP")"
            rawargs="$(jq -r ".choices[0].message.tool_calls[$i].function.arguments" "$RESP")"

            say "→ $name $(printf '%s' "$rawargs" | cut -c1-300)"

            if ! jq -e . >/dev/null 2>&1 <<<"$rawargs"; then
                say "   (invalid JSON arguments — returning an error to the model)"
                result="error: arguments were not valid JSON; send the tool call again with valid JSON"
            else
                result="$(run_tool "$name" "$rawargs" | truncate_output)"
            fi
            say "← ${#result} chars"

            jq --arg id "$id" --arg c "$result" \
                '. + [{role: "tool", tool_call_id: $id, content: $c}]' \
                "$HIST" >"$TMPFILE" && mv "$TMPFILE" "$HIST"
        done
        # loop: send the tool results back and let the model continue
    done
}

# ---------------------------------------------------------------- main

main() {
    jq -n --arg s "$SYSTEM_PROMPT" '[{role: "system", content: $s}]' >"$HIST"

    say "sarvam-loop • model=$MODEL • permissions=$PERMISSION_MODE"
    say "workspace=$WORKSPACE"
    say "state=$STATE_DIR  (history.json is the full transcript)"

    local input
    while :; do
        if (($# > 0)); then
            input="$1"
            shift
        else
            printf '\nyou> '
            IFS= read -r input || break
        fi

        case "$input" in
        "") continue ;;
        /exit | /quit) break ;;
        /reset)
            jq -n --arg s "$SYSTEM_PROMPT" '[{role: "system", content: $s}]' >"$HIST"
            say "(history cleared)"
            continue
            ;;
        esac

        jq --arg c "$input" '. + [{role: "user", content: $c}]' \
            "$HIST" >"$TMPFILE" && mv "$TMPFILE" "$HIST"

        agent_turn || say "(turn failed; history kept for inspection)"
    done

    say "bye. transcript: $HIST"
}

main "$@"
