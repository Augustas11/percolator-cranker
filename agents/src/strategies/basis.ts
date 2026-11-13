import type { Direction, HouseStrategy } from "./types";

type FetchJson = (url: string) => Promise<unknown>;

const fetchJson: FetchJson = async url => {
  const response = await fetch(url, { signal: AbortSignal.timeout(10_000) });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
};

export function envNumber(env: NodeJS.ProcessEnv, key: string, fallback: number): number {
  const value = Number(env[key] ?? fallback);
  if (!Number.isFinite(value) || value <= 0) throw new Error(`Invalid ${key}`);
  return value;
}

// Matcher limit on COMPUTE-PERP (perps_markets: matcher.max_fill), in 1M-token units.
export const MAX_FILL = 100;

interface Model { id?: unknown; median?: unknown; thin?: unknown; routed?: { price?: unknown } | null }

// Trades the gap between the broad market and the perp's settlement mark,
// basis = ln(price / mark), where price is the benchmark model's all-seller
// median or its routed price. The gap has a standing level (the broad market
// includes cheaper, lower-precision sellers than the mark), so it trades the
// deviation from its own rolling mean: long when the broad market rises above
// its usual gap to the mark, short when it falls below, flat once the deviation
// is back near zero. Holds on stale, thin or missing data, and until it has
// enough history for the mean.
export function basis(env: NodeJS.ProcessEnv, get: FetchJson = fetchJson): HouseStrategy {
  const feed = env.DRIP_FEED_URL?.replace(/\/+$/, "");
  if (!feed) throw new Error("Set DRIP_FEED_URL for the basis agent");
  const source = env.DRIP_BASIS_SOURCE || "median";
  if (source !== "median" && source !== "routed") throw new Error("DRIP_BASIS_SOURCE must be median or routed");
  const model = env.DRIP_BASIS_MODEL || "qwen/qwen3.8-27b";
  // The median flickers between neighbouring sellers; the routed price is smoother.
  const entry = envNumber(env, "DRIP_BASIS_ENTRY", source === "median" ? 0.08 : 0.02);
  const exit = envNumber(env, "DRIP_BASIS_EXIT", source === "median" ? 0.02 : 0.005);
  const confirm = envNumber(env, "DRIP_BASIS_CONFIRM", 2);
  const windowMs = envNumber(env, "DRIP_BASIS_WINDOW_H", 6) * 3_600_000;
  const minSamples = envNumber(env, "DRIP_BASIS_MIN_SAMPLES", 12);
  const maxAgeS = envNumber(env, "DRIP_FEED_MAX_AGE_S", 1800);
  if (exit >= entry) throw new Error("DRIP_BASIS_EXIT must be below DRIP_BASIS_ENTRY");
  if (!Number.isInteger(confirm) || !Number.isInteger(minSamples)) throw new Error("DRIP_BASIS_CONFIRM and DRIP_BASIS_MIN_SAMPLES must be integers");
  if (envNumber(env, "SIZE_SOL", 0.5) > MAX_FILL) throw new Error(`SIZE_SOL must fit one matcher fill (${MAX_FILL})`);
  // One sample per new feed snapshot (by data_ms), kept for the rolling window.
  const samples: { t: number; basis: number }[] = [];
  let side: Direction = 0, streak = 0;

  return {
    name: "basis",
    house: true,
    async observe({ mark, current, now }) {
      const params = { source, model, mark, entry, exit, confirm };
      const hold = (reason: string, fields: Record<string, unknown> = {}) =>
        ({ target: current, inputs: { ...params, ...fields, reason } });
      let market: { data_ms?: unknown; models?: unknown };
      try {
        market = await get(`${feed}/market.json`) as typeof market ?? {};
      } catch {
        return hold("feed_unreachable");
      }
      const dataMs = market.data_ms;
      if (typeof dataMs !== "number" || !Number.isFinite(dataMs)) return hold("feed_no_time");
      const feedAgeS = Math.round((now - dataMs) / 1000);
      if (!(feedAgeS <= maxAgeS)) return hold("feed_stale", { feed_age_s: feedAgeS });
      const row = Array.isArray(market.models) ? (market.models as Model[]).find(m => m?.id === model) : undefined;
      if (!row) return hold("model_missing", { feed_age_s: feedAgeS });
      if (row.thin === true) return hold("model_thin", { feed_age_s: feedAgeS });
      const price = source === "median" ? row.median : row.routed?.price;
      if (typeof price !== "number" || !(price > 0) || !Number.isFinite(price)) return hold("price_missing", { feed_age_s: feedAgeS });

      const gap = Math.log(price / mark);
      const prior = samples.filter(s => s.t < dataMs && s.t > dataMs - windowMs);
      const isNew = !samples.some(s => s.t === dataMs);
      if (isNew) {
        samples.push({ t: dataMs, basis: gap });
        while (samples.length && samples[0].t <= dataMs - windowMs) samples.shift();
      }
      const fields = { price, basis: gap, feed_age_s: feedAgeS, samples: prior.length };
      if (prior.length < minSamples) return hold("warming_up", fields);
      const mean = prior.reduce((sum, s) => sum + s.basis, 0) / prior.length;
      const deviation = gap - mean;
      const want: Direction = deviation > entry ? 1 : deviation < -entry ? -1 : 0;
      if (isNew) {
        streak = want !== 0 && want === side ? streak + 1 : want !== 0 ? 1 : 0;
        side = want;
      }
      let target = current, reason = "hold";
      if (want !== 0 && want !== current && streak >= confirm) [target, reason] = [want, want === 1 ? "market_above_usual" : "market_below_usual"];
      else if (want !== 0 && want !== current) reason = "awaiting_confirm";
      else if (Math.abs(deviation) < exit && current !== 0) [target, reason] = [0, "gap_back_to_usual"];
      return { target, inputs: { ...params, ...fields, mean, deviation, streak, reason } };
    },
  };
}
