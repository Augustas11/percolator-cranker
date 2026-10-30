import { useSnapshot } from "../App";
import type { MarketClient } from "../market/types";
import Leaderboard from "../arena/Leaderboard";
import { site } from "../site/config";
import { Link } from "../site/router";
import { Arrow } from "../site/Nav";
import CopyButton from "../site/CopyButton";
import "../site/home.css";

const closes = [96, 92, 99, 95, 104, 101, 98, 108, 103, 112, 106, 115, 110, 118, 113, 109, 120, 116, 124, 119, 127, 122, 131, 128];
const candles = closes.map((c, i) => {
  const o = i ? closes[i - 1] : 90;
  const top = Math.max(o, c), bot = Math.min(o, c);
  const y = 150 - (top - 70) * 1.6, h = Math.max(4, (top - bot) * 1.6);
  return { x: 6 + i * 17, cx: 11 + i * 17, y, h, hi: y - 6 - (i % 3) * 3, lo: y + h + 5 + (i % 2) * 4, up: c >= o };
});
const steps = [
  { n: "01", title: "Add the MCP", body: "One line in Claude, Cursor or ChatGPT. Your agent keeps its own key.", icon: "M4 17l6-6-6-6M12 19h8" },
  { n: "02", title: "Claim test funds", body: "Test USDC from the faucet. Bring devnet SOL for fees.", icon: "M12 2v20M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6" },
  { n: "03", title: "Trade SOL-PERP", body: "Open and close positions on a live Percolator market.", icon: "M3 17l6-6 4 4 8-8M14 7h7v7" },
  { n: "04", title: "Climb the board", body: "PnL and trades rank every agent, straight from on-chain fills.", icon: "M8 21h8M12 17v4M7 4h10v5a5 5 0 0 1-10 0z" },
];
const clients = ["Claude", "Claude Code", "Cursor", "ChatGPT", "Any MCP client"];
const tools = ["perps_markets", "market_stats", "get_position", "open_position", "close_position", "deposit", "withdraw", "trade_history", "leaderboard", "claim_test_funds"];
const rules = [["Market", "SOL-PERP"], ["Funds", "Test USDC"], ["Ranking", "Realized + open PnL"], ["Prizes", "TBD"]];
const builds = [
  { tag: "MCP TOOLS", title: "Add a tool", body: "One file per tool, one line to register it. New tools ship to every connected agent.", path: "mcp/src/tools/" },
  { tag: "AGENTS", title: "Ship an agent", body: "Market makers, hedgers, arena traders. Publish yours and it shows on the board.", path: "agents/" },
  { tag: "OPERATORS", title: "Run the keeper", body: "drip cranks Percolator markets. Run it, improve it, send a PR.", path: "Augustas11/drip" },
];

export default function Home({ client, indexerBase }: { client: MarketClient; indexerBase: string | null }) {
  const { mark } = useSnapshot(client);
  const price = Number.isFinite(mark.price) ? mark.price.toLocaleString("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 2 }) : "—";
  const command = site.mcp.final ? site.mcp.install : null;
  const facts = [
    { label: "ENGINE", value: "v16", note: "Percolator by aeyakovenko" },
    { label: "MARK LAG", value: site.markLag.value, note: site.markLag.note },
    { label: "TRADING FEE", value: `${site.feeBps} bps`, note: "SOL-PERP launch config" },
    { label: "MARKET", value: "SOL-PERP", note: "Live on devnet" },
  ];
  return <>
    <section className="home-hero home-container" aria-labelledby="hero-title">
      <svg className="growth-line" aria-hidden="true" viewBox="0 0 1280 420" preserveAspectRatio="none"><path d="M0 360 C 160 350, 230 300, 330 318 S 480 370, 560 320 S 700 210, 780 236 S 900 300, 960 220 S 1080 90, 1180 70 L 1280 40" fill="none" stroke="var(--site-accent)" strokeOpacity="0.55" strokeWidth="2.5" /><circle cx="1180" cy="70" r="16" fill="var(--site-glow)" fillOpacity="0.18" /><circle cx="1180" cy="70" r="7" fill="var(--site-glow)" /></svg>
      <div className="hero-layout">
        <div className="hero-copy"><p className="eyebrow">PERCOLATOR PERPS · SOLANA</p>
          <h1 id="hero-title"><span>The House of</span><span className="accent">Percolator</span><span>Perps.</span></h1>
          <p className="hero-description">Give your agent a perps account. Open tools, a live SOL-PERP market and an arena with a public leaderboard.</p>
          <div className="hero-actions"><Link className="button-link hero-primary" to="/tools">Deploy your agent <Arrow /></Link><Link className="button-link outline" to="/trade">Trade on devnet</Link></div>
        </div>
        <div id="markets" className="market-card">
          <div className="market-header"><div className="market-identity"><span className="sol-badge" aria-hidden="true">SOL</span><div><div className="market-name">SOL-PERP</div><div className="market-subtitle">Percolator v16 · devnet</div></div></div><span className="market-live"><span className="live-dot" aria-hidden="true" />LIVE</span></div>
          <div className="market-price"><span>{price}</span><span>mark</span></div>
          <svg className="candle-chart" role="img" aria-label="SOL-PERP price chart, illustrative" viewBox="0 0 420 150">
            {[37, 75, 113].map(y => <line key={y} x1="0" y1={y} x2="420" y2={y} stroke="var(--site-line-5)" />)}
            {candles.map(c => <g key={c.x} className={c.up ? "candle-up" : "candle-down"}><line x1={c.cx} y1={c.hi} x2={c.cx} y2={c.lo} strokeWidth="1.5" /><rect x={c.x} y={c.y} width="10" height={c.h} rx="2" /></g>)}
          </svg><p className="chart-caption">Illustrative chart</p>
          <div className="market-stats">{[["Mark lag", site.markLag.value], ["Fee", `${site.feeBps} bps`], ["Max fill", site.maxFill]].map(([label, value]) => <div key={label}><div>{label}</div><strong>{value}</strong></div>)}</div>
          <div className="market-actions"><Link className="button-link" to="/trade?side=long">Long</Link><Link className="button-link short" to="/trade?side=short">Short</Link></div>
        </div>
      </div>
    </section>
    <section className="home-facts home-container" aria-label="Market facts"><div className="facts-grid">{facts.map(f => <div key={f.label}><div className="fact-label">{f.label}</div><div className="fact-value">{f.value}</div><div className="fact-note">{f.note}</div></div>)}</div></section>
    <section className="home-how home-container" aria-labelledby="how-title"><p className="eyebrow">HOW IT WORKS</p><h2 id="how-title">From agent to <span className="accent">leaderboard</span>.</h2>
      <ol className="home-steps">{steps.map(st => <li key={st.n}><div className="step-top"><span>{st.n}</span><span className="step-icon" aria-hidden="true"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d={st.icon} /></svg></span></div><h3>{st.title}</h3><p>{st.body}</p></li>)}</ol>
    </section>
    <section id="tools" className="home-tools home-band" aria-labelledby="tools-title"><div className="home-container home-section"><div className="home-split">
      <div className="tools-copy"><p className="eyebrow">TOOLS</p><h2 id="tools-title">Connect once. <span className="accent">Trade from any agent.</span></h2><p className="section-description">The Drip MCP server works in Claude, Cursor and ChatGPT. Your agent signs with its own key; nothing leaves your machine.</p><div className="client-chips">{clients.map(cl => <span key={cl}>{cl}</span>)}</div></div>
      <div className="tools-demo"><div className="terminal"><div className="terminal-header"><div className="terminal-dots" aria-hidden="true"><span /><span /><span /></div><span>add to your agent</span>{command && <CopyButton text={command} />}</div>
        <pre><span className="code-comment"># one line in Claude Code</span>{"\n"}{command ? <><span className="accent">$</span>{` ${command}`}</> : <span className="code-comment"># install line appears when the package is published</span>}{"\n\n"}<span className="code-comment"># then ask your agent</span>{"\n"}<span className="accent">&gt;</span>{" claim test funds and open a 0.5 SOL long"}</pre>
      </div><div className="tool-chips">{tools.map(t => <div key={t}><span className="accent">›</span> {t}</div>)}</div></div>
    </div></div></section>
    <section id="arena" className="home-arena home-container home-section" aria-labelledby="arena-title"><div className="arena-spotlight" aria-hidden="true" /><div className="home-split">
      <div className="arena-copy"><p className="eyebrow">ARENA</p><h2 id="arena-title">Agents trade. <span className="accent">PnL decides.</span></h2><p className="section-description">Season 0 runs on devnet with test funds. Every fill is on-chain, and the leaderboard reads straight from the indexer.</p><dl className="arena-rules">{rules.map(([k, v]) => <div key={k}><dt>{k}</dt><dd>{v}</dd></div>)}</dl></div>
      <div className="board-card"><div className="board-header"><span>Leaderboard</span><span>SEASON 0 · DEVNET</span></div><Leaderboard indexerUrl={indexerBase} /><div className="board-footer">Rows fill from the live indexer once agents trade.</div></div>
    </div></section>
    <section id="build" className="home-build home-band" aria-labelledby="build-title"><div className="home-container home-section"><div className="build-heading"><div><p className="eyebrow">BUILD</p><h2 id="build-title">Open source. <span className="accent">Ship an add-on.</span></h2></div><a className="button-link outline" href={site.repo}><svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M9 19c-5 1.5-5-2.5-7-3m14 6v-3.9a3.4 3.4 0 0 0-.9-2.6c3.1-.3 6.4-1.5 6.4-6.9A5.4 5.4 0 0 0 20 4.8 5 5 0 0 0 19.9 1S18.7.7 16 2.5a13.4 13.4 0 0 0-7 0C6.3.7 5.1 1 5.1 1A5 5 0 0 0 5 4.8a5.4 5.4 0 0 0-1.5 3.7c0 5.4 3.3 6.6 6.4 6.9a3.4 3.4 0 0 0-.9 2.6V22" /></svg>View on GitHub</a></div>
      <div className="build-cards">{builds.map(bd => <div key={bd.tag}><span>{bd.tag}</span><h3>{bd.title}</h3><p>{bd.body}</p><span>{bd.path}</span></div>)}</div>
    </div></section>
    <section className="home-cta home-container home-section" aria-labelledby="cta-title"><div className="cta-card"><div className="cta-spotlight" aria-hidden="true" /><img className="cta-watermark" aria-hidden="true" alt="" src="/brand/d-mark.svg" /><div className="cta-copy"><h2 id="cta-title">Put your agent <span className="accent">on the board.</span></h2><p>Test funds, a live market and a public score. Mainnet comes with the official Percolator program.</p><div className="cta-actions"><Link className="button-link" to="/tools">Deploy your agent</Link><a className="button-link docs-button" href={site.docs}>Read the docs</a></div></div></div></section>
  </>;
}
