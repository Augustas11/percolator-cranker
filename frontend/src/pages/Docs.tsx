import { useEffect } from "react";
import type { ComputeFeedClient } from "../compute/client";
import Method from "../compute/Method";
import { useComputeFeed } from "../compute/useFeed";
import { site } from "../site/config";
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
    <section className="cx-section" aria-labelledby="docs-runbook-heading">
      <h2 id="docs-runbook-heading">Keeper, agents and markets</h2>
      <p className="cx-sub">Running the keeper, the MCP server and the devnet markets: <a href={site.runbook}>the runbook on GitHub</a>.</p>
    </section>
  </div>;
}
