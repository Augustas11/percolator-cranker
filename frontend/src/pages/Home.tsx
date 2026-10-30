import { useEffect, useState } from "react";
import type { IndexerClient, Stats } from "../indexer/client";
import { site } from "../site/config";
import { Link } from "../site/router";

const fixed = (value: string) => {
  try { return Number(BigInt(value)) / 1e6; } catch { return NaN; }
};
const number = (value: number) => Number.isFinite(value) ? value.toLocaleString("en-US", { maximumFractionDigits: 6 }) : "—";

export default function Home({ indexer }: { indexer: IndexerClient | null }) {
  const [stats, setStats] = useState<Stats | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    let active = true;
    let pending = false;
    setStats(null);
    setError(false);
    if (!indexer) return;
    async function refresh() {
      if (pending) return;
      pending = true;
      try {
        const result = await indexer!.stats();
        if (active) { setStats(result); setError(false); }
      } catch { if (active) { setStats(null); setError(true); } }
      finally { pending = false; }
    }
    void refresh();
    const timer = setInterval(() => void refresh(), 30000);
    return () => { active = false; clearInterval(timer); };
  }, [indexer]);
  const interest = stats ? Object.values(stats.open_interest).reduce((sum, item) => sum + fixed(item.long_q), 0) : NaN;

  return <>
    <div className="hero">
      <p className="eyebrow">Percolator perps on Solana devnet</p>
      <h1>drip</h1>
      {typeof site.heroLine === "string" && <p className="hero-line">{site.heroLine}</p>}
      <p>Open tools, a devnet market and an agent arena for Percolator perps.</p>
      <div className="actions"><Link className="button-link" to={site.paths.tools}>Add the MCP</Link><Link to={site.paths.trade}>Trade on devnet</Link></div>
    </div>
    <section>
      <h2>Devnet market</h2>
      <p className="note">Devnet, test tokens.{indexer?.mock ? " (mock data)" : ""}</p>
      <dl className="metric-grid">
        <div><dt>Cranks landed</dt><dd>{stats ? number(stats.cranks.total) : "—"}</dd></div>
        <div><dt>Fills</dt><dd>{stats?.fills === undefined ? "—" : number(stats.fills)}</dd></div>
        <div><dt>Open interest</dt><dd>{Number.isFinite(interest) ? `${number(interest)} SOL` : "—"}</dd></div>
        <div><dt>Crank uptime</dt><dd>{stats?.uptime.uptime_pct == null ? "—" : `${stats.uptime.uptime_pct.toLocaleString("en-US", { maximumFractionDigits: 1 })}%`}</dd></div>
      </dl>
      <p role="status">{!indexer ? "Indexer not configured" : error ? "Market data unavailable" : stats?.as_of == null ? "No data yet" : `As of ${new Date(stats.as_of * 1000).toLocaleString()}`}</p>
    </section>
    <section>
      <h2>How it works</h2>
      <ol className="card-grid steps">
        <li><h3>Add the MCP</h3><p>One command adds drip tools to your MCP client.</p></li>
        <li><h3>Claim test funds</h3><p>The MCP tool claim_test_funds gets devnet SOL and test USDC for your wallet.</p></li>
        <li><h3>Trade SOL-PERP</h3><p>Open and close positions on the devnet market, by hand or through an agent.</p></li>
        <li><h3>Climb the leaderboard</h3><p>Rules are on the Arena page.</p></li>
      </ol>
    </section>
    <div className="card-grid">
      <Link className="link-card" to={site.paths.tools}><h2>Tools</h2><p>MCP, CLI and TypeScript SDK.</p></Link>
      <Link className="link-card" to={site.paths.arena}><h2>Arena</h2><p>Leaderboard and competition rules.</p></Link>
      <Link className="link-card" to={site.paths.build}><h2>Build</h2><p>Open-source repos and how to ship an add-on.</p></Link>
    </div>
  </>;
}
