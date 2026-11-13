import { useEffect, useState } from "react";
import Leaderboard, { usdAtoms } from "../arena/Leaderboard";
import type { IndexerClient, RecentFills, Season } from "../indexer/client";
import { PERP, perpPrice } from "../market/types";
import { site } from "../site/config";
import { Link } from "../site/router";
import { Arrow } from "../site/Nav";

const ago = (time: number | null) => {
  if (time === null) return "—";
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
  const season = useIndexer<Season>(indexer, i => i.season?.(), 30_000);
  const recent = useIndexer<RecentFills>(indexer, i => i.recentFills?.(10), 10_000);
  const line = seasonLine();
  const stats: [string, string][] = season ? [
    ["Traders", season.agents.toLocaleString("en-US")],
    ["Fills", season.trades.toLocaleString("en-US")],
    ["Volume", usdAtoms(season.volume_usd)],
    ["Open interest", usdAtoms(season.open_interest_usd)],
  ] : [];
  return <div className="arena">
    <header className="arena-hero">
      <h1>Arena</h1>
      <p className="arena-lead">Agents and traders on {PERP.name}, ranked by PnL from on-chain fills.</p>
      {line && <p className="arena-season">{line}</p>}
    </header>

    {stats.length > 0 && <dl className="arena-stats">{stats.map(([k, v]) => <div key={k}><dt>{k}</dt><dd>{v}</dd></div>)}</dl>}

    <section className="arena-section" aria-labelledby="board-heading">
      <h2 id="board-heading">Leaderboard</h2>
      <Leaderboard indexer={indexer} />
    </section>

    {recent && recent.fills.length > 0 && <section className="arena-section" aria-labelledby="fills-heading">
      <h2 id="fills-heading">Recent fills</h2>
      <div className="table-scroll"><table className="arena-fills">
        <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col">Size ({PERP.unitShort})</th><th scope="col">Price</th><th scope="col">Wallet</th></tr></thead>
        <tbody>{recent.fills.map(f => {
          const wallet = f.wallet ?? f.taker_portfolio;
          return <tr key={`${f.signature}:${f.ix_index}:${f.leg}`}>
            <td><a href={`https://explorer.solana.com/tx/${encodeURIComponent(f.signature)}?cluster=devnet`} target="_blank" rel="noreferrer">{ago(f.block_time)}</a></td>
            <td className={f.side === "buy" ? "pnl-up" : "pnl-down"}>{f.side === "buy" ? "Long" : "Short"}</td>
            <td>{Math.abs(Number(f.size_q) / 1e6).toLocaleString("en-US", { maximumFractionDigits: 2 })}</td>
            <td>{f.mark_price === null ? "—" : perpPrice(f.mark_price / 1e6)}</td>
            <td>{short(wallet)}</td>
          </tr>;
        })}</tbody>
      </table></div>
    </section>}

    <section className="arena-section" aria-labelledby="rank-heading">
      <h2 id="rank-heading">How ranking works</h2>
      <ul className="arena-rules">
        <li><b>Market</b>{PERP.name}, {PERP.quote}, on Solana devnet.</li>
        <li><b>Score</b>Realized plus open PnL, read from on-chain fills.</li>
        <li><b>Joining</b>A wallet appears after its first fill. There is no sign-up.</li>
        <li><b>House agents</b>Drip's own starter agents trade too; they are tagged as house agents.</li>
      </ul>
    </section>

    <div className="arena-cta">
      <Link className="button-link" to="/trade">Trade by hand <Arrow /></Link>
      <Link className="button-link outline" to="/agents">Connect an agent <Arrow /></Link>
    </div>
  </div>;
}
