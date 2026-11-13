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

interface Model { id?: unknown; median?: unknown; thin?: unknown }

// Trades the gap between the broad market (median price across every seller of the
// benchmark model) and the perp's settlement mark: long when the market sits above the
// mark, short when below, flat once the gap closes. Holds on stale or missing data.
export function basis(env: NodeJS.ProcessEnv, get: FetchJson = fetchJson): HouseStrategy {
  const feed = env.DRIP_FEED_URL?.replace(/\/+$/, "");
  if (!feed) throw new Error("Set DRIP_FEED_URL for the basis agent");
  const model = env.DRIP_BASIS_MODEL || "qwen/qwen3.8-27b";
  const entry = envNumber(env, "DRIP_BASIS_ENTRY", 0.08);
  const exit = envNumber(env, "DRIP_BASIS_EXIT", 0.02);
  const confirm = envNumber(env, "DRIP_BASIS_CONFIRM", 2);
  const maxAgeS = envNumber(env, "DRIP_FEED_MAX_AGE_S", 1800);
  if (exit >= entry) throw new Error("DRIP_BASIS_EXIT must be below DRIP_BASIS_ENTRY");
  if (!Number.isInteger(confirm)) throw new Error("DRIP_BASIS_CONFIRM must be an integer");
  // Consecutive fresh snapshots past the entry threshold on one side.
  let seen: number | undefined, side: Direction = 0, streak = 0;

  return {
    name: "basis",
    house: true,
    async observe({ mark, current, now }) {
      const params = { model, mark, entry, exit, confirm };
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
      const median = row.median;
      if (typeof median !== "number" || !(median > 0) || !Number.isFinite(median)) return hold("median_missing", { feed_age_s: feedAgeS });

      const gap = Math.log(median / mark);
      const want: Direction = gap > entry ? 1 : gap < -entry ? -1 : 0;
      if (dataMs !== seen) {
        streak = want !== 0 && want === side ? streak + 1 : want !== 0 ? 1 : 0;
        side = want;
        seen = dataMs;
      }
      let target = current, reason = "hold";
      if (want !== 0 && want !== current && streak >= confirm) [target, reason] = [want, want === 1 ? "market_above_mark" : "market_below_mark"];
      else if (want !== 0 && want !== current) reason = "awaiting_confirm";
      else if (Math.abs(gap) < exit && current !== 0) [target, reason] = [0, "gap_closed"];
      return { target, inputs: { ...params, median, basis: gap, feed_age_s: feedAgeS, streak, reason } };
    },
  };
}
