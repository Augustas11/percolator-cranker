import { useMemo, useState, type ReactNode } from "react";
import type { ComputeFeedClient, ComputeHistory, ComputeLatest, ComputeMarket, MarketModel } from "../compute/client";
import { Axis, MIN_SERIES_POINTS, RangeStrip, Sparkline, SpreadSwarm, marketScale, sinceLabel, widest, type LogScale } from "../compute/charts";
import { addressUrl, ago, clock, count, day, direction, pct, short, times, txUrl, usd, usd4, when } from "../compute/format";
import { latestStale, marketStale, useComputeFeed } from "../compute/useFeed";
import { site } from "../site/config";
import { Arrow } from "../site/Nav";
import "../site/compute.css";

export { STALE_MS } from "../compute/useFeed";

const PROVIDERS_SHOWN = 8;
type SortKey = "sellers" | "spread" | "median";
const sorts: [SortKey, string][] = [["sellers", "Most sellers"], ["spread", "Widest spread"], ["median", "Highest median"]];

const Up = ({ up }: { up: boolean }) => <span className={`state ${up ? "is-up" : "is-down"}`}><i aria-hidden="true" /><span className="state-t">{up ? "up" : "down"}</span></span>;
const Change = ({ value }: { value: number | null }) => <span className={`chg is-${direction(value)}`}>{pct(value)}</span>;
const Stale = ({ why }: { why: string }) => <span className="stale"><span className="badge compute-stale">stale</span> {why}</span>;

function Skeleton() {
  return <div className="cx-skeleton" aria-busy="true">
    <span className="sr-only" role="status">Loading the compute index</span>
    <div className="sk sk-eyebrow" /><div className="sk sk-level" /><div className="sk sk-line" />
    <div className="sk-rows">{Array.from({ length: 6 }, (_, i) => <div key={i} className="sk sk-row" />)}</div>
  </div>;
}

function Hero({ market, now, error }: { market: ComputeMarket; now: number; error: string }) {
  const { index } = market;
  const stale = marketStale(market, now);
  const wide = widest(market);
  const hasSeries = index.series.length >= MIN_SERIES_POINTS;
  return <header className="cx-hero" aria-labelledby="compute-title">
    <div className="cx-hero-main">
      <p className="eyebrow">OPEN-MODEL INFERENCE · USD PER 1M TOKENS</p>
      <h1 id="compute-title">Compute index</h1>
      <div className="cx-level">
        <strong>{index.level.toFixed(2)}</strong>
        {(index.change_24h_pct !== null || index.change_7d_pct !== null) && <dl className="cx-changes">
          {index.change_24h_pct !== null && <div><dt>24 h</dt><dd><Change value={index.change_24h_pct} /></dd></div>}
          {index.change_7d_pct !== null && <div><dt>7 d</dt><dd><Change value={index.change_7d_pct} /></dd></div>}
        </dl>}
      </div>
      <p className="cx-base">Indexed from {when(index.base_ms)}.{index.change_24h_pct === null && " A 24 h change prints once the index is a day old."}</p>
      <ul className="cx-scale" aria-label="Scale">
        <li><strong>{count(index.n_quotes)}</strong> quotes</li>
        <li><strong>{count(index.n_providers)}</strong> providers</li>
        <li><strong>{count(index.n_models)}</strong> models</li>
        <li><strong>{count(index.n_quotes_up)}</strong> up now</li>
      </ul>
      <p className="cx-updated">Updated {ago(market.generated_ms, now)} · {clock(market.generated_ms)} UTC{stale && <> · <Stale why={stale} /></>}</p>
      {error && <p className="note" role="status">Could not refresh ({error}). Showing the last data received.</p>}
    </div>
    <div className="cx-hero-side">
      {wide && <div className="cx-widest">
        <span className="cx-k">Widest spread</span>
        <strong>{times(wide.min, wide.max)}</strong>
        <span>{wide.name}: {usd(wide.min)} to {usd(wide.max)} across {wide.sellers_total} sellers</span>
      </div>}
      {hasSeries
        ? <figure className="cx-index-line"><Sparkline series={index.series} label={`Index level ${sinceLabel(index.series)}: from ${index.series[0][1].toFixed(2)} to ${index.level.toFixed(2)}.`} className="spark spark-lg" /><figcaption>Index {sinceLabel(index.series)}</figcaption></figure>
        : <><p className="cx-k">Every live quote against its model's median</p><SpreadSwarm market={market} /></>}
    </div>
  </header>;
}

function ModelRow({ model, scale, open, onToggle }: { model: MarketModel; scale: LogScale; open: boolean; onToggle: () => void }) {
  const panel = `seller-list-${model.id.replace(/[^a-z0-9]/gi, "-")}`;
  const down = model.quotes.filter(q => !q.up).length;
  return <li className={`board-row${open ? " is-open" : ""}${model.thin ? " is-thin" : ""}`}>
    <button type="button" className="board-head" aria-expanded={open} aria-controls={panel} onClick={onToggle}>
      <span className="c-model"><span className="m-name">{model.name}</span><span className="m-family"><span>{model.id}</span>{model.thin && <em className="tag">thin</em>}</span></span>
      <span className="c-sellers"><b>{model.sellers_up}</b>/{model.sellers_total}<span className="u-mobile"> sellers up</span></span>
      <span className="c-strip"><RangeStrip model={model} scale={scale} /></span>
      <span className="c-median"><span className="u-mobile">median </span>{usd(model.median)}</span>
      <span className="c-spread" title="Dearest seller over cheapest">{times(model.min, model.max)}<span className="u-mobile"> spread</span></span>
      <span className="c-cheap"><span className="u-mobile">cheapest </span><b>{usd(model.cheapest.price)}</b> {model.cheapest.provider}<span className="q"> {model.cheapest.quantization}</span></span>
      <span className="c-chg"><span className="u-mobile">24 h </span><Change value={model.change_24h_pct} /></span>
      <span className="c-chev" aria-hidden="true"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round"><path d="M6 9l6 6 6-6" /></svg></span>
    </button>
    {open && <div className="board-panel" id={panel}>
      <div className="panel-meta">
        <span>Range {usd(model.min)}–{usd(model.max)}</span><span>Middle half {usd(model.p25)}–{usd(model.p75)}</span><span>Median {usd(model.median)}</span>
        {down > 0 && <span>{down} down</span>}
        {model.series.length >= MIN_SERIES_POINTS && <Sparkline series={model.series} label={`${model.name} median ${sinceLabel(model.series)}.`} className="spark spark-sm" />}
      </div>
      <div className="table-scroll"><table className="seller-table">
        <caption className="sr-only">{model.name} sellers, cheapest first</caption>
        <thead><tr><th scope="col">Seller</th><th scope="col">Precision</th><th scope="col">Price</th><th scope="col">vs median</th><th scope="col" className="col-pos">Position</th><th scope="col">Status</th><th scope="col"><span className="u-desk">24 h uptime</span><span className="u-mobile">24 h</span></th></tr></thead>
        <tbody>{model.quotes.map((q, i) => <tr key={`${q.provider}-${q.quantization}-${i}`} className={q.up ? undefined : "is-down"}>
          <th scope="row">{q.provider}<span className="u-mobile q"> {q.quantization}</span></th>
          <td>{q.quantization}</td>
          <td>{usd(q.price)}</td>
          <td>{q.price === null || !(model.median > 0) ? "—" : <Change value={(q.price / model.median - 1) * 100} />}</td>
          <td className="col-pos" aria-hidden="true">{q.price !== null && <span className="pos"><i style={{ left: `${scale.at(q.price)}%` }} /><b style={{ left: `${scale.at(model.median)}%` }} /></span>}</td>
          <td><Up up={q.up} /></td>
          <td>{q.uptime_24h_pct === null ? "—" : `${q.uptime_24h_pct.toFixed(1)}%`}</td>
        </tr>)}</tbody>
      </table></div>
    </div>}
  </li>;
}

function Board({ market }: { market: ComputeMarket }) {
  const [sort, setSort] = useState<SortKey>("sellers");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const scale = useMemo(() => marketScale(market), [market]);
  const models = useMemo(() => [...market.models].sort((a, b) =>
    sort === "spread" ? b.max / b.min - a.max / a.min : sort === "median" ? b.median - a.median : b.sellers_total - a.sellers_total || a.median - b.median), [market, sort]);
  const toggle = (id: string) => setOpen(prev => { const next = new Set(prev); if (next.has(id)) next.delete(id); else next.add(id); return next; });
  return <section className="cx-section" aria-labelledby="compute-board-heading">
    <div className="cx-section-head">
      <div>
        <h2 id="compute-board-heading">Price by model</h2>
        <p className="cx-sub">Same model, every seller. USD per 1M tokens, 15:1 input:output, log scale shared by all rows.</p>
      </div>
      <div className="cx-sort" role="group" aria-label="Sort models">{sorts.map(([key, label]) => <button key={key} type="button" aria-pressed={sort === key} onClick={() => setSort(key)}>{label}</button>)}</div>
    </div>
    <div className="cx-legend" aria-hidden="true">
      <span><i className="lg-whisker" />cheapest to dearest</span><span><i className="lg-box" />middle half</span><span><i className="lg-median" />median</span><span><i className="lg-dot" />one seller</span><span><i className="lg-dot is-down" />seller down</span>
    </div>
    <div className={`board${market.models.some(m => m.change_24h_pct !== null) ? "" : " no-chg"}`}>
      <div className="board-cols" aria-hidden="true">
        <span className="c-model">Model</span><span className="c-sellers">Up</span><span className="c-strip"><Axis scale={scale} /></span><span className="c-median">Median</span><span className="c-spread">Spread</span><span className="c-cheap">Cheapest</span><span className="c-chg">24 h</span><span className="c-chev" />
      </div>
      <ul className="board-list">{models.map(m => <ModelRow key={m.id} model={m} scale={scale} open={open.has(m.id)} onToggle={() => toggle(m.id)} />)}</ul>
    </div>
  </section>;
}

function Providers({ market }: { market: ComputeMarket }) {
  const [all, setAll] = useState(false);
  const rows = all ? market.providers : market.providers.slice(0, PROVIDERS_SHOWN);
  const most = Math.max(1, ...market.providers.map(p => p.models));
  return <section className="cx-section" aria-labelledby="compute-providers-heading">
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

function Moves({ market }: { market: ComputeMarket }) {
  const names = new Map(market.models.map(m => [m.id, m.name]));
  const changes = [...market.changes].sort((a, b) => b.at_ms - a.at_ms).slice(0, 12);
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
      })}</ol> : <p className="note">No seller has changed a price since {when(market.window.from_ms)}.</p>}
    </div>
    <div className="cx-col">
      <h3>Dynamic pricers</h3>
      <p className="cx-sub">Sellers with more than one price in the last 24 h, and their range.</p>
      {market.dynamic.length ? <ol className="move-list dyn-list">{[...market.dynamic].sort((a, b) => b.changes_24h - a.changes_24h).map(d => <li key={`${d.model}-${d.provider}-${d.quantization}`}>
        <span className="mv-what"><b>{d.provider}</b><span>{names.get(d.model) ?? d.model} · {d.quantization}</span></span>
        <span className="mv-price">{usd(d.min)}–{usd(d.max)}</span>
        <span className="dyn-n"><b>{d.changes_24h}</b> changes</span>
      </li>)}</ol> : <p className="note">No seller repriced more than once in 24 h.</p>}
    </div>
  </section>;
}

function Perp({ latest, history, now }: { latest: ComputeLatest; history: ComputeHistory | null; now: number }) {
  const stale = latestStale(latest, now);
  const trade = site.computeTrade;
  return <article className="cx-panel" aria-labelledby="compute-perp-heading">
    <p className="cx-k">Perp settlement price · Solana devnet, test tokens</p>
    <h2 id="compute-perp-heading">Qwen3.8-27B, full precision</h2>
    <div className="perp-price"><strong>{usd4(latest.mark_usd)}</strong><span>per 1M tokens</span></div>
    {history && history.points.length >= 2 && history.to_ms - history.from_ms >= 86_400_000 && <figure className="perp-hist">
      <Sparkline step min={2} series={history.points} label={`Settlement price over 7 days: low ${usd4(history.low)}, high ${usd4(history.high)}.`} className="spark spark-sm" />
      <figcaption>7 d · low {usd4(history.low)} · high {usd4(history.high)}</figcaption>
    </figure>}
    <p className="perp-why">Narrower than the index: it uses only the full-precision (bf16/fp16) sellers of one benchmark model, so the contract settles on a like-for-like product.</p>
    <ul className="perp-sellers" aria-label="Sellers in the settlement price">{latest.pricing_set.map(r => <li key={`${r.provider}-${r.quantization}`}>
      <span>{r.provider} <span className="q">{r.quantization}</span></span><b>{usd4(r.priced)}</b><Up up={r.up} />
    </li>)}</ul>
    <p className="cx-updated">Pushed {ago(latest.landed_ms, now)} ({when(latest.landed_ms)}){latest.sig && <> · <a href={txUrl(latest.sig)} target="_blank" rel="noreferrer">tx {short(latest.sig)}</a></>}{stale && <> · <Stale why={stale} /></>}</p>
    <div className="panel-actions">
      {trade ? <a className="button-link" href={trade}>Trade the perp <Arrow /></a>
        : latest.market && <a className="button-link outline" href={addressUrl(latest.market)} target="_blank" rel="noreferrer">Market account <Arrow /></a>}
    </div>
  </article>;
}

function Flop({ latest }: { latest: ComputeLatest }) {
  const flop = latest.flop_side;
  const openrouter = latest.source === "openrouter";
  return <article className="cx-panel" aria-labelledby="compute-flop-heading">
    <p className="cx-k">FLOP Network · on-chain</p>
    <h2 id="compute-flop-heading">FLOP price per unit of compute</h2>
    <div className="perp-price"><strong>{flop.flop_per_gn == null ? "—" : flop.flop_per_gn.toPrecision(3)}</strong><span>FLOP per G_n</span></div>
    <dl className="flop-facts">{([
      ["Buyers", flop.buyers ?? "—"],
      ["Miners", flop.miners ?? "—"],
      ["Settled jobs", flop.prints ?? "—"],
      ["From FLOP's gateway", flop.gateway_share == null ? "—" : `${Math.round(flop.gateway_share * 100)}%`],
    ] as const).map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
    {flop.stale && <p className="note">Not computed right now: {flop.stale}.</p>}
    <p className="perp-why">{openrouter ? "Today this is FLOP's listed rate, not a traded price. It does not set the settlement price." : "This sets the settlement price right now, converted to USD at a fixed stand-in rate."}</p>
  </article>;
}

function Method({ market, latest }: { market: ComputeMarket | null; latest: ComputeLatest | null }) {
  const openrouter = latest?.source === "openrouter";
  const rows: [string, ReactNode][] = [
    ["Quote", "One seller, one model, one precision, from OpenRouter's public endpoint list."],
    ["Price", <>(15 × input + output) ÷ 16, in USD per 1M tokens.</>],
    ["Up", "OpenRouter reports the endpoint with status 0. 24 h uptime is the share of reads it was up."],
    ["Model price", "Median of the quotes that are up. The box spans the 25th to 75th percentile."],
    ["Index", market ? <>Combines the {count(market.index.n_models)} model medians, indexed from {day(market.index.base_ms)}.{market.models.length > market.index.n_models && " Models marked thin are listed but left out."}</> : "Combines the model medians, indexed from its first read."],
    ["Stale", "No new read for 30 minutes. The page shows a stale badge next to the time."],
    ["Settlement price", openrouter
      ? <>bf16 sellers first; fewer than 2 up, fp16, then both. Weighted 1 ÷ price². With 3 or more, a quote under half the median counts as half. Clamp now: {latest?.clamp ?? "unknown"}.</>
      : <>FLOP's on-chain price ({latest?.source ?? "unknown"}), converted to USD at a fixed stand-in rate.</>],
    ["Check it", <>Each settlement price is a signed log line; <code>staging verify-push-log</code> recomputes it from the logged sellers.</>],
  ];
  return <section className="cx-section" aria-labelledby="compute-how-heading">
    <h2 id="compute-how-heading">Method</h2>
    <dl className="method">{rows.map(([k, v], i) => <div key={k}><dt><span>{String(i + 1).padStart(2, "0")}</span>{k}</dt><dd>{v}</dd></div>)}</dl>
    <p className="cx-fine">Independent of Flop Labs and OpenRouter.{site.computeRepo && <> Source code: <a href={site.computeRepo}>{site.computeRepo}</a>.</>}</p>
  </section>;
}

export default function Compute({ feed }: { feed: ComputeFeedClient | null }) {
  const { latest, history, market, error, failed, loaded, now } = useComputeFeed(feed);
  if (!feed) return <div className="cx"><h1>Compute index</h1><p className="note">Price feed not configured.</p></div>;
  const nothing = !latest && !market;
  return <div className="cx">
    {!loaded && nothing ? <><h1 className="sr-only">Compute index</h1><Skeleton /></>
      : market ? <Hero market={market} now={now} error={failed.market ? error : ""} />
      : <header className="cx-hero is-empty"><div className="cx-hero-main"><h1>Compute index</h1><p role="status" className="error">Market feed unreachable ({failed.market ? error : "no data"}). Try again in a minute.</p></div></header>}
    {market && <Board market={market} />}
    {market && <div className="cx-pair"><Providers market={market} /><Moves market={market} /></div>}
    {(latest || failed.latest) && <div className="cx-panels">
      {latest ? <><Perp latest={latest} history={history} now={now} /><Flop latest={latest} /></>
        : <article className="cx-panel"><h2>Perp settlement price</h2><p role="status" className="error">Price feed unreachable ({error}). Try again in a minute.</p></article>}
    </div>}
    {(market || latest) && <Method market={market} latest={latest} />}
  </div>;
}
