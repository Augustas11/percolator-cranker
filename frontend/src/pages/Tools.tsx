import { useEffect, useRef, useState } from "react";
import { site } from "../site/config";

function Code({ children }: { children: string }) {
  const [label, setLabel] = useState("Copy");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const active = useRef(false);
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; clearTimeout(timer.current); };
  }, []);
  async function copy() {
    try {
      if (!navigator.clipboard?.writeText) throw new Error("Copy failed");
      await navigator.clipboard.writeText(children);
      if (!active.current) return;
      setLabel("Copied");
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setLabel("Copy"), 2000);
    } catch { if (active.current) setLabel("Copy failed"); }
  }
  return <div className="code-block"><pre><code>{children}</code></pre><button type="button" onClick={() => void copy()} aria-live="polite">{label}</button></div>;
}

export default function Tools() {
  return <>
    <h1>Tools</h1>
    <section><h2>MCP</h2>
      <p>Add drip to Claude Code:</p><Code>{site.mcp.install}</Code>
      <p>Other MCP clients:</p><Code>{site.mcp.json}</Code>
      {!site.mcp.final && <p className="note">Package not published yet. Command will change.</p>}
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
