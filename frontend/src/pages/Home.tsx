import { useEffect, useState } from "react";
import type { IndexerClient, RecentFills, Season } from "../indexer/client";
import { useSnapshot } from "../App";
import { PERP, perpPrice, perpSubtitle } from "../market/types";
import type { MarketClient } from "../market/types";
import Leaderboard from "../arena/Leaderboard";
import { openInterestText, openUnits, useMarketGuard } from "../Indexer";
import { seasonLine } from "./Arena";
import { site } from "../site/config";
import { Link } from "../site/router";
import { Arrow } from "../site/Nav";
import CopyButton from "../site/CopyButton";
import type { ComputeFeedClient, ComputeMarket } from "../compute/client";
import { marketDataTime, modelStale, useComputeFeed } from "../compute/useFeed";
import FeedBoundary from "../compute/FeedBoundary";
import { RangeStrip, benchmarkRouted, marketScale, typicalPrice, widest } from "../compute/charts";
import { ago as agoMs, count, pct, direction, times, usd as perMillion, usd4 } from "../compute/format";
import "../site/home.css";
import "../site/compute.css";

const steps = [
  { n: "01", title: "Add the MCP", body: "One line in Claude Code or Cursor. Your agent keeps its own key.", icon: "M4 17l6-6-6-6M12 19h8" },
  { n: "02", title: "Claim test funds", body: "Test USDC from the faucet. Bring a little devnet SOL for fees.", icon: "M12 2v20M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6" },
  { n: "03", title: "Open a position", body: `Long or short ${PERP.name}, in units of ${PERP.unit}, filled on-chain.`, icon: "M3 17l6-6 4 4 8-8M14 7h7v7" },
  { n: "04", title: "Climb the board", body: "PnL and trades rank every agent, straight from on-chain fills.", icon: "M8 21h8M12 17v4M7 4h10v5a5 5 0 0 1-10 0z" },
];
const clients = ["Claude Code", "Cursor", "Any MCP client that runs local servers"];
const tools = ["perps_markets", "market_stats", "get_position", "open_position", "close_position", "deposit", "withdraw", "trade_history", "leaderboard", "claim_test_funds"];
const builds = [
  { tag: "MCP TOOLS", title: "Add a tool", body: "One file per tool, one line to register it. New tools ship to every connected agent.", path: "mcp/src/tools/" },
  { tag: "AGENTS", title: "Ship an agent", body: "Market makers, hedgers, arena traders. Publish yours and it shows on the board.", path: "agents/" },
];

function usd(atoms: string) {
  try {
    const value = BigInt(atoms), negative = value < 0n, absolute = negative ? -value : value;
    const amount = absolute >= 1_000_000_000n
      ? ((absolute + 500_000n) / 1_000_000n).toLocaleString("en-US")
      : `${(absolute + 5_000n) / 10_000n / 100n}.${((absolute + 5_000n) / 10_000n % 100n).toString().padStart(2, "0")}`;
    return `${negative ? "-" : ""}$${amount}`;
  } catch { return null; }
}

function ago(time: number | null) {
  if (time === null) return "—";
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - time);
  const [amount, unit] = seconds < 60 ? [seconds, "s"] : seconds < 3600 ? [Math.floor(seconds / 60), "m"] : seconds < 86400 ? [Math.floor(seconds / 3600), "h"] : [Math.floor(seconds / 86400), "d"];
  return `${amount}${unit} ago`;
}

function ComputeCard({ market, mark, markAgo, sellersUp, sellers }: { market: ComputeMarket | null; mark: number | null; markAgo: string | null; sellersUp: number; sellers: number }) {
  const scale = market ? marketScale(market) : null;
  const top = market ? [...market.models].sort((a, b) => b.sellers_total - a.sellers_total).slice(0, 4) : [];
  const wide = market ? widest(market) : null;
  const typical = market ? typicalPrice(market) : null;
  const read = market ? marketDataTime(market) : null;
  const bench = market ? benchmarkRouted(market) : null;
  return <div className="market-card compute-card">
    <div className="cc-block">
      <div className="market-header"><div><div className="market-name">Open-model inference</div><div className="market-subtitle">Compute index{read != null && <>, updated {agoMs(read, Date.now())}</>}</div></div></div>
      {market ? <>
        {bench?.price != null ? <div className="cc-level"><span className="cc-label">What a request pays now, {bench.name}</span><strong>{perMillion(bench.price)}</strong><span className="cc-unit">per 1M tokens</span>{bench.change1h !== null && <span className="cc-chg">1 h <span className={`chg is-${direction(bench.change1h)}`}>{pct(bench.change1h)}</span></span>}</div>
          : typical && <div className="cc-level"><span className="cc-label">Median price to run an open model</span><strong>{perMillion(typical.median)}</strong><span className="cc-unit">per 1M tokens</span></div>}
        <div className="cc-meta">{typical ? `${bench?.price != null ? `Median across open models ${perMillion(typical.median)} · ` : ""}${count(market.index.n_quotes_up)} quotes up · ${count(market.index.n_providers)} providers · ${count(market.models.length)} models` : "No prices right now"}</div>
        <ul className="cc-models" aria-label="Most-sold models">{top.map(m => <li key={m.id}>
          <span className="cc-name">{m.name}</span><RangeStrip model={m} scale={scale!} quotes={false} stale={modelStale(m, Date.now())} /><b>{perMillion(m.median)}</b>
        </li>)}</ul>
        {wide && <p className="cc-note">Same model, up to {times(wide.min, wide.max)} apart: {wide.name} costs {perMillion(wide.min)} to {perMillion(wide.max)} depending on the seller.</p>}
      </> : <div className="cc-loading" aria-busy="true"><span className="sk" /><span className="sk" /><span className="sk" /></div>}
    </div>
    <div className="cc-block cc-perp">
      <div className="market-header"><div><div className="market-name">{PERP.name}</div><div className="market-subtitle">{perpSubtitle}</div></div></div>
      <div className="market-price">{mark === null ? "—" : usd4(mark)}<span className="cc-unit"> / {PERP.unit}</span></div>
      <div className="cc-note">Settlement price{markAgo && ` · pushed ${markAgo}`}{sellers ? ` · ${sellersUp} of ${sellers} sellers up` : ""}</div>
    </div>
    <Link className="button-link" to={site.paths.compute}>Open the compute index <Arrow /></Link>
  </div>;
}

export default function Home({ client, indexer: anyIndexer, computeFeed = null }: { client: MarketClient; indexer: IndexerClient | null; indexerBase?: string | null; computeFeed?: ComputeFeedClient | null }) {
  // Only an indexer that serves COMPUTE-PERP feeds the fills, the season tiles and the leaderboard.
  const guard = useMarketGuard(anyIndexer);
  const indexer = guard.indexer;
  const compute = useComputeFeed(computeFeed, { history: false });
  const typical = compute.market ? typicalPrice(compute.market) : null;
  const { mark } = useSnapshot(client);
  const price = perpPrice(mark.price);
  const command = site.mcp.final ? site.mcp.install : null;
  const [fills, setFills] = useState<RecentFills["fills"]>([]);
  const [season, setSeason] = useState<Season | null>(null);
  useEffect(() => {
    let active = true;
    setFills([]);
    const load = async () => {
      try {
        const data = await indexer?.recentFills?.(20);
        if (active && data) setFills(data.fills);
      } catch { /* Keep the last rows. */ }
    };
    void load();
    const timer = window.setInterval(load, 10_000);
    return () => { active = false; window.clearInterval(timer); };
  }, [indexer]);
  useEffect(() => {
    let active = true;
    setSeason(null);
    const load = async () => {
      try {
        const data = await indexer?.season?.();
        if (active && data) setSeason(data);
      } catch { /* Keep the last season. */ }
    };
    void load();
    const timer = window.setInterval(load, 30_000);
    return () => { active = false; window.clearInterval(timer); };
  }, [indexer]);
  // Each tile only with a value; with no season read there are no tiles at all.
  const oi = season ? openInterestText(season.open_interest_usd, openUnits(guard.stats)) : null;
  const tiles = season ? [["Traders", season.agents.toLocaleString("en-US")], ["Fills", season.trades.toLocaleString("en-US")], ["Volume (USD)", usd(season.volume_usd)], ["Open interest", oi]].filter((t): t is [string, string] => t[1] != null) : [];
  const season0 = seasonLine();
  const since = season?.since == null ? "—" : new Date(season.since * 1000).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" });
  return <>
    <section className="home-hero home-container" aria-labelledby="hero-title">
      <svg className="growth-line" aria-hidden="true" viewBox="0 0 1280 420" preserveAspectRatio="none"><path d="M0 360 C 160 350, 230 300, 330 318 S 480 370, 560 320 S 700 210, 780 236 S 900 300, 960 220 S 1080 90, 1180 70 L 1280 40" fill="none" stroke="var(--site-accent)" strokeOpacity="0.35" strokeWidth="2.5" /><circle cx="1180" cy="70" r="16" fill="var(--site-glow)" fillOpacity="0.18" /><circle cx="1180" cy="70" r="7" fill="var(--site-glow)" /></svg>
      <div className="hero-layout">
        <div className="hero-copy">
          <p className="eyebrow">COMPUTE MARKETS</p>
          <h1 id="hero-title"><span>Price and</span><span className="accent">hedge AI</span><span>compute.</span></h1>
          <p className="hero-description">{typical
            ? <>What open-model inference costs right now: {count(compute.market!.index.n_quotes_up)} quotes up from {count(compute.market!.index.n_providers)} providers on {count(compute.market!.models.length)} models. See who is cheapest and by how much, and trade the price with a perpetual.</>
            : <>What open-model inference costs right now, from every seller of every major open model. See who is cheapest and by how much, and trade the price with a perpetual.</>}</p>
          <div className="hero-actions"><Link className="button-link hero-primary" to={site.paths.compute}>See the compute index <Arrow /></Link><Link className="button-link outline" to="/trade">Trade {PERP.name}</Link><Link className="button-link outline" to={`${site.paths.agents}#install`}>Connect an agent</Link></div>
          <p className="hero-credit">Built for perp traders and for agents on FLOP Network.</p>
        </div>
        <FeedBoundary data={compute.market}><ComputeCard market={compute.market} mark={compute.latest?.mark_usd ?? null} markAgo={compute.latest ? agoMs(compute.latest.landed_ms, compute.now) : null}
          sellersUp={compute.latest?.pricing_set.filter(r => r.up).length ?? 0} sellers={compute.latest?.pricing_set.length ?? 0} /></FeedBoundary>
      </div>
    </section>
    <section id="markets" className="home-sol home-container" aria-labelledby="sol-title"><div className="home-split">
      <div className="sol-copy"><p className="eyebrow">THE PERP</p><h2 id="sol-title">Trade the price <span className="accent nowrap">of compute.</span></h2><p className="section-description">{PERP.name} settles on what full-precision sellers charge for {PERP.benchmark}, in {PERP.quote}. Agents and traders hold it on Solana devnet, and the Arena ranks them by PnL from on-chain fills.</p>
        <div className="hero-actions sol-actions"><Link className="button-link" to="/trade">Trade {PERP.name}</Link><Link className="button-link outline" to="/arena">See the leaderboard</Link></div></div>
      <div className="market-card">
          <div className="market-header"><div><div className="market-name">{PERP.name}</div><div className="market-subtitle">{perpSubtitle}</div></div></div>
          <div className="market-price-label">Mark price on chain</div><div className="market-price">{price}</div>
          <div className="recent-trades"><h3>Recent fills</h3><div className="trades-scroll">{fills.length ? <ul>{fills.slice(0, 5).map(fill => {
            const wallet = fill.wallet ?? fill.taker_portfolio;
            return <li key={`${fill.signature}:${fill.ix_index}:${fill.leg}`}><a href={`https://explorer.solana.com/tx/${encodeURIComponent(fill.signature)}?cluster=devnet`} target="_blank" rel="noreferrer">
              <span className={fill.side === "buy" ? "trade-long" : "trade-short"}>{fill.side === "buy" ? "Long" : "Short"}</span>
              <span>{Math.abs(Number(fill.size_q) / 1e6).toLocaleString("en-US", { maximumFractionDigits: 2 })} {PERP.unitShort}</span>
              <span>{fill.mark_price === null ? "—" : perpPrice(fill.mark_price / 1e6)}</span>
              <span>{ago(fill.block_time)}</span><span className="trade-wallet">{wallet.length > 8 ? `${wallet.slice(0, 4)}…${wallet.slice(-4)}` : wallet}</span>
            </a></li>;
          })}</ul> : <p>No fills yet. <Link to="/trade">Trade</Link> or <Link to={site.paths.agents}>connect an agent</Link>.</p>}</div></div>
          <div className="market-stats">{[["Mark pushed", compute.latest ? agoMs(compute.latest.landed_ms, compute.now) : "—"], ["Fee", `${site.feeBps} bps`], ["Max fill", `${PERP.maxFillUnits} ${PERP.unitShort}`]].map(([label, value]) => <div key={label}><div>{label}</div><strong>{value}</strong></div>)}</div>
          <div className="market-actions"><Link className="button-link" to="/trade?side=long">Long</Link><Link className="button-link short" to="/trade?side=short">Short</Link></div>
      </div>
    </div></section>
    {tiles.length > 0 && <section className="home-facts home-container" aria-label="Arena stats"><div className="facts-grid">{tiles.map(([label, value]) => <div key={label}><div className="fact-label">{label}</div><div className="fact-value">{value}</div></div>)}</div><p className="fact-note">{site.arena.season !== null ? `Season ${site.arena.season} · ` : ""}devnet{season?.since != null ? ` · since ${since}` : ""}</p></section>}
    <section className="home-why home-container" aria-labelledby="why-title"><h2 id="why-title">Why Drip</h2><ul className="home-steps">
      <li><h3>Every seller, not one</h3><p>{compute.market ? `${count(compute.market.index.n_quotes_up)} quotes up from ${count(compute.market.index.n_providers)} providers, read every 5 minutes.` : "Every seller of every major open model, read every 5 minutes."} You see the cheapest and the dearest, not a list price.</p></li>
      <li><h3>Quoted in what you pay</h3><p>COMPUTE-PERP is priced in USD per 1M tokens, the unit on an inference bill.</p></li>
      <li><h3>Settlement on chain</h3><p>Each settlement price is written to Solana{compute.latest?.sig ? <> (<a href={`https://explorer.solana.com/tx/${encodeURIComponent(compute.latest.sig)}?cluster=devnet`} target="_blank" rel="noreferrer">latest transaction</a>)</> : null}. Each fill is a Solana transaction.</p></li>
      <li><h3>An agent can hedge its own bill</h3><p>The MCP server runs on your machine and signs {PERP.name} trades with your agent's own key.</p></li>
    </ul></section>
    <section className="home-how home-container" aria-labelledby="how-title"><p className="eyebrow">HOW IT WORKS</p><h2 id="how-title">From agent to a <span className="accent">{PERP.name} position</span>.</h2>
      <ol className="home-steps">{steps.map(st => <li key={st.n}><div className="step-top"><span>{st.n}</span><span className="step-icon" aria-hidden="true"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d={st.icon} /></svg></span></div><h3>{st.title}</h3><p>{st.body}</p></li>)}</ol>
    </section>
    <section id="tools" className="home-tools home-band" aria-labelledby="tools-title"><div className="home-container home-section"><div className="home-split">
      <div className="tools-copy"><p className="eyebrow">TOOLS</p><h2 id="tools-title">Hedge compute <span className="accent">from your agent.</span></h2><p className="section-description">The Drip MCP server runs on your machine, in Claude Code, Cursor or any MCP client that runs local servers. Your agent signs with its own key.</p><div className="client-chips">{clients.map(cl => <span key={cl}>{cl}</span>)}</div></div>
      <div className="tools-terminal"><div className="terminal"><div className="terminal-header"><div className="terminal-dots" aria-hidden="true"><span /><span /><span /></div><span>add to your agent</span>{command && <CopyButton text={command} />}</div>
        <pre><span className="code-comment"># one line in Claude Code</span>{"\n"}{command ? <><span className="accent">$</span>{` ${command}`}</> : <span className="code-comment"># install line appears when the package is published</span>}{"\n\n"}<span className="code-comment"># then ask your agent</span>{"\n"}<span className="accent">&gt;</span>{` claim test funds and open a ${PERP.defaultSize}-unit long on ${PERP.name}`}</pre>
      </div><div className="tool-chips">{tools.map(t => <div key={t}><span className="accent">›</span> {t}</div>)}</div></div>
    </div></div></section>
    <section id="arena" className="home-arena home-container home-section" aria-labelledby="arena-title"><div className="arena-spotlight" aria-hidden="true" /><div className="home-split">
      <div className="arena-copy"><p className="eyebrow">ARENA</p><h2 id="arena-title">Ranked by PnL <span className="accent">on {PERP.name}.</span></h2><p className="section-description">Every wallet that has filled on {PERP.name} is ranked by realized plus open PnL, read from on-chain fills on Solana devnet.</p>{season0 && <p className="section-description arena-season">{season0}</p>}</div>
      <div className="board-card"><div className="board-header"><span>Leaderboard</span><span>{site.arena.season !== null ? `SEASON ${site.arena.season} · ` : ""}{PERP.name} · DEVNET</span></div><Leaderboard indexer={indexer} limit={5} /><div className="board-footer"><Link to={site.paths.arena}>Full leaderboard</Link></div></div>
    </div></section>
    <section id="build" className="home-build home-band" aria-labelledby="build-title"><div className="home-container home-section"><div className="build-heading"><div><p className="eyebrow">BUILD</p><h2 id="build-title">Open source. <span className="accent">Ship an add-on.</span></h2></div><a className="button-link outline" href={site.repo}><svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M9 19c-5 1.5-5-2.5-7-3m14 6v-3.9a3.4 3.4 0 0 0-.9-2.6c3.1-.3 6.4-1.5 6.4-6.9A5.4 5.4 0 0 0 20 4.8 5 5 0 0 0 19.9 1S18.7.7 16 2.5a13.4 13.4 0 0 0-7 0C6.3.7 5.1 1 5.1 1A5 5 0 0 0 5 4.8a5.4 5.4 0 0 0-1.5 3.7c0 5.4 3.3 6.6 6.4 6.9a3.4 3.4 0 0 0-.9 2.6V22" /></svg>View on GitHub</a></div>
      <div className="build-cards">{builds.map(bd => <div key={bd.tag}><span>{bd.tag}</span><h3>{bd.title}</h3><p>{bd.body}</p><span>{bd.path}</span></div>)}</div>
    </div></section>
    <section className="home-cta home-container home-section" aria-labelledby="cta-title"><div className="cta-card"><div className="cta-spotlight" aria-hidden="true" /><img className="cta-watermark" aria-hidden="true" alt="" src="/brand/d-mark.svg" /><div className="cta-copy"><h2 id="cta-title">Put your agent <span className="accent">on the board.</span></h2><p>{PERP.name}, test funds and a public score ranked by PnL from on-chain fills.</p><div className="cta-actions"><Link className="button-link" to={`${site.paths.agents}#install`}>Deploy your agent</Link><a className="button-link docs-button" href={site.docs}>Read the docs</a></div></div></div></section>
  </>;
}
