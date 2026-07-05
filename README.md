# MiniMaxProxy

A Zed extension that proxies inline code completions through the MiniMax-M3 chat-completions API. The prompt window is roughly 250k tokens, and the server rewrites each prompt into a shape M3 actually handles well.

Install it as a dev extension from [MaraniMatias/zed-minimax-proxy](https://github.com/MaraniMatias/zed-minimax-proxy). No marketplace step required.

## Quick start

```bash
git clone https://github.com/MaraniMatias/zed-minimax-proxy
cd zed-minimax-proxy

cargo build --release --manifest-path server/Cargo.toml
```

Then in Zed: `Cmd+Shift+P` → `zed: install dev extension` and pick the cloned directory. Zed compiles the WASM extension, restarts when you trigger a completion, and locates the server binary at the path built above.

Drop your API token in `~/.config/zed/settings.json` (see [Settings](#settings)) and reload Zed. The first completion request starts the server; nothing else to configure.

## Where the server binary lives

The extension looks for `minimax-proxy-server` in this order:

1. On `PATH` (`worktree.which(...)`); pick this if you `cargo install --path server`.
2. At `server/target/release/minimax-proxy-server` relative to the extension directory.
3. At `bin/minimax-proxy-server` relative to the extension directory.

If none of these resolve, the extension returns an error pointing at the missing binary. Build the server first, then reload.

## Settings

Settings live under the `minimax-proxy` LSP entry in `~/.config/zed/settings.json`. Five fields are recognized; the others are ignored. Defaults in parentheses.

| Field        | Type    | Default      | Notes                                                       |
| ------------ | ------- | ------------ | ----------------------------------------------------------- |
| `model`      | string  | `MiniMax-M3` | Forwarded to MiniMax as the model name.                     |
| `max_tokens` | integer | `256`        | Max output tokens per completion. Range 1-8192.             |
| `api_token`  | string  | (none)       | API token. Required. Sent as `MINIMAX_API_KEY`.              |
| `temperature`| number  | `0.2`        | Sampling temperature. Range 0.0-2.0.                        |
| `top_p`      | number  | `0.95`       | Nucleus sampling. Range 0.0-1.0.                            |

### Example `settings.json`

```json
{
  "lsp": {
    "minimax-proxy": {
      "settings": {
        "api_token": "sk-...",
        "model": "MiniMax-M3",
        "max_tokens": 256
      }
    }
  }
}
```

If `api_token` is missing, the server exits on startup (it requires `MINIMAX_API_KEY`). No silent default.

## How it talks to MiniMax

Zed sends the prompt as Qwen FIM markers; the proxy turns that into a chat-completion message in the format M3 was tuned on:

```
<|fim_prefix|>...código antes...<|fim_suffix|>...código después...<|fim_middle|>
            ↓
# language: typescript
<contextBeforeCursor>
...código antes...<cursorPosition>
<contextAfterCursor>
...código después...
```

The model fills `<cursorPosition>`. The rewrite keeps the cursor location explicit so M3 stops regurgitating FIM markers in its output.

Other things the proxy does:

- Splits the request timeout across the payload size (`base + per-K-tokens`), clamped to a hard cap.
- Slices long prompts at ~250k input tokens, keeping the most recent prefix and suffix.
- Applies default stop sequences (`<|fim_*>`, `<|endoftext|>`, doubled newlines) plus anything the caller passes.

## Building from source

### The extension (WASM)

```bash
cargo build --target wasm32-wasip2
```

The artifact lives at `target/wasm32-wasip2/debug/minimax_proxy_extension.wasm`. Dev installs in Zed read from this path; nothing to copy by hand.

`rust-toolchain.toml` pins the toolchain and target, so a fresh checkout should build without extra setup as long as `rustup target add wasm32-wasip2` resolves.

### The server

```bash
cargo build --release --manifest-path server/Cargo.toml
```

Output: `server/target/release/minimax-proxy-server`. The extension finds it there. To install it on your PATH instead:

```bash
cargo install --path server
```

### The Bun alternative

A Bun script in `docs/server.ts` implements the same HTTP contract. It's a development aid, not what the extension spawns. Useful for testing prompt rewrites without recompiling Rust:

```bash
MINIMAX_API_KEY=... bun docs/server.ts
```

## Talking to the server directly

The server listens on `127.0.0.1:8787` and accepts OpenAI-shaped `text_completion` requests:

```bash
curl -s http://127.0.0.1:8787/v1/completions \
  -H "content-type: application/json" \
  -d '{
    "model": "MiniMax-M3",
    "prompt": "<|fim_prefix|>const greet = (name) => <|fim_suffix|>;\n<|fim_middle|>",
    "max_tokens": 64
  }'
```

This works whether the server was launched by Zed or by hand.

## Repository layout

```
zed-minimax-proxy/
├── Cargo.toml              # workspace root + extension package
├── rust-toolchain.toml
├── extension.toml
├── src/lib.rs              # the extension (WASM)
├── server/                 # the HTTP proxy (Rust)
│   ├── Cargo.toml
│   └── src/main.rs
├── docs/server.ts          # Bun alternative (local testing)
├── README.md
└── LICENSE
```

## Limitations

- `user_stop_takes_priority_over_default` in the server crate had a stale assertion against an earlier draft of `apply_stop_sequences`. Fixed in this release.
- The extension expects a pre-built server binary in one of the three documented locations; there is no automatic build step.
- Streaming is not supported; the entire response is returned in a single message. Flipping `stream: true` in the server would require a different response shape.
