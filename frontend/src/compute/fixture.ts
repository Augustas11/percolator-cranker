import type { ComputeMarket, MarketChange, MarketDynamic, MarketModel, MarketProvider, MarketQuote, Point, Routed } from "./client";

// A typed market.json for tests and `npm run dev` without the devnet feed.
// Deterministic: the same `now` always gives the same file.

const HOUR = 3_600_000;

const PROVIDERS = [
  "DeepInfra", "Together", "Fireworks", "Novita", "Parasail", "Chutes", "Nebius", "Baseten", "Groq", "Cerebras",
  "SambaNova", "GMICloud", "Lambda", "Hyperbolic", "Crusoe", "Nscale", "Friendli", "InferenceNet", "Kluster", "Mancer",
  "AtlasCloud", "SiliconFlow", "Phala", "Targon", "Venice", "Darkbloom", "Near AI", "Ionstream", "Modal", "DekaLLM",
  "Wafer", "StreamLake", "Tenstorrent", "Cloudflare", "BaseRun", "Enfer", "Featherless", "NextBit", "Avian", "Relace",
  "Morph", "Infermatic", "Liquid", "Inflection", "Ubicloud", "Alibaba", "Z.ai", "Moonshot",
];
const QUANTS = ["bf16", "fp8", "fp8", "fp8", "fp4", "unknown", "fp16", "int8"];

// [id, name, family, sellers, cheapest, dearest]
const MODELS: [string, string, string, number, number, number][] = [
  ["z-ai/glm-5.3", "GLM 5.3", "GLM", 46, 0.15, 3.18],
  ["deepseek/deepseek-v4", "DeepSeek V4", "DeepSeek", 31, 0.21, 2.4],
  ["openai/gpt-oss-120b", "GPT-OSS-120B", "GPT-OSS", 26, 0.04, 0.38],
  ["qwen/qwen3.8-27b", "Qwen3.8-27B", "Qwen", 22, 0.115, 1.02],
  ["moonshotai/kimi-k3", "Kimi K3", "Kimi", 21, 0.52, 3.6],
  ["qwen/qwen3.8-235b-a22b", "Qwen3.8-235B-A22B", "Qwen", 20, 0.16, 1.4],
  ["meta-llama/llama-4.2-70b", "Llama 4.2 70B", "Llama", 19, 0.12, 0.92],
  ["minimax/minimax-m3", "MiniMax M3", "MiniMax", 17, 0.28, 1.55],
  ["deepseek/deepseek-r3", "DeepSeek R3", "DeepSeek", 16, 0.42, 2.9],
  ["openai/gpt-oss-20b", "GPT-OSS-20B", "GPT-OSS", 16, 0.03, 0.2],
  ["google/gemma-4-27b", "Gemma 4 27B", "Gemma", 14, 0.08, 0.5],
  ["qwen/qwen3.8-coder-480b", "Qwen3.8 Coder 480B", "Qwen", 13, 0.3, 2.1],
  ["nvidia/nemotron-ultra-4", "Nemotron Ultra 4", "Nemotron", 11, 0.45, 1.9],
  ["meta-llama/llama-4.2-405b", "Llama 4.2 405B", "Llama", 10, 0.8, 3.4],
  ["mistralai/mistral-large-4", "Mistral Large 4", "Mistral", 9, 0.6, 2.2],
  ["google/gemma-4-9b", "Gemma 4 9B", "Gemma", 8, 0.02, 0.11],
  ["qwen/qwen3.8-8b", "Qwen3.8-8B", "Qwen", 8, 0.025, 0.14],
  ["mistralai/mistral-small-4", "Mistral Small 4", "Mistral", 6, 0.05, 0.3],
  ["nvidia/nemotron-nano-4", "Nemotron Nano 4", "Nemotron", 4, 0.03, 0.12],
  ["meta-llama/llama-4.2-8b", "Llama 4.2 8B", "Llama", 3, 0.02, 0.06],
];

function rng(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const round = (n: number) => Math.round(n * 1e6) / 1e6;

/** Quantile by linear interpolation over sorted values. */
export function quantile(sorted: number[], q: number) {
  if (!sorted.length) return NaN;
  const i = (sorted.length - 1) * q, lo = Math.floor(i), hi = Math.ceil(i);
  return sorted[lo] + (sorted[hi] - sorted[lo]) * (i - lo);
}

/** The routed price by the feed's rule: up sellers weighted 1 / price² times uptime, no seller above half the weight. */
export function routedFrom(quotes: MarketQuote[]): Omit<Routed, "change_1h_pct" | "change_24h_pct" | "series"> | null {
  const up = quotes.filter((q): q is MarketQuote & { price: number } => q.up && q.price !== null && q.price > 0);
  if (!up.length) return null;
  let w = up.map(q => (q.uptime_24h_pct ?? 100) / 100 / (q.price * q.price));
  let capped = false;
  // Cap at half and hand the excess to the rest, until nobody is over (one seller alone keeps everything).
  for (let k = 0; k < up.length && up.length > 1; k++) {
    const total = w.reduce((a, b) => a + b, 0), over = w.findIndex(x => x / total > 0.5 + 1e-12);
    if (over < 0) break;
    capped = true;
    const rest = total - w[over];
    w = w.map((x, i) => i === over ? rest : x);
  }
  const total = w.reduce((a, b) => a + b, 0);
  const shares = w.map(x => x / total);
  const price = up.reduce((a, q, i) => a + q.price * shares[i], 0);
  const top = up.map((q, i) => ({ provider: q.provider, quantization: q.quantization, price: q.price, share: round(shares[i]) }))
    .sort((a, b) => b.share - a.share).slice(0, 5);
  return { price: round(price), sellers: up.length, top, capped, uptime_fallback: false };
}

/** USD per billion G_n per USD per 1M tokens for the benchmark model (the live feed's ratio for Qwen3.8-27B). */
export const BENCH_GN_FACTOR = 19.2309;
export const FLOP_RATE = 0.00291;

export function marketFixture(now = Date.now()): ComputeMarket {
  const rand = rng(11);
  const base = now - 20 * HOUR;
  const models: MarketModel[] = MODELS.map(([id, name, family, n, lo, hi], m) => {
    const start = Math.floor(rand() * PROVIDERS.length);
    const quotes: MarketQuote[] = Array.from({ length: n }, (_, i) => {
      // Log-uniform between the ends, skewed toward the cheap side, with the two ends pinned.
      const u = i === 0 ? 0 : i === n - 1 ? 1 : Math.pow(rand(), 1.7);
      const price = round(lo * Math.pow(hi / lo, u));
      const up = i < 2 || rand() > 0.07;
      return {
        provider: PROVIDERS[(start + i * 7) % PROVIDERS.length],
        quantization: i === 1 ? "bf16" : QUANTS[Math.floor(rand() * QUANTS.length)],
        price: m === 5 && i === 4 ? null : price,
        up,
        uptime_24h_pct: up ? round(90 + rand() * 10) : round(40 + rand() * 40),
      };
    }).sort((a, b) => (a.price ?? Infinity) - (b.price ?? Infinity));
    const live = quotes.filter(q => q.up && q.price !== null).map(q => q.price!).sort((a, b) => a - b);
    const cheap = quotes.find(q => q.up && q.price !== null)!;
    const median = round(quantile(live, 0.5));
    return {
      id, name, family, sellers_total: n, sellers_up: quotes.filter(q => q.up).length,
      median, p25: round(quantile(live, 0.25)), p75: round(quantile(live, 0.75)), min: live[0], max: live[live.length - 1],
      ...(n < 5 ? { thin: true } : {}),
      cheapest: { provider: cheap.provider, quantization: cheap.quantization, price: cheap.price! },
      change_24h_pct: null,
      series: [[base, round(median * (1 + (rand() - 0.5) * 0.04))], [now, median]] as Point[],
      quotes,
    };
  });
  // Routed price per model, with a 4 h series at 15 min steps ending at the current value.
  for (const model of models) {
    const r = routedFrom(model.quotes);
    if (!r) continue;
    const series: Point[] = Array.from({ length: 17 }, (_, i) => [now - (16 - i) * 15 * 60_000, i === 16 ? r.price : round(r.price * (1 + Math.sin(i * 1.3 + model.median) * 0.012))]);
    const hourAgo = series[12][1];
    model.routed = { ...r, change_1h_pct: round((r.price / hourAgo - 1) * 100), change_24h_pct: null, series };
    if (model.id === "qwen/qwen3.8-27b") model.routed_usd_per_bn_gn = round(r.price * BENCH_GN_FACTOR);
  }
  const bench = models.find(m => m.id === "qwen/qwen3.8-27b")!;
  const listGn = bench.median * BENCH_GN_FACTOR, routedGn = bench.routed!.price * BENCH_GN_FACTOR;

  const byProvider = new Map<string, { models: Set<string>; quotes: number; uptime: number[]; cheapest: number }>();
  for (const model of models) for (const q of model.quotes) {
    const p = byProvider.get(q.provider) ?? { models: new Set(), quotes: 0, uptime: [], cheapest: 0 };
    p.models.add(model.id); p.quotes++;
    if (q.uptime_24h_pct !== null) p.uptime.push(q.uptime_24h_pct);
    if (model.cheapest?.provider === q.provider && model.cheapest?.quantization === q.quantization) p.cheapest++;
    byProvider.set(q.provider, p);
  }
  const providers: MarketProvider[] = [...byProvider].map(([name, p]) => ({
    name, models: p.models.size, quotes: p.quotes, cheapest_in: p.cheapest,
    uptime_24h_pct: p.uptime.length ? round(p.uptime.reduce((a, b) => a + b, 0) / p.uptime.length) : null,
  })).sort((a, b) => b.models - a.models || b.quotes - a.quotes);

  const changes: MarketChange[] = Array.from({ length: 9 }, (_, i) => {
    const model = models[(i * 3) % models.length], q = model.quotes[(i * 5) % model.quotes.length];
    const to = q.price ?? model.median;
    return { at_ms: now - (i * 47 + 6) * 60_000, model: model.name, provider: q.provider, quantization: q.quantization, from: round(to * (i % 3 === 0 ? 1.12 : 0.9)), to };
  });
  const dynamic: MarketDynamic[] = [models[0], models[4], models[1]].map((model, i) => {
    const q = model.quotes[3 + i];
    return { model: model.name, provider: q.provider, quantization: q.quantization, min: round((q.price ?? model.median) * 0.82), max: round((q.price ?? model.median) * 1.21), changes_24h: [212, 96, 31][i] };
  });

  const nQuotes = models.reduce((a, m) => a + m.sellers_total, 0);
  return {
    generated_ms: now - 60_000,
    window: { from_ms: base, to_ms: now },
    index: {
      level: 100.42, base_ms: base, change_24h_pct: null, change_7d_pct: null,
      n_models: models.length, n_quotes: nQuotes, n_quotes_up: models.reduce((a, m) => a + m.sellers_up, 0), n_providers: providers.length,
      series: [[base, 100], [now, 100.42]],
    },
    models, providers, changes, dynamic,
    routed: { benchmark: bench.id, price: bench.routed!.price, usd_per_bn_gn: round(routedGn), series: bench.routed!.series },
    flop: {
      basis_model: bench.id, gn_price_flop: FLOP_RATE,
      implied_flop_usd: listGn / 1e9 / FLOP_RATE, implied_flop_usd_routed: routedGn / 1e9 / FLOP_RATE,
      parity: { market_usd_per_bn_gn: round(routedGn), flop_rate_flop_per_gn: FLOP_RATE, implied_flop_usd: listGn / 1e9 / FLOP_RATE, implied_flop_usd_routed: routedGn / 1e9 / FLOP_RATE, as_of_ms: now - 60_000 },
    },
  };
}
