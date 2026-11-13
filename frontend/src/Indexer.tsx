import { useEffect, useState } from "react";
import type { Fills, IndexerClient, RecentFills, Season, Stats } from "./indexer/client";
import { devnetConfig } from "./chain/config";
import { PERP, perpPrice } from "./market/types";

const numberFormatter = new Intl.NumberFormat("en-US", { maximumFractionDigits: 2 });
export const number = (value: number) => Number.isFinite(value) ? numberFormatter.format(value) : "—";
export const fixed = (value: string) => {
  try { return Number(BigInt(value)) / 1e6; } catch { return NaN; }
};
export const UNREACHABLE = "The indexer is unreachable right now. Try again in a minute.";
export const OTHER_MARKET = `The indexer reads a different market, not ${PERP.name}.`;

export type OwnFill = Fills["fills"][number] & { portfolio: string };

export interface TradeIndexer {
  stats: Stats | null;
  season: Season | null;
  recent: RecentFills["fills"] | null;
  fills: OwnFill[] | null;
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
  const [fills, setFills] = useState<OwnFill[] | null>(null);
  const [moreHistory, setMoreHistory] = useState(false);
  const [statsError, setStatsError] = useState("");
  const [fillsError, setFillsError] = useState("");

  useEffect(() => {
    let active = true;
    let pending = false;
    setStats(null); setSeason(null); setRecent(null); setFills(null);
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
          if (!owner) return;
          try {
            const positions = await indexer!.positions(owner);
            const histories = await Promise.all(positions.portfolios.map(async p => ({ ...await indexer!.fills(p.portfolio), portfolio: p.portfolio })));
            const unique = new Map(histories.flatMap(h => h.fills.map(f => ({ ...f, portfolio: h.portfolio }))).map(f => [`${f.portfolio}:${f.signature}:${f.ix_index}:${f.leg}`, f]));
            if (active) { setFills([...unique.values()].sort((a, b) => b.slot - a.slot)); setMoreHistory(histories.some(h => h.next_before !== null)); setFillsError(""); }
          } catch (error) { if (active) setFillsError(message(error)); }
        })(),
      ]);
      pending = false;
    }
    void refresh();
    const timer = setInterval(() => void refresh(), 15000);
    return () => { active = false; clearInterval(timer); };
  }, [indexer, owner]);

  return { stats, season, recent, fills, moreHistory, statsError, fillsError, sameMarket: stats ? stats.market === market : null };
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
        <td className={fill.side === "buy" ? "up" : "down"}>{fill.side === "buy" ? "long" : "short"}</td><td className="num">{number(Math.abs(fixed(fill.size_q)))}</td>
        <td className="num">{fill.mark_price === null || !Number.isSafeInteger(fill.mark_price) ? "—" : perpPrice(fill.mark_price / 1e6)}</td><td>{fill.status}</td>
        <td><a href={`https://explorer.solana.com/tx/${encodeURIComponent(fill.signature)}?cluster=devnet`} target="_blank" rel="noreferrer">{fill.signature.slice(0, 7)}…{fill.signature.slice(-4)}</a></td>
      </tr>)}</tbody>
    </table></div>}
    {data.moreHistory && <p className="tx-empty">Older fills are on the explorer.</p>}
  </>;
}
