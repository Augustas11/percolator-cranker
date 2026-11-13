import { useEffect, useState } from "react";
import type { ComputeFeedClient, ComputeHistory, ComputeLatest, ComputeMarket, MarketModel } from "./client";

export const STALE_MS = 30 * 60_000;
const REFRESH_MS = 60_000;

export interface FeedState {
  latest: ComputeLatest | null;
  history: ComputeHistory | null;
  market: ComputeMarket | null;
  /** First failure of the last refresh, or "". */
  error: string;
  /** Which files failed on the last refresh. */
  failed: { latest: boolean; history: boolean; market: boolean };
  loaded: boolean;
  now: number;
}

const reason = (r: PromiseRejectedResult) => r.reason instanceof Error ? r.reason.message : "price feed request failed";

/** Polls the three feed files every minute. Each is settled on its own: one failed file never hides the others. */
export function useComputeFeed(feed: ComputeFeedClient | null, files: { history?: boolean } = {}): FeedState {
  const wantHistory = files.history ?? true;
  const [state, setState] = useState<FeedState>(() => ({ latest: null, history: null, market: null, error: "", failed: { latest: false, history: false, market: false }, loaded: false, now: Date.now() }));
  useEffect(() => {
    let active = true;
    setState({ latest: null, history: null, market: null, error: "", failed: { latest: false, history: false, market: false }, loaded: false, now: Date.now() });
    if (!feed) return;
    const refresh = () => Promise.allSettled([feed.latest(), wantHistory ? feed.history() : Promise.resolve(null), feed.market()]).then(([l, h, m]) => {
      if (!active) return;
      const failures = [l, h, m].filter((r): r is PromiseRejectedResult => r.status === "rejected");
      setState(prev => ({
        latest: l.status === "fulfilled" ? l.value : prev.latest,
        history: h.status === "fulfilled" ? h.value : prev.history,
        market: m.status === "fulfilled" ? m.value : prev.market,
        error: failures.length ? reason(failures[0]) : "",
        failed: { latest: l.status === "rejected", history: h.status === "rejected", market: m.status === "rejected" },
        loaded: true,
        now: Date.now(),
      }));
    });
    void refresh();
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => { active = false; clearInterval(timer); };
  }, [feed, wantHistory]);
  return state;
}

/** Why latest.json is stale, or null. Same rules as FEED.md. */
export function latestStale(latest: ComputeLatest | null, now: number) {
  return !latest ? null
    : now - latest.generated_ms > STALE_MS ? "The price feed has not updated for over 30 minutes."
    : latest.landed_ms > latest.generated_ms + 5 * 60_000 ? "The latest price has a time in the future."
    : latest.generated_ms - latest.landed_ms > STALE_MS ? "No new price for over 30 minutes." : null;
}

/** When the market's newest data was read: `data_ms`, or `generated_ms` from feeds that predate the field. Null when the feed says it has none. */
export function marketDataTime(market: ComputeMarket) {
  return market.data_ms === undefined ? market.generated_ms : market.data_ms;
}

/** Why market.json is stale, or null. Judged on the newest data read, not on when the file was written. */
export function marketStale(market: ComputeMarket | null, now: number) {
  if (!market) return null;
  const t = marketDataTime(market);
  return t === null || !Number.isFinite(t) ? "The market feed has no recent read."
    : now - t > STALE_MS ? "No new prices read for over 30 minutes."
    : t > now + 5 * 60_000 ? "The market feed has a time in the future." : null;
}

/** A model whose quotes are not fresh: flagged by the feed, or its newest read is over 30 minutes old. */
export function modelStale(model: Pick<MarketModel, "stale" | "data_ms">, now: number) {
  return model.stale === true || (typeof model.data_ms === "number" && now - model.data_ms > STALE_MS) || model.data_ms === null;
}
