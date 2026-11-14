import type { ReactNode } from "react";
import type { ComputeLatest, ComputeMarket } from "./client";
import { count, day, txUrl } from "./format";
import { PERP } from "../market/types";

/** One FAQ row: a native details element, so it opens by click or keyboard with no state. */
export function Faq({ q, open, children }: { q: string; open?: boolean; children: ReactNode }) {
  return <details className="faq" open={open}><summary>{q}</summary><div className="faq-a">{children}</div></details>;
}

/** How the compute index and the settlement price are made, as FAQ groups. Live values where the feed is loaded, the plain rule otherwise. */
export default function Method({ market, latest, openGroup }: { market: ComputeMarket | null; latest: ComputeLatest | null; openGroup: "compute-index" | "compute-perp" | null }) {
  const fixedRate = latest !== null && latest.source !== "openrouter";
  const routed = !!(market?.routed || market?.models.some(m => m.routed));
  return <>
    <section id="compute-index" className="cx-section" aria-labelledby="compute-how-heading">
      <h2 id="compute-how-heading">Compute index</h2>
      <Faq q="What is a quote?" open={openGroup === "compute-index"}>One seller, one model, one precision, from OpenRouter's public endpoint list.</Faq>
      <Faq q="How do input and output prices become one price?">(15 × input + output) ÷ 16, in USD per 1M tokens.</Faq>
      <Faq q="When does a seller count as up?">OpenRouter reports the endpoint with status 0. 24 h uptime is the share of the model's reads in the last 24 hours where the quote was listed and up; a read that did not list it counts as down.</Faq>
      <Faq q="How is a model's price set?">Median of the quotes that are up.</Faq>
      <Faq q="How often are prices read?">The collector reads every model every 5 minutes. The feed rewrites its files every minute and carries the time of the newest read.</Faq>
      <Faq q="How is the index calculated?">{market ? <>Geometric mean of the {count(market.index.n_models)} model medians, each against its own starting price. {market.index.base_ms === null ? "The base is set at the first read where every model has a price." : <>100 = {day(market.index.base_ms)}.</>}{market.models.length > market.index.n_models && " Models marked thin are listed but left out."}</> : "Geometric mean of the model medians, each against its own starting price. 100 = the first read."}</Faq>
      {routed && <Faq q="What is the routed price?">What a request pays right now if it is spread across the sellers that are up, the way OpenRouter's default routing spreads traffic: cheaper sellers get more (1 ÷ price²), weighted by each seller's recent uptime, with no seller above half the weight.</Faq>}
      <Faq q="When is data marked stale?">No new read for 30 minutes. The update time turns red, and a model not read for 30 minutes shows its sellers grey.</Faq>
    </section>
    <section id="compute-perp" className="cx-section" aria-labelledby="compute-perp-heading">
      <h2 id="compute-perp-heading">{PERP.name}</h2>
      <Faq q={`What does ${PERP.name} settle on?`} open={openGroup === "compute-perp"}>{fixedRate
        ? <>FLOP's on-chain price ({latest?.source ?? "unknown"}), converted to USD at a fixed rate.</>
        : <>bf16 sellers first; fewer than 2 up, fp16, then both. Weighted 1 ÷ price². With 3 or more, a quote under half the median counts as half.{latest && <> Clamp now: {latest.clamp ?? "unknown"}.</>}</>}</Faq>
      <Faq q="How can I check a settlement price?">Each settlement price is written to Solana{latest?.sig ? <> (<a href={txUrl(latest.sig)} target="_blank" rel="noreferrer">latest transaction</a>)</> : null}.</Faq>
    </section>
  </>;
}
