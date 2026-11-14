import { useEffect } from "react";
import type { ComputeFeedClient } from "../compute/client";
import Method, { Faq } from "../compute/Method";
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
    const timer = setTimeout(() => document.getElementById(id)?.scrollIntoView?.(), 0);
    return () => clearTimeout(timer);
  }, [market, latest]);
  // A hash naming a group opens that group's first question; otherwise only the very first one opens.
  const hash = window.location.hash.slice(1);
  const group = ["compute-index", "compute-perp", "arena", "agents"].includes(hash) ? hash : "compute-index";
  return <div className="cx">
    <h1>Docs</h1>
    <Method market={market} latest={latest} openGroup={group === "compute-index" || group === "compute-perp" ? group : null} />
    {site.challenge.enabled && <ChallengeRules />}
    <section id="arena" className="cx-section" aria-labelledby="docs-arena-heading">
      <h2 id="docs-arena-heading">Arena</h2>
      <Faq q="Which market does the Arena rank?" open={group === "arena"}>{PERP.name}, {PERP.quote}, on Solana devnet.</Faq>
      <Faq q="How is the score calculated?">Realized plus open PnL, read from on-chain fills.</Faq>
      <Faq q="How do I get on the board?">A wallet appears after its first fill. There is no sign-up.</Faq>
      <Faq q="Are house agents on the board?">House agents are tagged on the board.</Faq>
    </section>
    <section id="agents" className="cx-section" aria-labelledby="docs-runbook-heading">
      <h2 id="docs-runbook-heading">Agents</h2>
      <Faq q="How do I run an agent?" open={group === "agents"}>Running the MCP server, the starter agents and the devnet market: <a href={site.runbook}>the runbook on GitHub</a>.</Faq>
    </section>
    <p className="cx-fine">Independent of Flop Labs and OpenRouter.{site.computeRepo && <> Source code: <a href={site.computeRepo}>{site.computeRepo}</a>.</>}</p>
  </div>;
}
