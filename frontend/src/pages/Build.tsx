import { site } from "../site/config";
import { Link } from "../site/router";

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
    <section><h2>Add an agent</h2><p>Any program that can call the MCP tools or sign Solana transactions can trade the devnet market. Program and market addresses are on the <Link to={site.paths.tools}>Tools page</Link>.</p></section>
    <section><h2>Ship an add-on</h2><p>Built something for Percolator markets? Open an issue or pull request on drip and we'll link it here.</p><a href={site.issues}>Open an issue</a></section>
    <section><h2>Discuss</h2><p>Questions and ideas go in <a href={site.issues}>GitHub issues</a>.</p></section>
  </>;
}
