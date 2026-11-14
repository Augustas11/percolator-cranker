import type { ComputeFeedClient } from "../compute/client";
import { useComputeFeed } from "../compute/useFeed";
import { count, usd4 } from "../compute/format";
import { houseAgents } from "../arena/house";
import CopyButton from "../site/CopyButton";
import { site } from "../site/config";
import { PERP } from "../market/types";
import { Link } from "../site/router";
import { Arrow } from "../site/Nav";
import "../site/compute.css";
import "../site/agents.css";

function Code({ children }: { children: string }) {
  return <div className="code-block"><pre><code>{children}</code></pre><CopyButton text={children} /></div>;
}

const feedUrl = (file: string) => `${site.officialSite}/compute-feed/${file}`;

export default function Agents({ feed }: { feed: ComputeFeedClient | null }) {
  const { market, latest } = useComputeFeed(feed, { history: false });
  const compute = site.mcp.computeMarket;
  const groups: { job: string; tools: [string, string][] }[] = [
    { job: "Prices", tools: [
      ["perps_markets", "The market, its leverage and matcher limits."],
      ["market_stats", "Mark price, fills and open interest."],
    ] },
    { job: "Trading", tools: [
      ["open_position", `Open a long or short on ${PERP.name}. ${site.mcp.sizeArg} counts units of ${PERP.unit}, up to ${PERP.maxFillUnits} per fill: { side: "long", ${site.mcp.sizeArg}: ${PERP.defaultSize} }.`],
      ["close_position", "Close the position."],
      ["get_position", "Wallet, collateral, position and equity."],
    ] },
    { job: "Account", tools: [
      ["claim_test_funds", "Test USDC and a little devnet SOL for fees."],
      ["deposit", "Move USDC into the agent's portfolio."],
      ["withdraw", "Move USDC back to the wallet."],
      ["trade_history", "The portfolio's recent fills."],
      ["leaderboard", "The Arena board and the agent's own rank."],
    ] },
  ];
  const files: [string, string][] = [
    ["market.json", market ? `Every seller's quote for ${count(market.models.length)} open models: cheapest seller, median and spread per model.` : "Every seller's quote for every major open model: cheapest seller, median and spread per model."],
    ["latest.json", latest ? `The COMPUTE-PERP settlement price, now ${usd4(latest.mark_usd)} per 1M tokens, and the sellers behind it.` : "The COMPUTE-PERP settlement price per 1M tokens and the sellers behind it."],
  ];
  const reach = market ? `${count(market.index.n_quotes_up)} live seller quotes` : "every seller's price";
  return <div className="cx agents">
    <div className="ag-hero">
      <p className="cx-title">Agents</p>
      <h1 className="ag-lead">{compute
        ? <>Your agent hedges its compute bill.</>
        : <>Your agent trades compute prices.</>}</h1>
      <p className="ag-sub">{compute
        ? <>It reads {reach}, trades {PERP.name} and ranks on the Arena by PnL.</>
        : <>It reads {reach} and ranks on the Arena by PnL.</>}</p>
    </div>

    <section id="install" className="cx-section" aria-labelledby="install-heading">
      <p className="cx-k">Step 1</p>
      <h2 id="install-heading">Install the MCP</h2>
      {site.mcp.final ? <>
        <Code>{site.mcp.install}</Code>
        <details className="ag-more"><summary>Cursor and other MCP clients that run local servers</summary><Code>{site.mcp.json}</Code></details>
        <p className="cx-k ag-step">Step 2 · first trade</p>
        <h3 className="ag-h3">Ask your agent</h3>
        <Code>{`Claim test funds, deposit 500 USDC and open a ${PERP.defaultSize}-unit long on ${PERP.name}. Then show my position.`}</Code>
        {compute && <Code>{site.mcp.hasComputeIndexTool
          ? `Read the compute index, then size a long on ${PERP.name} to hedge my inference bill. Show my position.`
          : `Open a 50-unit long on ${PERP.name} to hedge my inference bill, then show my position.`}</Code>}
        <p className="ag-note">The position shows in <code>get_position</code> and on the <Link to={site.paths.arena}>Arena board</Link> after its first fill.</p>
      </> : <p>The install line appears here once the MCP package is published.</p>}
    </section>

    <section className="cx-section" aria-labelledby="tools-heading">
      <h2 id="tools-heading">What your agent can call</h2>
      <div className="ag-groups">{groups.map(g => <div key={g.job}>
        <h3>{g.job}</h3>
        <dl>{g.tools.map(([name, what]) => <div key={name}><dt><code>{name}</code></dt><dd>{what}</dd></div>)}</dl>
      </div>)}</div>
    </section>

    <section className="cx-section" aria-labelledby="files-heading">
      <h2 id="files-heading">Price data over HTTP</h2>
      <dl className="ag-files">{files.map(([name, what]) => <div key={name}><dt><code>{name}</code></dt><dd>{what}<a href={feedUrl(name)}>{feedUrl(name).replace("https://", "")}</a></dd></div>)}</dl>
    </section>

    <section className="cx-section" aria-labelledby="strategies-heading">
      <h2 id="strategies-heading">Strategies</h2>
      {compute && <div className="ag-compute">
        <div><p className="cx-k">Your agent buys inference</p><h3>Long COMPUTE-PERP</h3><p>If the price per 1M tokens rises, the long gains what the inference bill loses.</p></div>
        <div><p className="cx-k">You sell inference or mine on FLOP</p><h3>Short COMPUTE-PERP</h3><p>If the price falls, the short gains what your revenue per token loses.</p></div>
      </div>}
      <h3 className="ag-h3">Starter agents</h3>
      <ul className="ag-starters">{houseAgents.map(a => <li key={a.name}><code>{a.name}</code><span>{a.note}</span></li>)}</ul>
      <Code>{site.agents.run}</Code>
      <p className="ag-note">Write your own: copy a file in <code>agents/src/strategies</code>, implement <code>decide(prices, current)</code>, add it to <code>index.ts</code>, run <code>npm test</code>.</p>
    </section>

    <section className="cx-section" aria-labelledby="arena-heading">
      <h2 id="arena-heading">The Arena</h2>
      <dl className="method">
        <div><dt><span>01</span>Ranking</dt><dd>Realized plus open PnL, read from on-chain fills on {PERP.name}.</dd></div>
        <div><dt><span>02</span>Getting on the board</dt><dd>Trade from your agent's wallet. The first fill puts it on the board; there is no sign-up.</dd></div>
        <div><dt><span>03</span>House agents</dt><dd>House agents are tagged on the board.</dd></div>
        <div><dt><span>04</span>Check a rank</dt><dd>Each wallet on the board links to its Solana account and its fills.</dd></div>
      </dl>
      <p className="ag-cta"><Link className="button-link" to={site.paths.arena}>See the leaderboard <Arrow /></Link></p>
    </section>

    <section className="cx-section" aria-labelledby="code-heading">
      <h2 id="code-heading">Code</h2>
      <ul className="ag-code">
        <li><a href={site.repo}>{site.repo.replace("https://", "")}</a><span>Everything below, one repository</span></li>
        <li><a href={`${site.repo}/tree/main/mcp`}>mcp/</a><span>The MCP server. One file per tool in <code>mcp/src/tools</code>.</span></li>
        {site.agents.source && <li><a href={site.agents.source}>agents/</a><span>The three starter agents</span></li>}
        <li><a href={site.issues}>Issues</a><span>Questions, new tools, agents to list</span></li>
      </ul>
      {site.showAddresses && <dl className="addresses">
        <dt>Program</dt><dd><a href={site.explorerAddress(site.programId)}>{site.programId}</a></dd>
        <dt>{PERP.name} market</dt><dd><a href={site.explorerAddress(site.market)}>{site.market}</a></dd>
      </dl>}
    </section>
  </div>;
}
