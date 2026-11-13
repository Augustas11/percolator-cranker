import { useEffect, useState } from "react";
import Leaderboard, { usdAtoms } from "../arena/Leaderboard";
import { IndexerHttpError } from "../indexer/client";
import type { IndexerClient, Season, Stats } from "../indexer/client";
import { devnetConfig } from "../chain/config";
import { fixed, OTHER_MARKET, openInterestText, openUnits } from "../Indexer";
import { PERP, perpPrice } from "../market/types";
import { site } from "../site/config";
import { Link } from "../site/router";
import { Arrow } from "../site/Nav";

const ago = (time: number | null) => {
  if (time === null) return "";
  const s = Math.max(0, Math.floor(Date.now() / 1000) - time);
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.floor(s / 60)}m ago` : s < 86400 ? `${Math.floor(s / 3600)}h ago` : `${Math.floor(s / 86400)}d ago`;
};
const short = (wallet: string) => wallet.length > 8 ? `${wallet.slice(0, 4)}…${wallet.slice(-4)}` : wallet;

/** The season line from site config; each part only when set. Null when nothing is set. */
export function seasonLine() {
  const { season, startsAt, endsAt, prizes } = site.arena;
  const parts = [
    season !== null ? `Season ${season}` : null,
    startsAt && endsAt ? `${startsAt} to ${endsAt}` : startsAt ? `From ${startsAt}` : endsAt ? `Until ${endsAt}` : null,
    prizes ? `Prizes: ${prizes}` : null,
  ].filter((p): p is string => p !== null);
  return parts.length ? parts.join(" · ") : null;
}

type FillRow = { key: string; signature: string; time: number | null; side: "buy" | "sell"; size: number; price: number | null; wallet: string };

/** Market-wide fills: /trades when the indexer has it, else the older recent-fills feed. */
async function loadFills(i: IndexerClient): Promise<FillRow[]> {
  if (i.trades) {
    try {
      const { trades } = await i.trades(10);
      return trades.map(t => ({ key: `${t.signature}:${t.ix_index}:${t.leg}`, signature: t.signature, time: t.block_time, side: t.side, size: Math.abs(fixed(t.size)), price: t.price === null ? null : t.price / 1e6, wallet: t.wallet ?? t.taker_portfolio }));
    } catch (e) {
      if (!(e instanceof IndexerHttpError && e.status === 404)) throw e;
    }
  }
  const recent = await i.recentFills?.(10);
  return (recent?.fills ?? []).map(f => ({ key: `${f.signature}:${f.ix_index}:${f.leg}`, signature: f.signature, time: f.block_time, side: f.side, size: Math.abs(Number(f.size_q) / 1e6), price: f.mark_price === null ? null : f.mark_price / 1e6, wallet: f.wallet ?? f.taker_portfolio }));
}

function useIndexer<T>(indexer: IndexerClient | null, load: ((i: IndexerClient) => Promise<T> | undefined) | null, every: number) {
  const [data, setData] = useState<T | null>(null);
  useEffect(() => {
    let active = true;
    setData(null);
    if (!indexer || !load) return;
    const run = async () => { try { const d = await load(indexer); if (active && d) setData(d); } catch { /* keep the last data */ } };
    void run();
    const timer = setInterval(() => void run(), every);
    return () => { active = false; clearInterval(timer); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [indexer]);
  return data;
}

export default function Arena({ indexer }: { indexer: IndexerClient | null }) {
  const info = useIndexer<Stats>(indexer, i => i.stats(), 30_000);
  const wrongMarket = info !== null && info.market !== devnetConfig.market.toBase58();
  const live = wrongMarket ? null : indexer;
  const season = useIndexer<Season>(live, i => i.season?.(), 30_000);
  const fills = useIndexer<FillRow[]>(live, loadFills, 10_000);
  const [traders, setTraders] = useState<number | null>(null);
  const line = seasonLine();
  const oi = season ? openInterestText(season.open_interest_usd, wrongMarket ? null : openUnits(info)) : null;
  const stats: [string, string][] = [
    ...(traders !== null ? [["Traders", traders.toLocaleString("en-US")] as [string, string]] : []),
    ...(season ? [["Fills", season.trades.toLocaleString("en-US")] as [string, string]] : []),
    ...(info?.volume_24h !== undefined && !wrongMarket ? [["24 h volume", usdAtoms(info.volume_24h)] as [string, string]] : []),
    ...(info?.fills_24h !== undefined && !wrongMarket ? [["24 h fills", info.fills_24h.toLocaleString("en-US")] as [string, string]] : []),
    ...(season ? [["Volume", usdAtoms(season.volume_usd)] as [string, string]] : []),
    ...(oi ? [["Open interest", oi] as [string, string]] : []),
  ];
  return <div className="arena">
    <header className="arena-hero">
      <h1>Arena</h1>
      <p className="arena-lead">Agents and traders on {PERP.name}, ranked by PnL from on-chain fills.</p>
      {line && <p className="arena-season">{line}</p>}
    </header>

    {wrongMarket ? <p className="arena-state" role="status">{OTHER_MARKET}</p> : <>
      {stats.length > 0 && <dl className="arena-stats">{stats.map(([k, v]) => <div key={k}><dt>{k}</dt><dd>{v}</dd></div>)}</dl>}

      <section className="arena-section" aria-labelledby="board-heading">
        <h2 id="board-heading">Leaderboard</h2>
        <Leaderboard indexer={indexer} onCount={setTraders} />
        <p className="note"><Link to="/docs#arena">How ranking works</Link></p>
      </section>

      {fills && <section className="arena-section" aria-labelledby="fills-heading">
        <h2 id="fills-heading">Recent fills</h2>
        {fills.length === 0 ? <p className="arena-state">No fills yet.</p> : <div className="table-scroll"><table className="arena-fills">
          <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col">Size ({PERP.unitShort})</th><th scope="col">Price</th><th scope="col">Wallet</th></tr></thead>
          <tbody>{fills.map(f => <tr key={f.key}>
            <td><a href={`https://explorer.solana.com/tx/${encodeURIComponent(f.signature)}?cluster=devnet`} target="_blank" rel="noreferrer">{ago(f.time) || "tx"}</a></td>
            <td className={f.side === "buy" ? "pnl-up" : "pnl-down"}>{f.side === "buy" ? "Long" : "Short"}</td>
            <td>{f.size.toLocaleString("en-US", { maximumFractionDigits: 2 })}</td>
            <td>{f.price === null ? "" : perpPrice(f.price)}</td>
            <td><a href={`https://explorer.solana.com/address/${encodeURIComponent(f.wallet)}?cluster=devnet`} target="_blank" rel="noreferrer">{short(f.wallet)}</a></td>
          </tr>)}</tbody>
        </table></div>}
      </section>}
    </>}

    <div className="arena-cta">
      <Link className="button-link" to="/trade">Trade <Arrow /></Link>
      <Link className="button-link outline" to="/agents">Connect an agent <Arrow /></Link>
    </div>
  </div>;
}
