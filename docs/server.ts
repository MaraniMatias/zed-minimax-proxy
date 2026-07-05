const PORT = Number(process.env.PORT ?? 8787);
const MINIMAX_API_KEY = process.env.MINIMAX_API_KEY;

const DEFAULT_MAX_TOKENS = Number(process.env.MINIMAX_MAX_TOKENS ?? 256);
const DEFAULT_TEMPERATURE = Number(process.env.MINIMAX_TEMPERATURE ?? 0.2);
const DEFAULT_TOP_P = Number(process.env.MINIMAX_TOP_P ?? 0.95);

const REQUEST_TIMEOUT_MS = Number(process.env.MINIMAX_TIMEOUT_MS ?? 60000);
const TIMEOUT_BASE_MS = Number(process.env.MINIMAX_TIMEOUT_BASE_MS ?? 10000);
const TIMEOUT_PER_K_TOKENS_MS = Number(process.env.MINIMAX_TIMEOUT_PER_K_TOKENS_MS ?? 2000);

const MAX_INPUT_CHARS = Number(process.env.MINIMAX_MAX_INPUT_CHARS ?? 1_000_000);
const MAX_PREFIX_CHARS = Math.floor(MAX_INPUT_CHARS / 2);
const MAX_SUFFIX_CHARS = MAX_INPUT_CHARS - MAX_PREFIX_CHARS;

const CHARS_PER_TOKEN = 4;

const LARGE_LOG_THRESHOLD = Number(process.env.MINIMAX_LARGE_LOG_THRESHOLD ?? 2000);
const LARGE_LOG_HEAD = Number(process.env.MINIMAX_LARGE_LOG_HEAD ?? 300);
const LARGE_LOG_TAIL = Number(process.env.MINIMAX_LARGE_LOG_TAIL ?? 300);

// Good values: "none", "minimal", "low", "medium", "high".
// For inline completions, I would start with "minimal".
const REASONING_EFFORT = process.env.MINIMAX_REASONING_EFFORT ?? "minimal";

if (!MINIMAX_API_KEY) {
  console.error("Missing MINIMAX_API_KEY");
  console.error("Run: export MINIMAX_API_KEY='your_key'");
  process.exit(1);
}

type CompletionRequest = {
  model?: string;
  prompt?: string;
  max_tokens?: number;
  max_output_tokens?: number;
  temperature?: number;
  top_p?: number;
  stop?: string | string[];
};

function jsonResponse(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: {
      "content-type": "application/json",
    },
  });
}

function preview(value: string, maxLength = 1000): string {
  if (value.length <= maxLength) return value;
  return `${value.slice(0, maxLength)}\n...[truncated ${value.length - maxLength} chars]`;
}

function previewLarge(value: string, headChars = LARGE_LOG_HEAD, tailChars = LARGE_LOG_TAIL): string {
  if (value.length <= LARGE_LOG_THRESHOLD) return value;
  const omitted = value.length - headChars - tailChars;
  return [
    value.slice(0, headChars),
    `...[truncated ${omitted} chars of ${value.length} total]...`,
    value.slice(value.length - tailChars),
  ].join("\n");
}

function head(value: string, maxChars: number): string {
  if (value.length <= maxChars) return value;
  return value.slice(0, maxChars);
}

function tail(value: string, maxChars: number): string {
  if (value.length <= maxChars) return value;
  return value.slice(value.length - maxChars);
}

function computeTimeoutMs(payloadChars: number): number {
  const estTokens = payloadChars / CHARS_PER_TOKEN;
  const kTokens = Math.ceil(estTokens / 1000);
  const adaptive = TIMEOUT_BASE_MS + kTokens * TIMEOUT_PER_K_TOKENS_MS;
  return Math.min(adaptive, REQUEST_TIMEOUT_MS);
}

function parseQwenFimPrompt(prompt: string): { prefix: string; suffix: string } | null {
  const prefixMarker = "<|fim_prefix|>";
  const suffixMarker = "<|fim_suffix|>";
  const middleMarker = "<|fim_middle|>";

  const prefixStart = prompt.indexOf(prefixMarker);
  const suffixStart = prompt.indexOf(suffixMarker);
  const middleStart = prompt.indexOf(middleMarker);

  if (prefixStart === -1 || suffixStart === -1 || middleStart === -1) {
    return null;
  }

  if (!(prefixStart < suffixStart && suffixStart < middleStart)) {
    return null;
  }

  const prefix = prompt.slice(prefixStart + prefixMarker.length, suffixStart);
  const suffix = prompt.slice(suffixStart + suffixMarker.length, middleStart);

  return { prefix, suffix };
}

function guessLanguageFromPrefix(prefix: string): string {
  const lower = prefix.toLowerCase();

  if (lower.includes("export default") || lower.includes("function ") || lower.includes("const ")) {
    return "javascript";
  }

  if (lower.includes("interface ") || lower.includes(": string") || lower.includes(": number")) {
    return "typescript";
  }

  if (lower.includes("def ") || lower.includes("import ")) {
    return "python";
  }

  if (lower.includes("fn ") || lower.includes("let mut")) {
    return "rust";
  }

  if (lower.includes("package main") || lower.includes("func ")) {
    return "go";
  }

  return "unknown";
}

function buildMinuetStyleUserPrompt(originalPrompt: string): string {
  const fim = parseQwenFimPrompt(originalPrompt);

  if (!fim) {
    const prefix = tail(originalPrompt, MAX_PREFIX_CHARS);
    const language = guessLanguageFromPrefix(prefix);

    return [
      `# language: ${language}`,
      "<contextBeforeCursor>",
      `${prefix}<cursorPosition>`,
      "<contextAfterCursor>",
      "",
    ].join("\n");
  }

  const prefix = tail(fim.prefix, MAX_PREFIX_CHARS);
  const suffix = head(fim.suffix, MAX_SUFFIX_CHARS);
  const language = guessLanguageFromPrefix(prefix);

  return [
    `# language: ${language}`,
    "<contextBeforeCursor>",
    `${prefix}<cursorPosition>`,
    "<contextAfterCursor>",
    suffix,
  ].join("\n");
}

function stripRepeatedPrompt(completion: string, originalPrompt: string): string {
  if (completion.startsWith(originalPrompt)) {
    return completion.slice(originalPrompt.length);
  }

  const trimmedPrompt = originalPrompt.trimEnd();
  if (completion.startsWith(trimmedPrompt)) {
    return completion.slice(trimmedPrompt.length);
  }

  const fim = parseQwenFimPrompt(originalPrompt);
  if (fim && completion.startsWith(fim.prefix)) {
    return completion.slice(fim.prefix.length);
  }

  return completion;
}

function removeMarkdownFences(text: string): string {
  return text.replace(/^```[a-zA-Z0-9_-]*\n?/, "").replace(/\n?```$/, "");
}

function removePromptArtifacts(text: string): string {
  return text
    .replaceAll("<|fim_prefix|>", "")
    .replaceAll("<|fim_suffix|>", "")
    .replaceAll("<|fim_middle|>", "")
    .replaceAll("<contextBeforeCursor>", "")
    .replaceAll("<contextAfterCursor>", "")
    .replaceAll("<cursorPosition>", "")
    .replaceAll("<cursor>", "")
    .replaceAll("</cursor>", "");
}

function applyStopSequences(text: string, stop?: string | string[]): string {
  if (!stop) return text;

  const stops = Array.isArray(stop) ? stop : [stop];
  let result = text;

  for (const stopSequence of stops) {
    if (!stopSequence) continue;

    const index = result.indexOf(stopSequence);
    if (index !== -1) {
      result = result.slice(0, index);
    }
  }

  return result;
}

function cutObviousOverGeneration(text: string): string {
  const badBoundaries = [
    "```",
    "<|fim_prefix|>",
    "<|fim_suffix|>",
    "<|fim_middle|>",
    "<contextBeforeCursor>",
    "<contextAfterCursor>",
    "<cursorPosition>",
    "</context>",
  ];

  let result = text;

  for (const boundary of badBoundaries) {
    const index = result.indexOf(boundary);
    if (index !== -1) {
      result = result.slice(0, index);
    }
  }

  return result;
}

function cleanCompletion(rawCompletion: string, originalPrompt: string, stop?: string | string[]): string {
  let completion = rawCompletion;

  completion = stripRepeatedPrompt(completion, originalPrompt);
  completion = removeMarkdownFences(completion);
  completion = removePromptArtifacts(completion);
  completion = applyStopSequences(completion, stop);
  completion = cutObviousOverGeneration(completion);

  return completion.trimEnd();
}

function buildSystemPrompt(): string {
  return [
    "You are an AI code completion engine.",
    "",
    "Task:",
    "Complete code at <cursorPosition> using the surrounding context.",
    "",
    "Input format:",
    "<contextBeforeCursor> contains code before the cursor.",
    "<cursorPosition> is the exact cursor location.",
    "<contextAfterCursor> contains code after the cursor.",
    "",
    "Rules:",
    "1. Return only the text to insert at <cursorPosition>.",
    "2. Preserve exact whitespace and indentation.",
    "3. Do not repeat existing code before or after the cursor.",
    "4. Do not include markdown fences.",
    "5. Do not explain.",
    "6. Do not add comments unless the comment itself is the natural completion.",
    "7. Prefer concise completions: one line or a few lines.",
    "8. Make the completion fit cleanly before <contextAfterCursor>.",
  ].join("\n");
}

async function fetchMiniMaxBody(body: string): Promise<Response> {
  const timeoutMs = computeTimeoutMs(body.length);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);

  try {
    return await fetch("https://api.minimax.io/v1/chat/completions", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${MINIMAX_API_KEY}`,
      },
      body,
      signal: controller.signal,
    });
  } finally {
    clearTimeout(timeout);
  }
}

Bun.serve({
  port: PORT,

  async fetch(req) {
    const url = new URL(req.url);
    const startedAt = Date.now();

    console.log("\n--- Incoming request ---");
    console.log("Time:", new Date().toISOString());
    console.log("Method:", req.method);
    console.log("Path:", url.pathname);

    if (req.method !== "POST" || url.pathname !== "/v1/completions") {
      console.log("Rejected: route not found");
      return jsonResponse({ error: "Not found" }, 404);
    }

    try {
      const body = (await req.json()) as CompletionRequest;

      const prompt = String(body.prompt ?? "");
      const model = String(body.model ?? "MiniMax-M3");
      const maxTokens = Number(body.max_tokens ?? body.max_output_tokens ?? DEFAULT_MAX_TOKENS);
      const temperature = Number(body.temperature ?? DEFAULT_TEMPERATURE);
      const topP = Number(body.top_p ?? DEFAULT_TOP_P);

      const fim = parseQwenFimPrompt(prompt);
      const userPrompt = buildMinuetStyleUserPrompt(prompt);

      console.log("Model:", model);
      console.log("Max tokens:", maxTokens);
      console.log("Temperature:", temperature);
      console.log("Top P:", topP);
      console.log("Reasoning effort:", REASONING_EFFORT);
      console.log("Timeout cap ms:", REQUEST_TIMEOUT_MS);
      console.log("Qwen FIM detected:", Boolean(fim));
      console.log("Prompt length:", prompt.length);
      console.log("Prompt preview:");
      console.log(preview(prompt));

      if (fim) {
        console.log("Prefix length:", fim.prefix.length);
        console.log("Suffix length:", fim.suffix.length);
      }

      console.log("User prompt sent to MiniMax preview:");
      console.log(previewLarge(userPrompt));

      const upstreamPayload = {
        model,
        stream: false,
        messages: [
          {
            role: "system",
            content: buildSystemPrompt(),
          },
          {
            role: "user",
            content: userPrompt,
          },
        ],
        max_tokens: maxTokens,
        temperature,
        top_p: topP,

        // MiniMax-specific knobs.
        // "thinking" disabled avoids verbose hidden thinking output.
        // "reasoning.effort" lets you trade speed for quality.
        thinking: { type: "disabled" },
        reasoning: { effort: REASONING_EFFORT },
      };

      const upstreamBody = JSON.stringify(upstreamPayload);
      const timeoutMs = computeTimeoutMs(upstreamBody.length);

      console.log("Calling MiniMax...");
      console.log("Upstream body chars:", upstreamBody.length);
      console.log("Upstream timeout ms:", timeoutMs);

      const upstream = await fetchMiniMaxBody(upstreamBody);
      const upstreamText = await upstream.text();

      console.log("MiniMax status:", upstream.status);
      console.log("MiniMax response preview:");
      console.log(preview(upstreamText));

      if (!upstream.ok) {
        console.log("Finished with upstream error in", Date.now() - startedAt, "ms");

        return new Response(upstreamText, {
          status: upstream.status,
          headers: {
            "content-type": upstream.headers.get("content-type") ?? "application/json",
          },
        });
      }

      const upstreamJson = JSON.parse(upstreamText);
      const rawCompletion = String(upstreamJson.choices?.[0]?.message?.content ?? "");
      const completion = cleanCompletion(rawCompletion, prompt, body.stop);

      const upstreamUsage = upstreamJson.usage ?? {};

      console.log("Raw completion length:", rawCompletion.length);
      console.log("Raw completion preview:");
      console.log(preview(rawCompletion));

      console.log("Final completion length:", completion.length);
      console.log("Final completion preview:");
      console.log(preview(completion));

      const responseBody = {
        id: "cmpl-minimax-proxy",
        object: "text_completion",
        created: Math.floor(Date.now() / 1000),
        model,
        choices: [
          {
            text: completion,
            index: 0,
            logprobs: null,
            finish_reason: "stop",
          },
        ],
        usage: {
          prompt_tokens: Number(upstreamUsage.prompt_tokens ?? 0),
          completion_tokens: Number(upstreamUsage.completion_tokens ?? 0),
          total_tokens: Number(upstreamUsage.total_tokens ?? 0),
        },
      };

      console.log("Finished OK in", Date.now() - startedAt, "ms");

      return jsonResponse(responseBody);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);

      console.error("Proxy error:", error);
      console.log("Finished with proxy error in", Date.now() - startedAt, "ms");

      return jsonResponse(
        {
          error: {
            message,
            type: "proxy_error",
          },
        },
        500,
      );
    }
  },
});

console.log(`MiniMax completions proxy listening on http://localhost:${PORT}`);
console.log(`Reasoning effort: ${REASONING_EFFORT}`);
console.log(`Default max tokens: ${DEFAULT_MAX_TOKENS}`);
console.log(`Default temperature: ${DEFAULT_TEMPERATURE}`);
console.log(`Max input chars: ${MAX_INPUT_CHARS} (prefix ${MAX_PREFIX_CHARS} / suffix ${MAX_SUFFIX_CHARS})`);
console.log(`Timeout: adaptive base=${TIMEOUT_BASE_MS}ms + ${TIMEOUT_PER_K_TOKENS_MS}ms per 1k est. tokens, cap=${REQUEST_TIMEOUT_MS}ms`);
