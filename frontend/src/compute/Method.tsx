import type { ReactNode } from "react";
import type { ComputeLatest, ComputeMarket } from "./client";
import { count, day } from "./format";
import { site } from "../site/config";

/** How the compute index and the settlement price are made. Live values where the feed is loaded, the plain rule otherwise. */
export default function Method({ market, latest }: { market: ComputeMarket | null; latest: ComputeLatest | null }) {
  const standin = latest !== null && latest.source !== "openrouter";
  const rows: [string, ReactNode][] = [
    ["Quote", "One seller, one model, one precision, from OpenRouter's public endpoint list."],
    ["Price", <>(15 × input + output) ÷ 16, in USD per 1M tokens.</>],
    ["Up", "OpenRouter reports the endpoint with status 0. 24 h uptime is the share of reads it was up."],
    ["Model price", "Median of the quotes that are up. The box spans the 25th to 75th percentile."],
    ["Index", market ? <>Geometric mean of the {count(market.index.n_models)} model medians, each against its own starting price. 100 = {day(market.index.base_ms)}.{market.models.length > market.index.n_models && " Models marked thin are listed but left out."}</> : "Geometric mean of the model medians, each against its own starting price. 100 = the first read."],
    ["Stale", "No new read for 30 minutes. The page shows a stale badge next to the time."],
    ["Settlement price", standin
      ? <>FLOP's on-chain price ({latest?.source ?? "unknown"}), converted to USD at a fixed stand-in rate.</>
      : <>bf16 sellers first; fewer than 2 up, fp16, then both. Weighted 1 ÷ price². With 3 or more, a quote under half the median counts as half.{latest && <> Clamp now: {latest.clamp ?? "unknown"}.</>}</>],
    ["Check it", <>Each settlement price is a signed log line; <code>staging verify-push-log</code> recomputes it from the logged sellers.</>],
  ];
  return <section id="compute-index" className="cx-section" aria-labelledby="compute-how-heading">
    <h2 id="compute-how-heading">How the compute index is made</h2>
    <dl className="method">{rows.map(([k, v], i) => <div key={k}><dt><span>{String(i + 1).padStart(2, "0")}</span>{k}</dt><dd>{v}</dd></div>)}</dl>
    <p className="cx-fine">Independent of Flop Labs and OpenRouter.{site.computeRepo && <> Source code: <a href={site.computeRepo}>{site.computeRepo}</a>.</>}</p>
  </section>;
}
