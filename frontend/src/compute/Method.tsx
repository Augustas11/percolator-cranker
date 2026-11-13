import type { ReactNode } from "react";
import type { ComputeLatest, ComputeMarket } from "./client";
import { count, day, txUrl } from "./format";
import { site } from "../site/config";

/** How the compute index and the settlement price are made. Live values where the feed is loaded, the plain rule otherwise. */
export default function Method({ market, latest }: { market: ComputeMarket | null; latest: ComputeLatest | null }) {
  const fixedRate = latest !== null && latest.source !== "openrouter";
  const rows: [string, ReactNode][] = [
    ["Quote", "One seller, one model, one precision, from OpenRouter's public endpoint list."],
    ["Price", <>(15 × input + output) ÷ 16, in USD per 1M tokens.</>],
    ["Up", "OpenRouter reports the endpoint with status 0. 24 h uptime is the share of the model's reads in the last 24 hours where the quote was listed and up; a read that did not list it counts as down."],
    ["Model price", "Median of the quotes that are up. The box spans the 25th to 75th percentile."],
    ["Reads", "The collector reads every model every 5 minutes. The feed rewrites its files every minute and carries the time of the newest read."],
    ["Index", market ? <>Geometric mean of the {count(market.index.n_models)} model medians, each against its own starting price. {market.index.base_ms === null ? "The base is set at the first read where every model has a price." : <>100 = {day(market.index.base_ms)}.</>}{market.models.length > market.index.n_models && " Models marked thin are listed but left out."}</> : "Geometric mean of the model medians, each against its own starting price. 100 = the first read."],
    ...(market?.routed || market?.models.some(m => m.routed) ? [["Routed price", "What a request pays right now if it is spread across the sellers that are up, the way OpenRouter's default routing spreads traffic: cheaper sellers get more (1 ÷ price²), weighted by each seller's recent uptime, with no seller above half the weight."] as [string, ReactNode]] : []),
    ["Stale", "No new read for 30 minutes. The page shows a stale badge next to the time, and a model not read for 30 minutes shows its sellers grey."],
    ["Settlement price", fixedRate
      ? <>FLOP's on-chain price ({latest?.source ?? "unknown"}), converted to USD at a fixed rate.</>
      : <>bf16 sellers first; fewer than 2 up, fp16, then both. Weighted 1 ÷ price². With 3 or more, a quote under half the median counts as half.{latest && <> Clamp now: {latest.clamp ?? "unknown"}.</>}</>],
    ["Check it", <>Each settlement price is written to Solana{latest?.sig ? <> (<a href={txUrl(latest.sig)} target="_blank" rel="noreferrer">latest transaction</a>)</> : null}.</>],
  ];
  return <section id="compute-index" className="cx-section" aria-labelledby="compute-how-heading">
    <h2 id="compute-how-heading">How the compute index is made</h2>
    <dl className="method">{rows.map(([k, v], i) => <div key={k}><dt><span>{String(i + 1).padStart(2, "0")}</span>{k}</dt><dd>{v}</dd></div>)}</dl>
    <p className="cx-fine">Independent of Flop Labs and OpenRouter.{site.computeRepo && <> Source code: <a href={site.computeRepo}>{site.computeRepo}</a>.</>}</p>
  </section>;
}
