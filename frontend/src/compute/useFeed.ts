import { useEffect, useState } from "react";
import type { ComputeFeedClient, ComputeHistory, ComputeLatest, ComputeMarket } from "./client";

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

/** Why market.json is stale, or null. */
export function marketStale(market: ComputeMarket | null, now: number) {
  return !market ? null
    : now - market.generated_ms > STALE_MS ? "The market feed has not updated for over 30 minutes."
    : market.generated_ms > now + 5 * 60_000 ? "The market feed has a time in the future." : null;
}
