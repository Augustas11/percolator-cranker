import { useMemo, useState } from "react";
import type { ComputeFeedClient, ComputeHistory, ComputeLatest, ComputeMarket, MarketModel, MixKey } from "../compute/client";
import { Axis, DEFAULT_MIX, benchmarkRouted, MIN_SERIES_POINTS, RangeStrip, Sparkline, SpreadSwarm, computeSpread, hasMixes, marketScale, medianAt, sinceLabel, typicalPrice, widestModels, type LogScale } from "../compute/charts";
import { addressUrl, ago, clock, count, direction, pct, short, times, txUrl, usd, usd4, usdSmall, when } from "../compute/format";
import { latestStale, marketDataTime, marketStale, modelStale, useComputeFeed } from "../compute/useFeed";
import FeedBoundary from "../compute/FeedBoundary";
import { site } from "../site/config";
import { Link } from "../site/router";
import { Arrow } from "../site/Nav";
import { PERP, perpSubtitle } from "../market/types";
import "../site/compute.css";

export { STALE_MS } from "../compute/useFeed";

const PROVIDERS_SHOWN = 8;
type SortKey = "sellers" | "spread" | "median" | "paid" | "gn";
const sorts: [SortKey, string][] = [["sellers", "Most sellers"], ["spread", "Widest spread"], ["median", "Highest median"], ["paid", "Paid now"], ["gn", "Per unit of compute"]];
const CAPPED = "No seller counts for more than half.";

/** Input:output token mixes, as the switch shows them. */
export const MIXES: [MixKey, string, string][] = [["chat_3_1", "Chat", "3:1"], ["avg_15_1", "Average", "15:1"], ["agent_150_1", "Agent", "150:1"]];
const ratioOf = (mix: MixKey) => MIXES.find(([k]) => k === mix)![2];
const GN_FORMULA = "Price per 1M tokens ÷ compute per token. Compute per token is FLOP's registry value where it lists the model, otherwise 2 × active parameters, taken from the model id or estimated from its expert counts (marked est.); — when unknown.";

function MixSwitch({ mix, onMix }: { mix: MixKey; onMix: (m: MixKey) => void }) {
  return <div className="cx-mix cx-sort" role="group" aria-label="Input:output token mix">{MIXES.map(([key, label, r]) => <button key={key} type="button" aria-pressed={mix === key} onClick={() => onMix(key)}>{label} {r}</button>)}</div>;
}

const Up = ({ up, stale = false }: { up: boolean; stale?: boolean }) => {
  const state = stale ? "stale" : up ? "up" : "down";
  return <span className={`state is-${state}`}><i aria-hidden="true" /><span className="state-t">{state}</span></span>;
};
const Change = ({ value }: { value: number | null }) => <span className={`chg is-${direction(value)}`}>{pct(value)}</span>;
const Stale = ({ why }: { why: string }) => <span className="badge compute-stale" title={why}>stale<span className="sr-only">: {why}</span></span>;

function Skeleton() {
  return <div className="cx-skeleton" aria-busy="true">
    <span className="sr-only" role="status">Loading the compute index</span>
    <div className="sk sk-eyebrow" /><div className="sk sk-level" /><div className="sk sk-line" />
    <div className="sk-rows">{Array.from({ length: 6 }, (_, i) => <div key={i} className="sk sk-row" />)}</div>
  </div>;
}

/** The benchmark model: what a request pays now and the perp mark. */
function Benchmark({ market, latest }: { market: ComputeMarket; latest: ComputeLatest | null }) {
  const b = benchmarkRouted(market);
  const mark = typeof latest?.mark_usd === "number" && Number.isFinite(latest.mark_usd) ? latest.mark_usd : null;
  if (b.price === null && mark === null) return null;
  return <section className="cx-bench" aria-label={`${b.name} prices`}>
    <p className="cx-k">{b.name} · per 1M tokens</p>
    <dl className="cx-bench-nums">
      {b.price !== null && <div className="is-main">
        <dt>What a request pays now</dt>
        <dd><strong>{usd(b.price)}</strong>{b.change1h !== null && <span className="cx-bench-chg">1 h <Change value={b.change1h} /></span>}</dd>
        <Sparkline series={b.series} label={`${b.name} routed price ${sinceLabel(b.series)}.`} className="spark spark-sm" />
      </div>}
      {mark !== null && <div><dt>Perp mark</dt><dd><strong>{usd4(mark)}</strong></dd></div>}
    </dl>
    {site.computeTrade?.startsWith("/") && <Link className="button-link cx-bench-go" to={site.computeTrade}>Trade {PERP.name} <Arrow /></Link>}
  </section>;
}

function Hero({ market, latest, now, error, mix, onMix }: { market: ComputeMarket; latest: ComputeLatest | null; now: number; error: string; mix: MixKey; onMix: (m: MixKey) => void }) {
  const { index } = market;
  const stale = marketStale(market, now);
  const wide = widestModels(market)[0] ?? null;
  const typical = typicalPrice(market, mix);
  const read = marketDataTime(market);
  return <header className={`cx-hero${wide ? "" : " is-empty"}`} aria-labelledby="compute-title">
    <div className="cx-hero-main">
      <h1 id="compute-title" className="cx-title">Compute index</h1>
      {typical ? <>
        <p className="cx-label">Median price to run an open model</p>
        <div className="cx-price"><strong>{usd(typical.median)}</strong><span>per 1M tokens</span></div>
        {hasMixes(market) && <MixSwitch mix={mix} onMix={onMix} />}
      </> : <p className="cx-label">{market.models.length ? "No model has enough sellers for a price right now." : "No prices right now."}</p>}
      <div className="cx-foot">
        {typical && <ul className="cx-proof">
          <li><b>{count(index.n_quotes_up)}</b> live quotes</li>
          <li><b>{count(index.n_providers)}</b> providers</li>
          <li><b>{count(market.models.length)}</b> models</li>
        </ul>}
        {(read !== null || stale) && <p className={`cx-updated${stale ? " is-stale" : ""}`}>{read !== null ? <>Updated {ago(read, now)} · {clock(read)} UTC</> : "No recent update"}{stale && <span className="sr-only"> (stale: {stale})</span>}</p>}
      </div>
      {error && <p className="note" role="status">Could not refresh ({error}). Showing the last data received.</p>}
    </div>
    {wide && <div className="cx-hero-side">
      <p className="cx-spread-line">The same model costs up to <strong>{times(wide.min, wide.max)}</strong> more depending on the seller.</p>
      <SpreadSwarm market={market} />
    </div>}
    <Benchmark market={market} latest={latest} />
  </header>;
}

function Audience({ market, latest }: { market: ComputeMarket; latest: ComputeLatest | null }) {
  const rows: [string, string, string][] = [
    ["Buying inference", `Cheapest seller for each of ${count(market.models.length)} models`, "#board"],
    ["Selling or mining", `Where your price sits against ${count(market.index.n_providers)} providers`, "#providers"],
    ["Trading", latest ? `Hedge the Qwen3.8-27B price with the perp, now ${usd4(latest.mark_usd)}` : "Hedge the Qwen3.8-27B price with the perp", "#perp"],
  ];
  return <nav className="cx-who" aria-label="Jump to">
    {rows.map(([who, what, href]) => <a key={href} href={href}><span className="cx-k">{who}</span><span className="who-what">{what}</span><Arrow /></a>)}
  </nav>;
}

function GnCell({ value, spread, est }: { value: number | null | undefined; spread: ReturnType<typeof computeSpread>; est?: boolean }) {
  const ok = typeof value === "number" && Number.isFinite(value) && value > 0;
  return <span className="c-gn"><span className="u-mobile">per bn G_n </span><b>{ok ? usdSmall(value) : "—"}</b>{ok && est && <small className="gn-est" title="Active parameters estimated from the model's expert counts"> est.</small>}
    {ok && spread && <span className="gn-track" aria-hidden="true">
      {spread.p25 !== null && spread.p75 !== null && <i className="gn-band" style={{ left: `${spread.at(spread.p25)}%`, width: `${Math.max(2, spread.at(spread.p75) - spread.at(spread.p25))}%` }} />}
      <i className="gn-dot" style={{ left: `${spread.at(value)}%` }} />
    </span>}
  </span>;
}

function ModelRow({ model, scale, open, onToggle, now, mix, spread, showPaid }: { model: MarketModel; scale: LogScale; open: boolean; onToggle: () => void; now: number; mix: MixKey; spread: ReturnType<typeof computeSpread> | false; showPaid: boolean }) {
  const stale = modelStale(model, now);
  const median = medianAt(model, mix);
  const gn = model.compute?.gn_per_token;
  const panel = `seller-list-${model.id.replace(/[^a-z0-9]/gi, "-")}`;
  const down = model.quotes.filter(q => !q.up).length;
  const paid = model.routed;
  return <li className={`board-row${open ? " is-open" : ""}${model.thin ? " is-thin" : ""}`}>
    <button type="button" className="board-head" aria-expanded={open} aria-controls={panel} onClick={onToggle}>
      <span className="c-model"><span className="m-name">{model.name}</span><span className="m-family"><span>{model.id}</span>{model.thin && <em className="tag">thin</em>}{stale && <em className="tag">stale</em>}</span></span>
      <span className="c-sellers"><b>{model.sellers_up}</b>/{model.sellers_total}<span className="u-mobile"> sellers up</span></span>
      <span className="c-strip"><RangeStrip model={model} scale={scale} stale={stale} /></span>
      <span className="c-median"><span className="u-mobile">median </span>{usd(median)}</span>
      {showPaid && <span className="c-paid">{paid && <><span className="u-mobile">paid now </span>{usd(paid.price)}{paid.capped && <small className="paid-cap" title={CAPPED}>capped</small>}</>}</span>}
      <span className="c-spread" title="Dearest seller over cheapest">{times(model.min, model.max)}<span className="u-mobile"> spread</span></span>
      {spread !== false && <GnCell value={model.usd_per_bn_gn} spread={spread} est={model.compute?.basis === "2x_active_params_est"} />}
      <span className="c-cheap"><span className="u-mobile">cheapest </span>{model.cheapest ? <><b>{usd(model.cheapest.price)}</b> {model.cheapest.provider}<span className="q"> {model.cheapest.quantization}</span></> : <b>—</b>}</span>
      <span className="c-chg"><span className="u-mobile">24 h </span><Change value={model.change_24h_pct} /></span>
      <span className="c-chev" aria-hidden="true"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round"><path d="M6 9l6 6 6-6" /></svg></span>
    </button>
    {open && <div className="board-panel" id={panel}>
      <div className="panel-meta">
        {paid && <span>Paid now {usd(paid.price)} across {paid.sellers} sellers up</span>}
        <span>Range {usd(model.min)}–{usd(model.max)}</span><span>Middle half {usd(model.p25)}–{usd(model.p75)}</span><span>Median {usd(model.median)}</span>
        {mix !== DEFAULT_MIX && <span>Median at {ratioOf(mix)} {usd(median)}</span>}
        {typeof model.usd_per_bn_gn === "number" && <span>{usdSmall(model.usd_per_bn_gn)} per bn G_n{typeof model.usd_per_bn_gn_min === "number" && typeof model.usd_per_bn_gn_max === "number" && <> (sellers {usdSmall(model.usd_per_bn_gn_min)}–{usdSmall(model.usd_per_bn_gn_max)})</>}</span>}
        {typeof gn === "number" && <span>{gn.toLocaleString("en-US", { maximumSignificantDigits: 3 })} G_n per token{model.compute?.source ? ` (${model.compute.source})` : ""}</span>}
        {down > 0 && <span>{down} down</span>}
        {stale && <span>Not read in the last 30 minutes{model.data_ms ? `; last read ${when(model.data_ms)}` : ""}</span>}
        {model.series.length >= MIN_SERIES_POINTS && <Sparkline series={model.series} label={`${model.name} median ${sinceLabel(model.series)}.`} className="spark spark-sm" />}
      </div>
      {paid && paid.top.length > 0 && <div className="paid-top">
        <h3 className="cx-k">Where the money goes</h3>
        <div className="table-scroll"><table className="cx-table paid-table">
          <caption className="sr-only">{model.name}: where a request's money goes, by seller</caption>
          <thead><tr><th scope="col">Seller</th><th scope="col">Precision</th><th scope="col">Price</th><th scope="col">Share</th></tr></thead>
          <tbody>{paid.top.map((t, i) => <tr key={`${t.provider}-${t.quantization}-${i}`}>
            <th scope="row">{t.provider}<span className="u-mobile q"> {t.quantization}</span></th><td>{t.quantization}</td><td>{usd(t.price)}</td><td>{(t.share * 100).toFixed(t.share < 0.1 ? 1 : 0)}%</td>
          </tr>)}</tbody>
        </table></div>
        {paid.capped && <p className="note">{CAPPED}</p>}
        {paid.uptime_fallback && <p className="note">Uptime was not available, so sellers are weighted by price alone.</p>}
      </div>}
      <div className="table-scroll"><table className="seller-table">
        <caption className="sr-only">{model.name} sellers, cheapest first</caption>
        <thead><tr><th scope="col">Seller</th><th scope="col">Precision</th><th scope="col">Price</th><th scope="col">vs median</th><th scope="col" className="col-pos">Position</th><th scope="col">Status</th><th scope="col"><span className="u-desk">24 h uptime</span><span className="u-mobile">24 h</span></th></tr></thead>
        <tbody>{model.quotes.map((q, i) => <tr key={`${q.provider}-${q.quantization}-${i}`} className={q.up ? undefined : "is-down"}>
          <th scope="row">{q.provider}<span className="u-mobile q"> {q.quantization}</span></th>
          <td>{q.quantization}</td>
          <td>{usd(q.price)}</td>
          <td>{q.price === null || !(model.median > 0) ? "—" : <Change value={(q.price / model.median - 1) * 100} />}</td>
          <td className="col-pos" aria-hidden="true">{q.price !== null && <span className="pos"><i style={{ left: `${scale.at(q.price)}%` }} /><b style={{ left: `${scale.at(model.median)}%` }} /></span>}</td>
          <td><Up up={q.up} stale={stale} /></td>
          <td>{q.uptime_24h_pct === null ? "—" : `${q.uptime_24h_pct.toFixed(1)}%`}</td>
        </tr>)}</tbody>
      </table></div>
    </div>}
  </li>;
}

const MODELS_SHOWN = 20;

/** Ascending, nulls last. */
const byNum = (a: number | null | undefined, b: number | null | undefined, desc = false) =>
  a == null && b == null ? 0 : a == null ? 1 : b == null ? -1 : desc ? b - a : a - b;

function Board({ market, now, mix }: { market: ComputeMarket; now: number; mix: MixKey }) {
  const [all, setAll] = useState(false);
  const [sort, setSort] = useState<SortKey>("sellers");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const scale = useMemo(() => marketScale(market), [market]);
  const spread = useMemo(() => computeSpread(market), [market]);
  const showGn = spread !== null;
  const showPaid = market.models.some(m => m.routed);
  const mixed = hasMixes(market);
  const models = useMemo(() => [...market.models].sort((a, b) =>
    sort === "spread" ? b.max / b.min - a.max / a.min
    : sort === "median" ? byNum(medianAt(a, mix), medianAt(b, mix), true)
    : sort === "paid" ? byNum(a.routed?.price, b.routed?.price, true)
    : sort === "gn" ? byNum(a.usd_per_bn_gn, b.usd_per_bn_gn)
    : b.sellers_total - a.sellers_total || a.median - b.median), [market, sort, mix]);
  const toggle = (id: string) => setOpen(prev => { const next = new Set(prev); if (next.has(id)) next.delete(id); else next.add(id); return next; });
  return <section id="board" className="cx-section" aria-labelledby="compute-board-heading">
    <div className="cx-section-head">
      <div>
        <h2 id="compute-board-heading">Price by model</h2>
        <p className="cx-sub">USD per 1M tokens</p>
      </div>
      <div className="cx-sort" role="group" aria-label="Sort models">{sorts.filter(([key]) => (key !== "gn" || showGn) && (key !== "paid" || showPaid)).map(([key, label]) => <button key={key} type="button" aria-pressed={sort === key} onClick={() => setSort(key)}>{label}</button>)}</div>
    </div>
    {models.length === 0 ? <p className="cx-empty" role="status">No prices right now. The board fills in at the next read.</p> : <>
    <div className="cx-legend" aria-hidden="true">
      <span><i className="lg-whisker" />cheapest to dearest</span><span><i className="lg-box" />middle half</span><span><i className="lg-median" />median</span><span><i className="lg-dot" />one seller</span><span><i className="lg-dot is-down" />seller down</span>
    </div>
    <div className={`board${market.models.some(m => m.change_24h_pct !== null) ? "" : " no-chg"}${showGn ? " has-gn" : ""}${showPaid ? " has-paid" : ""}`}>
      <div className="board-cols" aria-hidden="true">
        <span className="c-model">Model</span><span className="c-sellers">Up</span><span className="c-strip"><Axis scale={scale} /></span><span className="c-median">Median{mixed && mix !== DEFAULT_MIX ? ` ${ratioOf(mix)}` : ""}</span>{showPaid && <span className="c-paid" title="What a request pays now, spread across the sellers that are up">Paid now</span>}<span className="c-spread">Spread</span>{showGn && <span className="c-gn" title={GN_FORMULA}>Per unit of compute<small>$ per bn G_n (FLOP's unit)</small></span>}<span className="c-cheap">Cheapest</span><span className="c-chg">24 h</span><span className="c-chev" />
      </div>
      <ul className="board-list">{(all ? models : models.slice(0, MODELS_SHOWN)).map(m => <ModelRow key={m.id} model={m} scale={scale} now={now} mix={mix} spread={showGn ? spread : false} showPaid={showPaid} open={open.has(m.id)} onToggle={() => toggle(m.id)} />)}</ul>
    </div>
    {models.length > MODELS_SHOWN && <button type="button" className="cx-more" aria-expanded={all} onClick={() => setAll(a => !a)}>{all ? `Show top ${MODELS_SHOWN}` : `Show all ${models.length} models`}</button>}
    </>}
  </section>;
}

function Providers({ market }: { market: ComputeMarket }) {
  const [all, setAll] = useState(false);
  const rows = all ? market.providers : market.providers.slice(0, PROVIDERS_SHOWN);
  const most = Math.max(1, ...market.providers.map(p => p.models));
  return <section id="providers" className="cx-section" aria-labelledby="compute-providers-heading">
    <div className="cx-section-head"><div>
      <h2 id="compute-providers-heading">Providers</h2>
      <p className="cx-sub">{count(market.providers.length)} sellers, by models listed.</p>
    </div></div>
    <div className="table-scroll"><table className="cx-table providers-table">
      <thead><tr><th scope="col">Provider</th><th scope="col">Models</th><th scope="col">Quotes</th><th scope="col">Cheapest in</th><th scope="col">24 h uptime</th></tr></thead>
      <tbody>{rows.map(p => <tr key={p.name}>
        <th scope="row">{p.name}</th>
        <td><span className="bar-cell"><span className="bar" aria-hidden="true"><i style={{ width: `${p.models / most * 100}%` }} /></span>{p.models}</span></td>
        <td>{p.quotes}</td>
        <td className={p.cheapest_in > 0 ? "is-win" : undefined}>{p.cheapest_in > 0 ? `${p.cheapest_in} ${p.cheapest_in === 1 ? "model" : "models"}` : "—"}</td>
        <td>{p.uptime_24h_pct === null ? "—" : `${p.uptime_24h_pct.toFixed(1)}%`}</td>
      </tr>)}</tbody>
    </table></div>
    {market.providers.length > PROVIDERS_SHOWN && <button type="button" className="cx-more" aria-expanded={all} onClick={() => setAll(a => !a)}>{all ? `Show top ${PROVIDERS_SHOWN}` : `Show all ${market.providers.length}`}</button>}
  </section>;
}

const changeKey = (c: { model: string; provider: string; quantization: string }) => `${c.model}\u0000${c.provider}\u0000${c.quantization}`;

/** Sellers with 3 or more changes in the feed's window, as one row each. */
export function frequentRepricers(changes: ComputeMarket["changes"]) {
  const groups = new Map<string, ComputeMarket["changes"]>();
  for (const c of changes) groups.set(changeKey(c), [...(groups.get(changeKey(c)) ?? []), c]);
  return [...groups.values()].filter(g => g.length >= 3).map(g => {
    const prices = g.flatMap(c => [c.from, c.to]);
    return { model: g[0].model, provider: g[0].provider, quantization: g[0].quantization, changes: g.length, min: Math.min(...prices), max: Math.max(...prices) };
  }).sort((a, b) => b.changes - a.changes);
}

function Moves({ market }: { market: ComputeMarket }) {
  const names = new Map(market.models.map(m => [m.id, m.name]));
  const frequent = frequentRepricers(market.changes);
  const grouped = new Set(frequent.map(changeKey));
  const changes = market.changes.filter(c => !grouped.has(changeKey(c))).sort((a, b) => b.at_ms - a.at_ms).slice(0, 12);
  const Row = ({ provider, model, quantization, min, max, n }: { provider: string; model: string; quantization: string; min: number; max: number; n: number }) => <li>
    <span className="mv-what"><b>{provider}</b><span>{names.get(model) ?? model} · {quantization}</span></span>
    <span className="mv-price">{usd(min)}–{usd(max)}</span>
    <span className="dyn-n"><b>{n}</b> changes</span>
  </li>;
  return <section className="cx-section cx-moves" aria-labelledby="compute-moves-heading">
    <h2 id="compute-moves-heading">Price moves</h2>
    <div className="cx-col">
      <h3>Last price changes</h3>
      {changes.length ? <ol className="move-list">{changes.map((c, i) => {
        const delta = c.from > 0 ? (c.to / c.from - 1) * 100 : null;
        return <li key={`${c.at_ms}-${i}`}>
          <time dateTime={new Date(c.at_ms).toISOString()}>{clock(c.at_ms)}</time>
          <span className="mv-what"><b>{names.get(c.model) ?? c.model}</b><span>{c.provider} · {c.quantization}</span></span>
          <span className="mv-price">{usd(c.from)} → {usd(c.to)}</span>
          <Change value={delta} />
        </li>;
      })}</ol> : <p className="note">{frequent.length ? "No other seller has changed a price" : "No seller has changed a price"} since {when(market.window.from_ms)}.</p>}
    </div>
    {frequent.length > 0 && <div className="cx-col">
      <h3>Frequent repricers</h3>
      <p className="cx-sub">3 or more price changes since {when(market.window.from_ms)}, and their range.</p>
      <ol className="move-list dyn-list">{frequent.map(f => <Row key={changeKey(f)} {...f} n={f.changes} />)}</ol>
    </div>}
    {(market.dynamic.length > 0 || frequent.length === 0) && <div className="cx-col">
      <h3>Dynamic pricers</h3>
      <p className="cx-sub">Sellers with more than one price in the last 24 h, and their range.</p>
      {market.dynamic.length ? <ol className="move-list dyn-list">{[...market.dynamic].sort((a, b) => b.changes_24h - a.changes_24h).map(d => <Row key={changeKey(d)} {...d} n={d.changes_24h} />)}</ol>
        : <p className="note">No seller repriced more than once in 24 h.</p>}
    </div>}
  </section>;
}

function Perp({ latest, history, now }: { latest: ComputeLatest; history: ComputeHistory | null; now: number }) {
  const stale = latestStale(latest, now);
  const trade = site.computeTrade;
  return <article id="perp" className="cx-panel" aria-labelledby="compute-perp-heading">
    <p className="cx-k">{PERP.name} · {perpSubtitle}</p>
    <h2 id="compute-perp-heading">The price the perp settles on</h2>
    <div className="perp-price"><strong>{usd4(latest.mark_usd)}</strong><span>per 1M tokens</span></div>
    {history && history.points.length >= 2 && history.to_ms - history.from_ms >= 86_400_000 && <figure className="perp-hist">
      <Sparkline step min={2} series={history.points} label={`Settlement price over 7 days: low ${usd4(history.low)}, high ${usd4(history.high)}.`} className="spark spark-sm" />
      <figcaption>7 d · low {usd4(history.low)} · high {usd4(history.high)}</figcaption>
    </figure>}
    <p className="cx-k">{latest.pricing_set.filter(r => r.up).length} of {latest.pricing_set.length} full-precision sellers up</p>
    <ul className="perp-sellers" aria-label="Sellers in the settlement price">{latest.pricing_set.map(r => <li key={`${r.provider}-${r.quantization}`}>
      <span>{r.provider} <span className="q">{r.quantization}</span></span><b>{usd4(r.priced)}</b><Up up={r.up} />
    </li>)}</ul>
    <p className="cx-updated">Pushed {ago(latest.landed_ms, now)} ({when(latest.landed_ms)}){latest.sig && <> · <a href={txUrl(latest.sig)} target="_blank" rel="noreferrer">tx {short(latest.sig)}</a></>}{stale && <> · <Stale why={stale} /></>}</p>
    <div className="panel-actions">
      {trade ? trade.startsWith("/") ? <Link className="button-link" to={trade}>Trade {PERP.name} <Arrow /></Link> : <a className="button-link" href={trade}>Trade {PERP.name} <Arrow /></a>
        : latest.market && <a className="button-link outline" href={addressUrl(latest.market)} target="_blank" rel="noreferrer">Market account <Arrow /></a>}
      <span className="trade-note">Test tokens</span>
    </div>
  </article>;
}

/** "Last 6 hours" from a window in seconds. */
export const windowLabel = (s: number) => s % 3600 === 0 ? `Last ${s / 3600} ${s === 3600 ? "hour" : "hours"}` : `Last ${Math.round(s / 60)} minutes`;

const ok = (v: number | null | undefined): v is number => typeof v === "number" && Number.isFinite(v) && v > 0;
/** The implied FLOP price is about a millionth of a dollar; per 1 million FLOP it reads as dollars and cents. */
const perMFlop = (v: number | null | undefined) => usdSmall(v == null ? v : v * 1e6);

function Flop({ latest, market, now }: { latest: ComputeLatest; market: ComputeMarket | null; now: number }) {
  const flop = latest.flop_side;
  const parity = market?.flop?.parity ?? null;
  const implied = market?.flop?.implied_flop_usd;
  const basisId = market?.flop?.basis_model ?? null;
  const basis = basisId ? market?.models.find(m => m.id === basisId)?.name ?? basisId : null;
  const routed = parity?.implied_flop_usd_routed ?? market?.flop?.implied_flop_usd_routed;
  const list = parity?.implied_flop_usd ?? implied;
  const gnPrice = parity?.market_usd_per_bn_gn;
  // The same model's list-price figure, the one the board's per-unit column shows, so the two reconcile.
  const gnList = basisId ? market?.models.find(m => m.id === basisId)?.usd_per_bn_gn : undefined;
  const rate = parity?.flop_rate_flop_per_gn ?? flop.flop_per_gn;
  const live = parity !== null && (ok(routed) || ok(list));
  const headline = ok(routed) ? routed : list;
  return <article className="cx-panel" aria-labelledby="compute-flop-heading">
    <p className="cx-k">FLOP Network · on-chain</p>
    <h2 id="compute-flop-heading">{live ? "FLOP parity" : "FLOP price per unit of compute"}{latest.source !== "openrouter" && <> <span className="badge compute-stale" title="The perp settles on this rate right now, converted to USD at a fixed rate.">perp source<span className="sr-only">: the perp settles on this rate right now, converted to USD at a fixed rate.</span></span></>}</h2>
    {live ? <div className="flop-parity">
      <div className="perp-price"><strong>{perMFlop(headline)}</strong><span>per 1M FLOP, {ok(routed) ? "from what requests pay" : "from list prices"}</span></div>
      {ok(routed) && ok(list) && <p className="flop-list"><b>{perMFlop(list)}</b> per 1M FLOP, from list prices</p>}
      <p className="perp-why">The FLOP price at which FLOP's rate equals what the open market pays for the same compute.</p>
      <dl className="flop-facts">
        {ok(gnPrice) && <div><dt>Market price of compute</dt><dd>{usdSmall(gnPrice)}</dd><small>$ per bn G_n{basis ? `, ${basis}` : ""}, {ok(routed) ? "from what requests pay" : "from list prices"}</small></div>}
        {ok(gnPrice) && ok(gnList) && ok(routed) && <div><dt>At list prices</dt><dd>{usdSmall(gnList)}</dd><small>$ per bn G_n, median list price, as on the board</small></div>}
        {ok(rate) && <div><dt>FLOP's rate</dt><dd>{rate.toPrecision(3)}</dd><small>FLOP per G_n</small></div>}
      </dl>
      {parity?.as_of_ms != null && <p className="cx-updated">Parity read {ago(parity.as_of_ms, now)}</p>}
    </div> : <>
      <div className="perp-price"><strong>{flop.flop_per_gn == null ? "—" : flop.flop_per_gn.toPrecision(3)}</strong><span>FLOP per G_n</span></div>
      {ok(implied) && <div className="flop-implied">
        <p className="cx-k">Implied FLOP price · derived</p>
        <div className="perp-price"><strong>{perMFlop(implied)}</strong><span>per 1M FLOP</span></div>
        <p className="perp-why">The FLOP price at which FLOP's rate equals what the open market pays for the same compute{basis ? <> on {basis}</> : null}.</p>
      </div>}
    </>}
    <p className="cx-k flop-window">{windowLabel(flop.window_s ?? site.flopWindowS)}</p>
    <dl className="flop-facts">{([
      ["Buyer accounts", flop.buyers ?? "—"],
      ["Miners", flop.miners ?? "—"],
      ["Jobs settled", flop.prints ?? "—"],
    ] as const).map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
    {flop.stale && <p className="note">Not computed right now: {flop.stale}.</p>}
  </article>;
}

export default function Compute({ feed }: { feed: ComputeFeedClient | null }) {
  const { latest, history, market, error, failed, loaded, now } = useComputeFeed(feed);
  const [mix, setMix] = useState<MixKey>(DEFAULT_MIX);
  if (!feed) return <div className="cx"><h1>Compute index</h1><p className="note" role="status">The compute index is unreachable right now. Try again in a minute.</p></div>;
  const nothing = !latest && !market;
  return <div className="cx">
    {!loaded && nothing ? <><h1 className="sr-only">Compute index</h1><Skeleton /></>
      : market ? <FeedBoundary data={market} name="Compute index"><Hero market={market} latest={latest} now={now} error={failed.market ? error : ""} mix={mix} onMix={setMix} /></FeedBoundary>
      : <header className="cx-hero is-empty"><div className="cx-hero-main"><h1>Compute index</h1><p role="status" className="error">Market feed unreachable ({failed.market ? error : "no data"}). Try again in a minute.</p></div></header>}
    {market && <FeedBoundary data={market}><Audience market={market} latest={latest} /></FeedBoundary>}
    {market && <FeedBoundary data={market} name="Price by model"><Board market={market} now={now} mix={mix} /></FeedBoundary>}
    {(latest || failed.latest) && <div className="cx-panels">
      {latest ? <><FeedBoundary data={latest} name={PERP.name}><Perp latest={latest} history={history} now={now} /></FeedBoundary><FeedBoundary data={latest} name="FLOP price"><Flop latest={latest} market={market} now={now} /></FeedBoundary></>
        : <article id="perp" className="cx-panel"><h2>Perp settlement price</h2><p role="status" className="error">Price feed unreachable ({error}). Try again in a minute.</p></article>}
    </div>}
    {market && <div className="cx-pair"><FeedBoundary data={market} name="Providers"><Providers market={market} /></FeedBoundary><FeedBoundary data={market} name="Price moves"><Moves market={market} /></FeedBoundary></div>}
    {(market || latest) && <p className="cx-fine cx-how"><Link to={`${site.paths.docs}#compute-index`}>How these prices are made</Link></p>}
  </div>;
}
