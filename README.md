# OxideCode

A native, OpenCode-compatible coding agent written in Rust and C. It speaks the
OpenCode V2 HTTP/SSE API, so existing clients — including
[TermuxFileManger](https://github.com/its-zain-dev/TermuxFileManger) — keep
working while the whole agent runs as a single fast binary with no JavaScript
runtime.

## Why

- **Native speed.** Rust serves the API, runs the agent loop and renders the
  terminal UI. The only C is a small POSIX PTY / syscall layer, linked directly.
- **Small footprint.** One statically-linked-style binary, SQLite bundled, no
  Bun, Node or npm.
- **Same contract.** Envelopes, event names and payloads match OpenCode 2.2.6,
  including the SSE frame format used by the Android integration.
- **Batteries included.** Sessions, streaming messages, tools, permissions, VCS,
  file search, PTY and shell endpoints, model catalog and provider streaming.

## Install on Termux (arm64)

```bash
curl -fsSL https://raw.githubusercontent.com/its-zain-dev/oxidecode/main/scripts/install-termux.sh | bash
```

The installer places the binary at `$PREFIX/bin/opencode` and links
`$PREFIX/bin/oxidecode` to it, because the Android app invokes `opencode`.
It replaces an existing `opencode` binary in Termux.

Then:

```bash
opencode                 # terminal UI
opencode --version       # oxidecode 2.2.6
opencode service status  # prints the local API URL
opencode service get password
opencode models
opencode pair             # prints a browser pairing URL
```

Configure a provider through the usual OpenCode-compatible config
(`~/.config/opencode/opencode.json`) or environment variables such as
`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY` and
`DEEPSEEK_API_KEY`. Without a provider key, a deterministic offline mock model
is enabled automatically (`mock/mock-echo`).

## Build from source

The repository builds natively and in CI:

```bash
cargo build --release --locked -p oxidecode-cli --bin oxidecode
```

Cross builds:

| Target | How |
| --- | --- |
| Termux / Android arm64 | `cargo ndk -t arm64-v8a build --release --locked` |
| Linux x86_64 | `cargo build --release --locked` |
| Linux arm64 static | `cross build --release --target aarch64-unknown-linux-musl` |

GitHub Actions builds all three targets on every push and publishes binaries on
version tags.

## Layout

| Crate | Purpose |
| --- | --- |
| `oxidecode-sys` | Safe wrappers for the C PTY / syscall layer |
| `oxidecode-core` | IDs, config, auth, paths, SQLite store, session model |
| `oxidecode-provider` | Model catalog, OpenAI/Anthropic/mock streaming |
| `oxidecode-tools` | read, write, edit, bash, glob, grep, list, webfetch, todos |
| `oxidecode-server` | Axum HTTP/SSE API, agent runner, permissions |
| `oxidecode-tui` | Ratatui interface matching the OpenCode V2 look |
| `oxidecode-cli` | `opencode`-compatible CLI and daemon lifecycle |

## API compatibility

The server implements the OpenCode V2 routes used by the terminal UI and the
Android integration, including session creation and lookup, prompts, message
history, model/provider/agent catalogs, file search, permissions, PTY and the
event stream. Events are emitted as one compact `data: {...}` line per frame.

## License

MIT. OxideCode is an independent implementation and is not affiliated with the
OpenCode project.
