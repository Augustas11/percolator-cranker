import CopyButton from "../site/CopyButton";
import { site } from "../site/config";

function Code({ children }: { children: string }) {
  return <div className="code-block"><pre><code>{children}</code></pre><CopyButton text={children} /></div>;
}

export default function Tools() {
  return <>
    <h1>Tools</h1>
    <section><h2>MCP</h2>
      {site.mcp.final ? <>
        <p>Add drip to Claude Code:</p><Code>{site.mcp.install}</Code>
        <p>Other MCP clients:</p><Code>{site.mcp.json}</Code>
      </> : <p>The MCP package is not published yet. The install command will appear here when it is.</p>}
    </section>
    <section><h2>Faucet</h2>{site.faucet ? <a href={site.faucet}>Get test funds</a> : <p>Ask your agent to call the MCP tool claim_test_funds. A web faucet is coming.</p>}</section>
    <section><h2>Keeper CLI</h2>
      <p>drip runs cranks and liquidations for Percolator markets. Build and run it from source:</p>
      <Code>{site.cli}</Code><a href={site.runbook}>Runbook</a>
    </section>
    <section><h2>TypeScript</h2>
      <p>Read the devnet market with @solana/web3.js:</p><Code>{site.sdk.snippet}</Code>
      {!site.sdk.final && <p className="note">SDK package not published yet.</p>}
    </section>
    <section><h2>Addresses</h2><dl className="addresses">
      <dt>Program</dt><dd><a href={site.explorerAddress(site.programId)}>{site.programId}</a></dd>
      <dt>Market</dt><dd><a href={site.explorerAddress(site.market)}>{site.market}</a></dd>
    </dl></section>
  </>;
}
