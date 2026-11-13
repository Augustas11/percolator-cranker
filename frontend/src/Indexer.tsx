import { useEffect, useState } from "react";
import { IndexerHttpError } from "./indexer/client";
import type { Fills, IndexerClient, Positions, RecentFills, Season, Stats, Trades } from "./indexer/client";
import { devnetConfig } from "./chain/config";
import { PERP, perpPrice } from "./market/types";

const numberFormatter = new Intl.NumberFormat("en-US", { maximumFractionDigits: 2 });
export const number = (value: number) => Number.isFinite(value) ? numberFormatter.format(value) : "—";
export const fixed = (value: string) => {
  try { return Number(BigInt(value)) / 1e6; } catch { return NaN; }
};
export const UNREACHABLE = "The indexer is unreachable right now. Try again in a minute.";
export const OTHER_MARKET = `The indexer reads a different market, not ${PERP.name}.`;

const usdFormat = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });

/**
 * Open interest as text: USD and units when both are above zero, either one alone when only it is, null when neither.
 * The indexer's USD figure can read 0 while units are open (no price at the last change), so a zero USD is left out.
 */
export function openInterestText(usdAtoms: string | null | undefined, units: number | null | undefined) {
  const usd = usdAtoms == null ? NaN : fixed(usdAtoms);
  const u = units ?? NaN;
  const parts = [Number.isFinite(usd) && usd > 0 ? usdFormat.format(usd) : null, Number.isFinite(u) && u > 0 ? `${number(u)} ${PERP.unitShort}` : null].filter(Boolean);
  return parts.length ? parts.join(" · ") : null;
}

/** Open units on the market's first asset from /stats, or null. */
export function openUnits(stats: Stats | null | undefined) {
  const oi = stats ? Object.values(stats.open_interest ?? {})[0] : undefined;
  const u = oi ? fixed(oi.long_q) : NaN;
  return Number.isFinite(u) ? u : null;
}

/**
 * The same-market guard: reads /stats every 30 s and hands back the indexer only while it serves this market.
 * `indexer` is null until the first read confirms the market, and stays null when it reads another one.
 */
export function useMarketGuard(indexer: IndexerClient | null, market = devnetConfig.market.toBase58()) {
  const [stats, setStats] = useState<Stats | null>(null);
  useEffect(() => {
    let active = true;
    setStats(null);
    if (!indexer) return;
    const run = async () => { try { const s = await indexer.stats(); if (active) setStats(s); } catch { /* keep the last read */ } };
    void run();
    const timer = setInterval(() => void run(), 30_000);
    return () => { active = false; clearInterval(timer); };
  }, [indexer]);
  const same = stats !== null && stats.market === market;
  return { stats: same ? stats : null, wrongMarket: stats !== null && !same, indexer: same ? indexer : null };
}

export type OwnFill = Fills["fills"][number] & { portfolio: string };

export interface TradeIndexer {
  stats: Stats | null;
  season: Season | null;
  recent: RecentFills["fills"] | null;
  /** Market-wide executed fills from /trades; null until read, or when the indexer predates the endpoint. */
  trades: Trades["trades"] | null;
  fills: OwnFill[] | null;
  /** The wallet's portfolios as the indexer last saw them (capital, PnL, positions). */
  portfolios: Positions["portfolios"] | null;
  moreHistory: boolean;
  statsError: string;
  fillsError: string;
  /** False when the indexer serves another market: its numbers must not be shown as this market's. */
  sameMarket: boolean | null;
}

/** Everything the trade screen reads from the indexer, polled every 15 s; each part keeps its last good value. */
export function useTradeIndexer(indexer: IndexerClient | null, owner: string | null, market = devnetConfig.market.toBase58()): TradeIndexer {
  const [stats, setStats] = useState<Stats | null>(null);
  const [season, setSeason] = useState<Season | null>(null);
  const [recent, setRecent] = useState<RecentFills["fills"] | null>(null);
  const [trades, setTrades] = useState<Trades["trades"] | null>(null);
  const [fills, setFills] = useState<OwnFill[] | null>(null);
  const [portfolios, setPortfolios] = useState<Positions["portfolios"] | null>(null);
  const [moreHistory, setMoreHistory] = useState(false);
  const [statsError, setStatsError] = useState("");
  const [fillsError, setFillsError] = useState("");

  useEffect(() => {
    let active = true;
    let pending = false;
    let tradesGone = false;
    setStats(null); setSeason(null); setRecent(null); setTrades(null); setFills(null); setPortfolios(null);
    setMoreHistory(false); setStatsError(""); setFillsError("");
    if (!indexer) return;
    const message = (error: unknown) => error instanceof Error ? error.message : "indexer request failed";
    async function refresh() {
      if (pending) return;
      pending = true;
      await Promise.all([
        (async () => {
          try {
            const result = await indexer!.stats();
            if (active) { setStats(result); setStatsError(""); }
          } catch (error) { if (active) setStatsError(message(error)); }
        })(),
        (async () => {
          try {
            const result = await indexer!.season?.();
            if (active && result) setSeason(result);
          } catch { /* volume is optional; keep the last value */ }
        })(),
        (async () => {
          try {
            const result = await indexer!.recentFills?.(30);
            if (active && result) setRecent(result.fills);
          } catch { /* keep the last trades */ }
        })(),
        (async () => {
          // Newer indexers only: an older one answers 404, and the Trades tab keeps the recent-fills feed.
          if (tradesGone || !indexer!.trades) return;
          try {
            const result = await indexer!.trades(30);
            if (active) setTrades(result.trades);
          } catch (error) { if (error instanceof IndexerHttpError && error.status === 404) tradesGone = true; }
        })(),
        (async () => {
          if (!owner) return;
          try {
            const positions = await indexer!.positions(owner);
            const histories = await Promise.all(positions.portfolios.map(async p => ({ ...await indexer!.fills(p.portfolio), portfolio: p.portfolio })));
            const unique = new Map(histories.flatMap(h => h.fills.map(f => ({ ...f, portfolio: h.portfolio }))).map(f => [`${f.portfolio}:${f.signature}:${f.ix_index}:${f.leg}`, f]));
            if (active) { setPortfolios(positions.portfolios); setFills([...unique.values()].sort((a, b) => b.slot - a.slot)); setMoreHistory(histories.some(h => h.next_before !== null)); setFillsError(""); }
          } catch (error) { if (active) setFillsError(message(error)); }
        })(),
      ]);
      pending = false;
    }
    void refresh();
    const timer = setInterval(() => void refresh(), 15000);
    return () => { active = false; clearInterval(timer); };
  }, [indexer, owner]);

  return { stats, season, recent, trades, fills, portfolios, moreHistory, statsError, fillsError, sameMarket: stats ? stats.market === market : null };
}

/** Signed size (units) and price of an executed fill, from the newer exact fields; null when the indexer did not send them. */
export function executed(fill: OwnFill) {
  if (fill.status !== "executed" || fill.executed_size == null || fill.applied_price == null || !Number.isSafeInteger(fill.applied_price)) return null;
  const size = fixed(fill.executed_size);
  return Number.isFinite(size) && size !== 0 && fill.applied_price > 0 ? { size, price: fill.applied_price / 1e6 } : null;
}

/**
 * Entry price of the current position from the wallet's own executed fills: the size-weighted average of the fills
 * that built it (reductions keep the average, a flip starts a new one). Null unless it can be derived exactly: the
 * full history is loaded, every executed fill carries its exact size and price, and one portfolio's fills add up to
 * the open position. `position` is signed units (long > 0).
 */
export function entryFromFills(fills: OwnFill[] | null, position: number, complete: boolean) {
  if (!fills || !complete || !Number.isFinite(position) || position === 0) return null;
  const byPortfolio = new Map<string, OwnFill[]>();
  for (const f of fills) if (f.status === "executed") byPortfolio.set(f.portfolio, [...byPortfolio.get(f.portfolio) ?? [], f]);
  for (const list of byPortfolio.values()) {
    let pos = 0, avg = 0, exact = true;
    for (const f of [...list].sort((a, b) => a.slot - b.slot || a.ix_index - b.ix_index || a.leg - b.leg)) {
      const x = executed(f);
      if (!x) { exact = false; break; }
      if (pos === 0 || Math.sign(pos) === Math.sign(x.size)) avg = (avg * Math.abs(pos) + x.price * Math.abs(x.size)) / (Math.abs(pos) + Math.abs(x.size));
      else if (Math.abs(x.size) > Math.abs(pos)) avg = x.price;
      pos += x.size;
      if (Math.abs(pos) < 1e-9) { pos = 0; avg = 0; }
    }
    if (exact && Math.abs(pos - position) < 1e-6) return avg;
  }
  return null;
}

/** The connected wallet's own indexed fills. */
export function OwnFills({ data, connected, reachable }: { data: TradeIndexer; connected: boolean; reachable: boolean }) {
  if (!reachable) return <p className="tx-empty" role="status">{UNREACHABLE}</p>;
  if (data.sameMarket === false) return <p className="tx-empty" role="status">{OTHER_MARKET}</p>;
  if (!connected) return <p className="tx-empty">Connect a wallet to see your fills.</p>;
  return <>
    {data.fillsError && <p className="tx-empty" role="status">{data.fillsError}</p>}
    {data.fills === null && !data.fillsError && <p className="tx-empty" aria-busy="true">Loading your fills.</p>}
    {data.fills !== null && data.fills.length === 0 && !data.fillsError && <p className="tx-empty">No fills yet.</p>}
    {data.fills !== null && data.fills.length > 0 && <div className="table-scroll"><table className="tx-table">
      <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col" className="num">Size ({PERP.unitShort})</th><th scope="col" className="num">Price</th><th scope="col">Status</th><th scope="col">Tx</th></tr></thead>
      <tbody>{data.fills.map(fill => <tr key={`${fill.portfolio}:${fill.signature}:${fill.ix_index}:${fill.leg}`}>
        <td>{fill.block_time === null ? "—" : new Date(fill.block_time * 1000).toLocaleString()}</td>
        <td className={fill.side === "buy" ? "up" : "down"}>{fill.side === "buy" ? "long" : "short"}</td><td className="num">{number(Math.abs(executed(fill)?.size ?? fixed(fill.size_q)))}</td>
        <td className="num">{executed(fill) ? perpPrice(executed(fill)!.price) : fill.mark_price === null || !Number.isSafeInteger(fill.mark_price) ? "—" : perpPrice(fill.mark_price / 1e6)}</td><td>{fill.status}</td>
        <td><a href={`https://explorer.solana.com/tx/${encodeURIComponent(fill.signature)}?cluster=devnet`} target="_blank" rel="noreferrer">{fill.signature.slice(0, 7)}…{fill.signature.slice(-4)}</a></td>
      </tr>)}</tbody>
    </table></div>}
    {data.moreHistory && <p className="tx-empty">Older fills are on the explorer.</p>}
  </>;
}
