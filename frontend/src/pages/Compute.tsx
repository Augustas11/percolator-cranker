import { useEffect, useState } from "react";
import type { ComputeFeedClient, ComputeHistory, ComputeLatest } from "../compute/client";
import { site } from "../site/config";
import "../site/home.css";
import "../site/compute.css";

export const STALE_MS = 30 * 60_000;
const REFRESH_MS = 60_000;

const money = (n: number | null | undefined) => n == null || !Number.isFinite(n) ? "—"
  : n.toLocaleString("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 4, maximumFractionDigits: 4 });
const when = (ms: number) => new Date(ms).toLocaleString("en-GB", { timeZone: "UTC", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }) + " UTC";
const ago = (ms: number, now: number) => {
  const min = Math.max(0, Math.round((now - ms) / 60_000));
  return min < 1 ? "just now" : min < 120 ? `${min} min ago` : `${Math.round(min / 60)} h ago`;
};
const short = (sig: string) => sig.length > 12 ? `${sig.slice(0, 6)}…${sig.slice(-6)}` : sig;
const txUrl = (sig: string) => `https://explorer.solana.com/tx/${encodeURIComponent(sig)}?cluster=devnet`;
const status = (up: boolean) => <span className={up ? "seller-up" : "seller-down"}>{up ? "up" : "down"}</span>;

function Chart({ history }: { history: ComputeHistory }) {
  const { points, from_ms: from, to_ms: to } = history;
  if (points.length === 0) return <p className="note">No prices in the last 7 days.</p>;
  const values = points.map(p => p[1]);
  const high = history.high ?? Math.max(...values), low = history.low ?? Math.min(...values);
  // A flat (or near-flat) history gets a band around it, so the scale never divides by zero.
  const band = Math.max(Math.abs(high) * 0.01, 1e-6);
  let lo = Math.min(low, ...values), hi = Math.max(high, ...values);
  if (hi - lo < band) { lo -= band; hi += band; }
  const W = 640, H = 200, pad = 12, span = Math.max(1, to - from);
  const x = (t: number) => Math.min(W, Math.max(0, (t - from) / span * W));
  const y = (v: number) => H - pad - (v - lo) / (hi - lo) * (H - 2 * pad);
  // The mark is a step: it holds until the next push changes it.
  const d = points.map(([t, v], i) => i === 0 ? `M${x(t)} ${y(v)}` : `H${x(t)} V${y(v)}`).join(" ") + ` H${x(points[points.length - 1][0])}`;
  const label = `Price over the last 7 days: from ${money(values[0])} to ${money(values[values.length - 1])}, low ${money(low)}, high ${money(high)}.`
    + (history.outages.length ? ` ${history.outages.length} seller outage(s) marked.` : "");
  return <figure className="compute-chart">
    <svg role="img" aria-label={label} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none">
      {history.recorder_gaps.map(g => <rect key={`gap-${g.from_ms}`} className="chart-gap" x={x(g.from_ms)} y={0} width={Math.max(1, x(g.to_ms ?? to) - x(g.from_ms))} height={H} />)}
      {history.outages.map(o => <rect key={`${o.provider}-${o.quantization}-${o.from_ms}`} className="chart-outage" x={x(o.from_ms)} y={0} width={Math.max(1, x(o.to_ms ?? to) - x(o.from_ms))} height={H} />)}
      <path d={d} className="chart-line" vectorEffect="non-scaling-stroke" />
    </svg>
    <figcaption className="chart-axis"><span>{when(from)}</span><span>high {money(high)} · low {money(low)}</span><span>{when(to)}</span></figcaption>
  </figure>;
}

export default function Compute({ feed }: { feed: ComputeFeedClient | null }) {
  const [latest, setLatest] = useState<ComputeLatest | null>(null);
  const [history, setHistory] = useState<ComputeHistory | null>(null);
  const [error, setError] = useState("");
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    let active = true;
    setLatest(null);
    setHistory(null);
    setError("");
    if (!feed) return;
    // Settled separately: a failed history.json must not hide a fresh latest.json.
    const refresh = () => Promise.allSettled([feed.latest(), feed.history()]).then(([l, h]) => {
      if (!active) return;
      if (l.status === "fulfilled") setLatest(l.value);
      if (h.status === "fulfilled") setHistory(h.value);
      const failed = [l, h].find(r => r.status === "rejected");
      setError(failed ? (failed.reason instanceof Error ? failed.reason.message : "price feed request failed") : "");
      setNow(Date.now());
    });
    void refresh();
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => { active = false; clearInterval(timer); };
  }, [feed]);

  // Stale if the feed's last push is old by the feed's own clock, the feed
  // itself stopped updating, or the push claims a time after the feed's.
  const staleWhy = !latest ? null
    : now - latest.generated_ms > STALE_MS ? "The price feed has not updated for over 30 minutes."
    : latest.landed_ms > latest.generated_ms + 5 * 60_000 ? "The latest price has a time in the future."
    : latest.generated_ms - latest.landed_ms > STALE_MS ? "No new price for over 30 minutes." : null;
  const openrouter = latest?.source === "openrouter";
  const inTable = latest ? [
    ...latest.pricing_set.map(r => ({ ...r, used: r.priced as number | null })),
    ...latest.sellers.filter(s => s.tier && !s.in_set).map(s => ({ provider: s.provider, quantization: s.quantization, listed: s.blended, used: null, up: s.up })),
  ] : [];
  const others = latest ? latest.sellers.filter(s => !s.in_set && !s.tier) : [];
  const flop = latest?.flop_side;

  return <>
    <h1>Compute price</h1>
    <section aria-labelledby="compute-price-heading">
      <h2 id="compute-price-heading">Qwen3.8-27B{feed?.mock && <> <span className="badge">sample data</span></>}</h2>
      {!feed ? <p>Price feed not configured.</p>
        : !latest && !error ? <p role="status">Loading the price feed…</p>
        : !latest ? <p role="status" className="error">Price feed unreachable ({error}). Try again in a minute.</p>
        : <>
          <div className="compute-price"><strong>{money(latest.mark_usd)}</strong><span>per 1M tokens</span></div>
          <p className="note">Updated {ago(latest.landed_ms, now)} ({when(latest.landed_ms)}).
            {staleWhy && <> <span className="badge compute-stale">stale</span> {staleWhy}</>}</p>
          {error && <p role="status" className="note">Could not refresh ({error}). Showing the last price received.</p>}
        </>}
    </section>

    {latest && <>
      <section aria-labelledby="compute-chart-heading">
        <h2 id="compute-chart-heading">Last 7 days</h2>
        {history ? <Chart history={history} /> : <p className="note">No history yet.</p>}
        {history && (history.outages.length > 0 || history.recorder_gaps.length > 0) && <ul className="compute-legend">
          {history.outages.map(o => <li key={`${o.provider}-${o.quantization}-${o.from_ms}`}><span className="swatch chart-outage" aria-hidden="true" />{o.provider} ({o.quantization}) was down {o.to_ms ? `from ${when(o.from_ms)} to ${when(o.to_ms)}` : `since ${when(o.from_ms)}`}.</li>)}
          {history.recorder_gaps.map(g => <li key={`gap-${g.from_ms}`}><span className="swatch chart-gap" aria-hidden="true" />No price list recorded {g.to_ms ? `from ${when(g.from_ms)} to ${when(g.to_ms)}` : `since ${when(g.from_ms)}`}.</li>)}
        </ul>}
      </section>

      <section aria-labelledby="compute-sellers-heading">
        <h2 id="compute-sellers-heading">Sellers behind the price</h2>
        <p className="note">USD per 1M tokens, input and output mixed {latest.blend_in_out}:1.</p>
        <div className="table-scroll"><table className="compute-table">
          <thead><tr><th scope="col">Seller</th><th scope="col">Precision</th><th scope="col">Listed price</th><th scope="col">Price used</th><th scope="col">Status</th></tr></thead>
          <tbody>{inTable.map(r => <tr key={`${r.provider}-${r.quantization}`}>
            <td>{r.provider}</td><td>{r.quantization}</td><td>{money(r.listed)}</td><td>{r.used === null ? "not used" : money(r.used)}</td><td>{status(r.up)}</td>
          </tr>)}</tbody>
        </table></div>
        {others.length > 0 && <details className="compute-others"><summary>All other sellers ({others.length})</summary>
          <p className="note">Lower precision. They count only when fewer than 2 full-precision sellers are up.</p>
          <div className="table-scroll"><table className="compute-table">
            <thead><tr><th scope="col">Seller</th><th scope="col">Precision</th><th scope="col">Listed price</th><th scope="col">Status</th></tr></thead>
            <tbody>{others.map(s => <tr key={`${s.provider}-${s.quantization}`}><td>{s.provider}</td><td>{s.quantization}</td><td>{money(s.blended)}</td><td>{status(s.up)}</td></tr>)}</tbody>
          </table></div>
        </details>}
      </section>

      {flop && <section aria-labelledby="compute-flop-heading">
        <h2 id="compute-flop-heading">FLOP on-chain price</h2>
        <p className="note">Paid jobs settled on FLOP's chain. G_n is FLOP's unit of compute.</p>
        <div className="facts-grid">{([
          ["FLOP per G_n", flop.flop_per_gn == null ? "—" : flop.flop_per_gn.toPrecision(3)],
          ["Buyers", flop.buyers ?? "—"],
          ["Miners", flop.miners ?? "—"],
          ["From FLOP's own gateway", flop.gateway_share == null ? "—" : `${Math.round(flop.gateway_share * 100)}%`],
        ] as const).map(([label, value]) => <div key={label}><div className="fact-label">{label}</div><div className="fact-value">{value}</div></div>)}</div>
        {flop.stale && <p className="note">Not computed right now: {flop.stale}.</p>}
        {openrouter ? <p>Today this is FLOP's listed rate, not a traded price. It does not set the price above.</p>
          : <p>This sets the price above right now, converted to USD at a fixed stand-in rate.</p>}
      </section>}

      <section aria-labelledby="compute-how-heading">
        <h2 id="compute-how-heading">How the price is made</h2>
        <ul>
          {openrouter ? <>
            <li>Source: OpenRouter's public seller list for Qwen3.8-27B, read every 5 minutes.</li>
            <li>Each seller's price mixes input and output {latest.blend_in_out}:1. Cheaper sellers weigh more (1 ÷ price²), as OpenRouter routes by default.</li>
            <li>Only sellers that are up count: bf16 first; with fewer than 2, fp16, then both, then all.</li>
            <li>With 3 or more sellers, a price under half the median counts as half the median. Now: {latest.clamp ?? "unknown"}.</li>
            <li>No fresh list for 30 minutes: no new price.</li>
          </> : <>
            <li>Source right now: FLOP's on-chain price ({latest.source ?? "unknown"}), converted to USD at a fixed stand-in rate, not a market rate.</li>
            <li>The seller table shows the last OpenRouter list in this push, if any; it does not set this price.</li>
          </>}
          {latest.sig && <li>Each price is written to a Solana devnet test market. Latest: <a href={txUrl(latest.sig)} target="_blank" rel="noreferrer">{short(latest.sig)}</a>.</li>}
          <li>Every price is logged as a signed line; <code>staging verify-push-log</code> recomputes it from the logged sellers.</li>
          <li>Test market, test tokens. Independent of Flop Labs.</li>
          {site.computeRepo && <li>Source code: <a href={site.computeRepo}>{site.computeRepo}</a>.</li>}
        </ul>
        {history && history.changes.length > 0 && <details className="compute-others"><summary>Price changes in the last 7 days ({history.changes.length})</summary><ul>
          {[...history.changes].reverse().map(c => <li key={`${c.t_ms}-${c.sig}`}>{when(c.t_ms)}: {money(c.from)} → {money(c.to)} (<a href={txUrl(c.sig)} target="_blank" rel="noreferrer">{short(c.sig)}</a>)</li>)}
        </ul></details>}
      </section>
    </>}
  </>;
}
