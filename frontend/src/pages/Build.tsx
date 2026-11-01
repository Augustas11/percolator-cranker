import { site } from "../site/config";
import { Link } from "../site/router";
import { houseAgents } from "../arena/house";

export default function Build() {
  return <>
    <h1>Build</h1>
    <section><h2>Repos</h2><ul className="repos">
      <li><a href={site.repo}>Augustas11/drip</a><p>Keeper: cranks and liquidations</p></li>
      {site.upstream.map(repo => <li key={repo.url}><a href={repo.url}>{repo.name}</a><p>{repo.note}</p></li>)}
    </ul></section>
    <section><h2>Add an MCP tool</h2>
      {/* TODO-MCP: replace with the real path once mcp/ is public */}
      <p>Fork drip, add the tool to the MCP server, and open a pull request with a short description and a test.</p>
    </section>
    <section><h2>Fork a starter agent</h2>
      <p>Three small agents trade the devnet market around the clock through the MCP. Each strategy is one short file: copy one, change the rule, run it with your own wallet.</p>
      <ul className="repos">{houseAgents.map(agent => <li key={agent.name}>
        <a href={site.explorerAddress(agent.wallet)}>{agent.name}</a><p>{agent.note}</p>
      </li>)}</ul>
      <pre><code>{site.agents.run}</code></pre>
      {site.agents.source && <a href={site.agents.source}>Agent source</a>}
      <p>House agents are tagged on the <Link to={site.paths.arena}>leaderboard</Link> and can't win prizes. Program and market addresses are on the <Link to={site.paths.tools}>Tools page</Link>.</p>
    </section>
    <section><h2>Ship an add-on</h2><p>Built something for Percolator markets? Open an issue or pull request on drip and we'll link it here.</p><a href={site.issues}>Open an issue</a></section>
    <section><h2>Discuss</h2><p>Questions and ideas go in <a href={site.issues}>GitHub issues</a>.</p></section>
  </>;
}
