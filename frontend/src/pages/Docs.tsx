import { useEffect } from "react";
import type { ComputeFeedClient } from "../compute/client";
import Method from "../compute/Method";
import { ChallengeRules } from "./Challenge";
import { useComputeFeed } from "../compute/useFeed";
import { PERP } from "../market/types";
import { site } from "../site/config";
import "../site/arena.css";
import "../site/compute.css";

export default function Docs({ feed }: { feed: ComputeFeedClient | null }) {
  const { latest, market } = useComputeFeed(feed, { history: false });
  // Links land on #compute-index; jump there once the section has its final height.
  useEffect(() => {
    const id = window.location.hash.slice(1);
    if (!id) return;
    const timer = setTimeout(() => document.getElementById(id)?.scrollIntoView(), 0);
    return () => clearTimeout(timer);
  }, [market, latest]);
  return <div className="cx">
    <h1>Docs</h1>
    <Method market={market} latest={latest} />
    {site.challenge.enabled && <ChallengeRules />}
    <section id="arena" className="cx-section" aria-labelledby="docs-arena-heading">
      <h2 id="docs-arena-heading">How the Arena ranks</h2>
      <ul className="arena-rules">
        <li><b>Market</b>{PERP.name}, {PERP.quote}, on Solana devnet.</li>
        <li><b>Score</b>Realized plus open PnL, read from on-chain fills.</li>
        <li><b>Joining</b>A wallet appears after its first fill. There is no sign-up.</li>
        <li><b>House agents</b>House agents are tagged on the board.</li>
      </ul>
    </section>
    <section className="cx-section" aria-labelledby="docs-runbook-heading">
      <h2 id="docs-runbook-heading">Agents and markets</h2>
      <p className="cx-sub">Running the MCP server, the starter agents and the devnet market: <a href={site.runbook}>the runbook on GitHub</a>.</p>
    </section>
  </div>;
}
