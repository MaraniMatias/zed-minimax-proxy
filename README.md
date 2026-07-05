# MiniMaxProxy

A Zed extension that proxies inline code completions through the MiniMax-M3 chat-completions API. The prompt window is roughly 250k tokens; the server turns each prompt into the chat-completion format M3 was tuned on.

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
  },
  "languages": {
    "Markdown-Inline": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "Plain Text": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "Markdown": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "Python": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "Rust": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "JSON": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "YAML": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "HTML": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "CSS": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "Dockerfile": { "language_servers": ["minimax-proxy", "cspell", "..."] },
    "JavaScript": {
      "language_servers": [
        "minimax-proxy",
        "cspell",
        "!tailwindcss-language-server",
        "!typescript-language-server",
        "!vue-language-server",
        "...",
      ],
    }
    // .... add "minimax-proxy" as lsp server
  }
}
```

If `api_token` is missing, the server exits on startup (it requires `MINIMAX_API_KEY`). It does not invent a placeholder.

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

A Bun script in `docs/server.ts` implements the same HTTP contract. It's a development aid, not what the extension spawns. Run it for prompt-rewrite tests when you don't want to rebuild the Rust server:

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
├── lefthook.yml            # git hooks (lint, test, build)
├── src/lib.rs              # the extension (WASM)
├── server/                 # the HTTP proxy (Rust)
│   ├── Cargo.toml
│   └── src/main.rs
├── docs/server.ts          # Bun alternative (local testing)
├── README.md
└── LICENSE
```

## Git hooks

[Lefthook](https://lefthook.dev/) runs three gates automatically:

- **pre-commit**: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Parallel; runs only when matching `.rs` files are staged.
- **pre-push**: `cargo build --target wasm32-wasip2`. Heavier, so it lives on push instead of commit.

Install once after cloning:

```bash
brew install lefthook      # or scoop / apt / etc.
lefthook install
```

Skip a hook on a one-off commit with `git commit --no-verify`.

## Limitations

- `user_stop_takes_priority_over_default` in the server crate had a stale assertion against an earlier draft of `apply_stop_sequences`. Fixed in this release.
- The extension expects a pre-built server binary in one of the three documented locations; there is no automatic build step.
- Streaming is not supported; the entire response is returned in a single message. Flipping `stream: true` in the server would require a different response shape.

## Releases

The server is released with [`release-plz`](https://release-plz.ails.it/) running 100% locally, no GitHub Actions consumed on the free tier. The `minimax-proxy-extension` package is excluded from release-plz (its version follows Zed's extension registry cadence, not this repo).

### One-time setup

```sh
cargo install release-plz --locked
cargo install cargo-zigbuild --locked
brew install zig       # cross-compile dependency for cargo-zigbuild
rustup target add aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu
```

`gh` (GitHub CLI) must be authenticated: `gh auth login`.

### Cutting a release

The flow runs entirely on your machine.

```sh
# 1. Confirm conventional-commit history since the last release.
git log v0.1.0..HEAD --oneline

# 2. release-plz opens a "chore: release vX.Y.Z" PR
#    that bumps server/Cargo.toml, server/Cargo.lock, and
#    writes server/CHANGELOG.md from commit history.
release-plz release

# 3. Auto-merge that PR; nothing executes on CI.
gh pr merge --auto --squash --delete-branch

# 4. Pull main locally.
git pull origin main

# 5. Build for each platform.
cargo build --release --target aarch64-apple-darwin \
  --manifest-path server/Cargo.toml

cargo build --release --target x86_64-apple-darwin \
  --manifest-path server/Cargo.toml

cargo zigbuild --release --target x86_64-unknown-linux-gnu \
  --manifest-path server/Cargo.toml

# 6. Package and upload.
for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
    tar -czf "server/target/minimax-proxy-server-v0.2.0-${t}.tar.gz" \
        -C "server/target/${t}/release" minimax-proxy-server
done

gh release create v0.2.0 \
    server/target/minimax-proxy-server-v0.2.0-*.tar.gz \
    --title "v0.2.0" --generate-notes
```

Use `release-plz release --dry-run` first if you want to preview the PR body and the changelog diff without pushing.

### Commit message format

release-plz reads conventional commits:

- `feat(server): ...` → minor bump on the server.
- `fix(server): ...` → patch bump.
- `feat!(server): ...` or a footer `BREAKING CHANGE: ...` → major bump.
- `chore(server):`, `docs:`, `refactor(server):` → no version change.

Prefix with `(server)` only when the change touches `server/`. The extension uses `(ext)` or no prefix and is ignored by release-plz.
