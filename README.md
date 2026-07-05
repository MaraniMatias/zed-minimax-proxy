# MiniMaxProxy

Extensión de Zed que proxea las code completions contra la API de **MiniMax-M3** con un contexto de hasta ~250k tokens. Pensada para inline completions en archivos grandes.

## Arquitectura

```
┌──────────┐    shell_env    ┌─────────────────────────┐    HTTPS    ┌────────────┐
│   Zed    │ ──────────────▶ │  minimax-proxy-server   │ ──────────▶ │  MiniMax   │
│ (WASM)   │ ◀── HTTP JSON ─ │  (Rust/Axum · :8787)    │ ◀────────── │  M3 API    │
└──────────┘                 └─────────────────────────┘             └────────────┘
   extension                       server/
   · wasm spawns                   · reescribe el prompt a formato
   · inyecta env vars                "Minuet style" (FIM markers)
     en cada LSP startup            · divide el timeout según tamaño
```

Hay **dos implementaciones del server** (mismo contrato HTTP, mismo comportamiento):

| Implementación | Path                       | Stack                    |
| -------------- | -------------------------- | ------------------------ |
| **Rust/Axum**  | `server/src/main.rs`        | Rust + axum + reqwest    |
| **Bun**        | `docs/server.ts`            | TypeScript + Bun runtime |

Por defecto la extensión de Zed usa el binario Rust (`server/`). El script de Bun es útil para iterar rápido sin recompilar.

## Build

### 1. WASM de la extensión

Zed, al instalar la extensión en modo dev, ejecuta `cargo build --target wasm32-wasip2` y lee el artefacto directamente desde `target/wasm32-wasip2/debug/<crate_name>.wasm` (no necesita un paso manual de copia). Si querés producir un release:

```bash
cargo build --release --target wasm32-wasip2
# salida: target/wasm32-wasip2/release/minimax_proxy_extension.wasm
```

El target `wasm32-wasip2` lo declara `rust-toolchain.toml`; rustup lo descarga solo si falta.

### 2. Server (Rust)

```bash
cargo build --release --manifest-path server/Cargo.toml
cp server/target/release/minimax-proxy-server bin/
```

### 3. Binario standalone (sin Zed)

El server expone `POST http://localhost:8787/v1/completions` con la API de OpenAI `text_completion`. Útil para probar con `curl` o integrarlo con otras herramientas.

## Instalación en Zed

1. Renombrá este directorio a `minimax-proxy` (debe coincidir con `id` en `extension.toml`).
2. Copialo a `~/.config/zed/extensions/minimax-proxy/`.
3. En `~/.config/zed/settings.json` agregá la configuración (ver abajo).
4. Reload Zed.

Quedan así:

```
~/.config/zed/extensions/minimax-proxy/
├── extension.toml
├── Cargo.toml
├── rust-toolchain.toml
├── src/lib.rs
├── bin/
│   └── minimax-proxy-server
├── server/
└── …
```

El `.wasm` lo maneja Zed; para dev extensions lo lee de `target/wasm32-wasip2/debug/` (cargo lo regenera en cada reload). No necesitás copiarlo a mano.

## Configuración

### Variables de entorno (env vars)

La extensión hereda el environment del shell al spawnear el server. Estas son las variables reconocidas:

| Variable                        | Default       | Descripción                                       |
| ------------------------------- | ------------- | ------------------------------------------------- |
| `MINIMAX_API_KEY` (requerida)    | —             | API token. Equivalente: `MINIMAX_API_TOKEN`.       |
| `MINIMAX_MODEL`                  | `MiniMax-M3`  | Modelo a invocar.                                  |
| `MINIMAX_MAX_TOKENS`             | `256`         | Tokens máximos de **output**.                      |
| `MINIMAX_MAX_INPUT_CHARS`        | `1_000_000`   | Cap de caracteres de entrada (≈ 250k tokens).       |
| `MINIMAX_TIMEOUT_MS`             | `60000`       | Techo duro del timeout adaptativo.                  |
| `MINIMAX_TIMEOUT_BASE_MS`        | `10000`       | Base del timeout (independiente del tamaño).        |
| `MINIMAX_TIMEOUT_PER_K_TOKENS_MS`| `2000`        | Extra por cada 1k tokens estimados de input.        |
| `MINIMAX_LARGE_LOG_THRESHOLD`    | `2000`        | Chars a partir de los cuales los logs se truncan.  |
| `MINIMAX_LARGE_LOG_HEAD/TAIL`    | `300`         | Cuánto mostrar de cada extremo en logs.            |

### Ejemplo de `settings.json`

```json
{
  "lsp": {
    "minimax-proxy": {
      "env": {
        "MINIMAX_API_KEY": "tu-key",
        "MINIMAX_MODEL": "MiniMax-M3",
        "MINIMAX_MAX_TOKENS": "256",
        "MINIMAX_MAX_INPUT_CHARS": "1000000"
      }
    }
  }
}
```

Alternativa: exportá las vars en tu shell (`~/.zshrc`, `~/.bashrc`, etc.) y Zed las propaga vía `shell_env`.

## Probar el server sin Zed

Con el server corriendo en `:8787`:

```bash
curl -s http://localhost:8787/v1/completions \
  -H "content-type: application/json" \
  -d '{
    "model": "MiniMax-M3",
    "prompt": "<|fim_prefix|>const greet = (name) => <|fim_suffix|>;\n<|fim_middle|>",
    "max_tokens": 64
  }'
```

Sin FIM markers también funciona: el prompt crudo se trata como prefijo y se inserta cursor al final.

### Arrancar el server (Rust)

```bash
MINIMAX_API_KEY=... cargo run --release --manifest-path server/Cargo.toml
```

### Arrancar el server (Bun)

```bash
MINIMAX_API_KEY=... bun docs/server.ts
```

> Las dos implementaciones leen las mismas env vars y devuelven el mismo shape JSON. La de Bun es útil para hot-reload y para experimentar con el prompt antes de tocar el código Rust.

## Estructura del repo

```
.
├── Cargo.toml              # package de la extensión WASM
├── rust-toolchain.toml     # pinea "stable" + target wasm32-wasip2
├── extension.toml          # manifiesto de la extensión Zed
├── src/lib.rs              # entrypoint de la extensión
├── server/                 # server HTTP en Rust
│   ├── Cargo.toml
│   └── src/main.rs
├── docs/server.ts          # server HTTP en TypeScript (Bun)
└── bin/
    └── minimax-proxy-server  # binario precompilado
```

El `.wasm` lo produce `cargo build --target wasm32-wasip2` en `target/wasm32-wasip2/debug/` (debug) o `release/`. Zed lo lee desde ahí; no hace falta commitearlo.

## Cómo funciona el prompt rewrite

Zed envía prompt en formato Qwen FIM:

```
<|fim_prefix|>...código antes...<|fim_suffix|>...código después...<|fim_middle|>
```

El proxy lo reescribe al estilo "Minuet" antes de pegarle a MiniMax:

```
# language: typescript
<contextBeforeCursor>
...código antes...<cursorPosition>
<contextAfterCursor>
...código después...
```

Esto es lo que el modelo recibe en el `user` message del chat completion. Permite que el modelo razone mejor el cursor location y evita que regurgite los marcadores FIM.

## Limitaciones conocidas

- El test `user_stop_takes_priority_over_default` falla en `server/src/main.rs` (preexistente, sin relación con el comportamiento actual).
- La API `zed_extension_api = 0.1.0` no expone UI para settings; se resuelven via `settings.json` + `shell_env`.
- No hay streaming de tokens (toda la respuesta llega de una). Cambiar `stream: false` a `true` en el server rompería el contrato actual de `text_completion`.
