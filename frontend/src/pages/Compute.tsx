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
  let lo = Math.min(...values), hi = Math.max(...values);
  if (hi - lo < hi * 0.01) { lo -= hi * 0.01; hi += hi * 0.01; }
  const W = 640, H = 200, pad = 12, span = Math.max(1, to - from);
  const x = (t: number) => Math.min(W, Math.max(0, (t - from) / span * W));
  const y = (v: number) => H - pad - (v - lo) / (hi - lo) * (H - 2 * pad);
  // The mark is a step: it holds until the next push changes it.
  const d = points.map(([t, v], i) => i === 0 ? `M${x(t)} ${y(v)}` : `H${x(t)} V${y(v)}`).join(" ") + ` H${x(points[points.length - 1][0])}`;
  const label = `Price over the last 7 days: from ${money(values[0])} to ${money(values[values.length - 1])}, low ${money(Math.min(...values))}, high ${money(Math.max(...values))}.`
    + (history.outages.length ? ` ${history.outages.length} seller outage(s) marked.` : "");
  return <figure className="compute-chart">
    <svg role="img" aria-label={label} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none">
      {history.recorder_gaps.map(g => <rect key={`gap-${g.from_ms}`} className="chart-gap" x={x(g.from_ms)} y={0} width={Math.max(1, x(g.to_ms ?? to) - x(g.from_ms))} height={H} />)}
      {history.outages.map(o => <rect key={`${o.provider}-${o.quantization}-${o.from_ms}`} className="chart-outage" x={x(o.from_ms)} y={0} width={Math.max(1, x(o.to_ms ?? to) - x(o.from_ms))} height={H} />)}
      <path d={d} className="chart-line" vectorEffect="non-scaling-stroke" />
    </svg>
    <figcaption className="chart-axis"><span>{when(from)}</span><span>high {money(Math.max(...values))} · low {money(Math.min(...values))}</span><span>{when(to)}</span></figcaption>
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
    const refresh = () => Promise.all([feed.latest(), feed.history()]).then(
      ([l, h]) => { if (active) { setLatest(l); setHistory(h); setError(""); setNow(Date.now()); } },
      e => { if (active) { setError(e instanceof Error ? e.message : "price feed request failed"); setNow(Date.now()); } });
    void refresh();
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => { active = false; clearInterval(timer); };
  }, [feed]);

  const stale = latest !== null && now - latest.landed_ms > STALE_MS;
  const inTable = latest ? [
    ...latest.pricing_set.map(r => ({ ...r, used: r.priced as number | null })),
    ...latest.sellers.filter(s => s.tier && !s.in_set).map(s => ({ provider: s.provider, quantization: s.quantization, listed: s.blended, used: null, up: s.up })),
  ] : [];
  const others = latest ? latest.sellers.filter(s => !s.in_set && !s.tier) : [];
  const flop = latest?.flop_side;
  const mock = feed?.mock ? " (mock data, not devnet)" : "";

  return <>
    <h1>Compute price</h1>
    <p className="compute-label">devnet · unaudited · not affiliated with Flop Labs</p>
    <section aria-labelledby="compute-price-heading">
      <h2 id="compute-price-heading">Price of AI inference{mock}</h2>
      {!feed ? <p>Price feed not configured.</p>
        : !latest && !error ? <p role="status">Loading the price feed…</p>
        : !latest ? <p role="status" className="error">Price feed unreachable ({error}). Try again in a minute.</p>
        : <>
          <div className="compute-price"><strong>{money(latest.mark_usd)}</strong><span>per 1 million tokens of Qwen3.8-27B</span></div>
          <p className="note">Updated {ago(latest.landed_ms, now)} ({when(latest.landed_ms)}).
            {stale && <> <span className="badge compute-stale">stale</span> No new price for over 30 minutes.</>}</p>
          {error && <p role="status" className="note">Could not refresh ({error}). Showing the last price received.</p>}
          <p>A token is a small piece of text. This is what it costs, in US dollars, to have the open model Qwen3.8-27B read and write a million of them, averaged across the sellers listed below.</p>
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
        <p className="note">Prices are US dollars per 1 million tokens, mixing input and output {latest.blend_in_out}:1. Precision is the number format a seller runs the model in; bf16 and fp16 keep full quality.</p>
        <div className="table-scroll"><table className="compute-table">
          <thead><tr><th scope="col">Seller</th><th scope="col">Precision</th><th scope="col">Listed price</th><th scope="col">Price used</th><th scope="col">Status</th></tr></thead>
          <tbody>{inTable.map(r => <tr key={`${r.provider}-${r.quantization}`}>
            <td>{r.provider}</td><td>{r.quantization}</td><td>{money(r.listed)}</td><td>{r.used === null ? "not used" : money(r.used)}</td><td>{status(r.up)}</td>
          </tr>)}</tbody>
        </table></div>
        {others.length > 0 && <details className="compute-others"><summary>All other sellers ({others.length})</summary>
          <p className="note">These run the model at lower precision, so they do not set the price while enough full-precision sellers are up.</p>
          <div className="table-scroll"><table className="compute-table">
            <thead><tr><th scope="col">Seller</th><th scope="col">Precision</th><th scope="col">Listed price</th><th scope="col">Status</th></tr></thead>
            <tbody>{others.map(s => <tr key={`${s.provider}-${s.quantization}`}><td>{s.provider}</td><td>{s.quantization}</td><td>{money(s.blended)}</td><td>{status(s.up)}</td></tr>)}</tbody>
          </table></div>
        </details>}
      </section>

      {flop && <section aria-labelledby="compute-flop-heading">
        <h2 id="compute-flop-heading">FLOP on-chain price</h2>
        <p className="note">FLOP Network settles compute jobs on its own chain. G_n is its unit of compute delivered.</p>
        <div className="facts-grid">{([
          ["FLOP per G_n", flop.flop_per_gn == null ? "—" : flop.flop_per_gn.toPrecision(3)],
          ["Buyers", flop.buyers ?? "—"],
          ["Miners", flop.miners ?? "—"],
          ["From FLOP's own gateway", flop.gateway_share == null ? "—" : `${Math.round(flop.gateway_share * 100)}%`],
        ] as const).map(([label, value]) => <div key={label}><div className="fact-label">{label}</div><div className="fact-value">{value}</div></div>)}</div>
        {flop.stale && <p className="note">Not computed right now: {flop.stale}.</p>}
        <p>On today's devnet this equals FLOP's listed rate, so it is shown for comparison only and does not set the price above.</p>
      </section>}

      <section aria-labelledby="compute-how-heading">
        <h2 id="compute-how-heading">How the price is made</h2>
        <ul>
          <li>The source is OpenRouter's public list of sellers for Qwen3.8-27B, read every 5 minutes.</li>
          <li>Each seller's price mixes input and output tokens {latest.blend_in_out}:1, the mix OpenRouter users send.</li>
          <li>Cheaper sellers count more: each price is weighted by 1 ÷ price², the way OpenRouter spreads requests by default.</li>
          <li>Only sellers that are up count. bf16 sellers go first; with fewer than 2 up, it tries fp16, then both, then everyone.</li>
          <li>With 3 or more sellers, a price under half the middle price counts as half the middle price, so one odd cheap listing cannot drag it down. Now: {latest.clamp ?? "unknown"}.</li>
          <li>If no fresh list arrives for 30 minutes, nothing new is published and the on-chain price simply ages.</li>
          <li>Each new price is written to a test market on Solana devnet a few minutes apart.</li>
          {latest.sig && <li>The latest price is in Solana transaction <a href={txUrl(latest.sig)} target="_blank" rel="noreferrer">{short(latest.sig)}</a>.</li>}
          <li>Every push is also logged as a signed line. Anyone with the log can check it with <code>staging verify-push-log</code>, which checks the signature and recomputes the price from the logged sellers.</li>
          {site.computeRepo && <li>Source code: <a href={site.computeRepo}>{site.computeRepo}</a>.</li>}
        </ul>
        {history && history.changes.length > 0 && <details className="compute-others"><summary>Price changes in the last 7 days ({history.changes.length})</summary><ul>
          {[...history.changes].reverse().map(c => <li key={`${c.t_ms}-${c.sig}`}>{when(c.t_ms)}: {money(c.from)} → {money(c.to)} (<a href={txUrl(c.sig)} target="_blank" rel="noreferrer">{short(c.sig)}</a>)</li>)}
        </ul></details>}
      </section>
    </>}
  </>;
}
