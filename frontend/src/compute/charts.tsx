import type { ComputeMarket, MarketModel, MixKey, Point } from "./client";
import { quantile } from "./fixture";
import { day, times, usd } from "./format";
import { PERP } from "../market/types";

/** Below this many points a time series is too short to draw as a line. */
export const MIN_SERIES_POINTS = 12;

const STEPS = [1, 2, 5];
function niceDown(v: number) {
  const e = Math.floor(Math.log10(v));
  for (const s of [...STEPS].reverse()) if (s * 10 ** e <= v) return s * 10 ** e;
  return 10 ** e;
}
function niceUp(v: number) {
  const e = Math.floor(Math.log10(v));
  for (const s of [...STEPS, 10]) if (s * 10 ** e >= v) return s * 10 ** e;
  return 10 ** (e + 1);
}

export interface LogScale { lo: number; hi: number; ticks: number[]; at: (v: number) => number }

/** A shared log axis from the cheapest to the dearest quote, on 1-2-5 ticks. `at` gives 0..100 (%). */
export function logScale(min: number, max: number): LogScale {
  // No usable bounds (no models, or a bad price): a fixed $0.01 to $10 axis rather than an endless tick loop.
  if (!Number.isFinite(min) || !Number.isFinite(max) || max <= 0) { min = 0.01; max = 10; }
  const lo = niceDown(Math.max(min, 1e-6)), hi = niceUp(Math.max(max, lo * 2));
  const span = Math.log(hi / lo);
  const ticks: number[] = [];
  for (let e = Math.floor(Math.log10(lo)); 10 ** e <= hi; e++) for (const s of STEPS) {
    const t = Number((s * 10 ** e).toPrecision(3));
    if (t >= lo && t <= hi) ticks.push(t);
  }
  return { lo, hi, ticks: ticks.length > 8 ? ticks.filter(t => /^1|^5/.test(String(t).replace(/^0\.0*/, ""))) : ticks, at: v => Math.min(100, Math.max(0, Math.log(Math.max(v, lo) / lo) / span * 100)) };
}
export const tickLabel = (t: number) => "$" + (t >= 1 ? t.toFixed(0) : String(Number(t.toPrecision(2))).replace(/^0/, "0"));

export function marketScale(market: ComputeMarket) {
  const priced = market.models.filter(m => Number.isFinite(m.min) && Number.isFinite(m.max) && m.max > 0);
  if (!priced.length) return logScale(NaN, NaN);
  return logScale(Math.min(...priced.map(m => m.min)), Math.max(...priced.map(m => m.max)));
}

export function stripLabel(m: MarketModel) {
  return `${m.name}: ${m.sellers_up} of ${m.sellers_total} sellers up. Cheapest ${usd(m.min)}, dearest ${usd(m.max)} (${times(m.min, m.max)}). Middle half ${usd(m.p25)} to ${usd(m.p75)}, median ${usd(m.median)}.`;
}

/** min-p25-median-p75-max of one model on the shared scale, with each live quote as a tick. */
export function RangeStrip({ model, scale, quotes = true, stale = false }: { model: MarketModel; scale: LogScale; quotes?: boolean; stale?: boolean }) {
  const at = scale.at;
  return <div className={`strip${model.thin ? " strip-thin" : ""}${stale ? " strip-stale" : ""}`} role="img" aria-label={stale ? `${stripLabel(model)} Not read in the last 30 minutes.` : stripLabel(model)}>
    {scale.ticks.map(t => <span key={t} className="strip-grid" style={{ left: `${at(t)}%` }} />)}
    <span className="strip-whisker" style={{ left: `${at(model.min)}%`, width: `${at(model.max) - at(model.min)}%` }} />
    <span className="strip-box" style={{ left: `${at(model.p25)}%`, width: `${Math.max(0.6, at(model.p75) - at(model.p25))}%` }} />
    {quotes && model.quotes.map((q, i) => q.price === null ? null
      : <span key={i} className={`strip-dot${stale ? " is-stale" : q.up ? "" : " is-down"}`} style={{ left: `${at(q.price)}%` }} />)}
    <span className="strip-median" style={{ left: `${at(model.median)}%` }} />
  </div>;
}

export function Axis({ scale }: { scale: LogScale }) {
  return <div className="strip-axis" aria-hidden="true">
    {scale.ticks.map(t => <span key={t} style={{ left: `${scale.at(t)}%` }}>{tickLabel(t)}</span>)}
  </div>;
}

/** The model with the widest dearest/cheapest ratio, skipping thin ones. */
export function widest(market: ComputeMarket) {
  const pool = market.models.filter(m => !m.thin && m.min > 0);
  return (pool.length ? pool : market.models).reduce<MarketModel | null>((best, m) => !best || m.max / m.min > best.max / best.min ? m : best, null);
}

/** The models whose dearest seller is furthest above the cheapest, widest first; thin and stale models are skipped. */
export function widestModels(market: ComputeMarket, n = 6) {
  return market.models.filter(m => !m.thin && !m.stale && m.min > 0 && m.max > m.min).sort((a, b) => b.max / b.min - a.max / a.min).slice(0, n);
}

/** Each live quote divided by its model's median: how far sellers sit from the going rate. */
export function spreadRatios(market: ComputeMarket) {
  const ratios: number[] = [];
  for (const m of market.models) if (m.median > 0 && !m.stale) for (const q of m.quotes) if (q.up && q.price !== null && q.price > 0 && Number.isFinite(q.price / m.median)) ratios.push(q.price / m.median);
  return ratios.sort((a, b) => a - b);
}

const fmtRatio = (r: number) => (r >= 10 ? r.toFixed(0) : r >= 1 ? r.toFixed(1) : r.toFixed(2)) + "\u00d7";

/** Beeswarm of every live quote as a multiple of its model's median, on a log axis. Fixed height; every quote is drawn. */
export function SpreadSwarm({ market }: { market: ComputeMarket }) {
  const ratios = spreadRatios(market);
  if (!ratios.length) return null;
  const W = 640, r = 2.6, col = r * 2 + 0.6, step = col;
  const lo = Math.min(0.25, niceDown(ratios[0])), hi = Math.max(4, niceUp(ratios[ratios.length - 1]));
  const x = (v: number) => 8 + Math.log(v / lo) / Math.log(hi / lo) * (W - 16);
  // A fixed plot: a column that is full passes its extra dots to the next free column on the dot's own side of the
  // median, so every quote is drawn and the pile at exactly 1× widens instead of running off the top.
  const H = 240, base = H - 26, room = Math.floor((base - 8) / step);
  const stacks = new Map<number, number>(), maxCol = Math.floor((W - 4) / col);
  const side = (v: number) => v < 0.995 ? -1 : v > 1.005 ? 1 : 0;
  const order = [...ratios].sort((p, q) => Math.abs(Math.log(p)) - Math.abs(Math.log(q)));
  let flip = 1;
  const dots = order.map(v => {
    const home = Math.round(x(v) / col), dir = side(v) || (flip = -flip);
    let c = home;
    // Past the plot's edge there is nowhere to spill: stack on the home column instead.
    while ((stacks.get(c) ?? 0) >= room) { c += dir; if (c < 1 || c > maxCol) { c = home; break; } }
    const n = stacks.get(c) ?? 0;
    stacks.set(c, n + 1);
    return { cx: c * col, n, v };
  });
  const q1 = quantile(ratios, 0.25), q3 = quantile(ratios, 0.75), top = ratios[ratios.length - 1], bottom = ratios[0];
  const marks = [0.25, 0.5, 1, 2, 4, 8, 16].filter(t => t >= lo && t <= hi);
  const label = `${ratios.length} live quotes, each divided by its model's median price. Half sit between ${fmtRatio(q1)} and ${fmtRatio(q3)} the median. Cheapest ${fmtRatio(bottom)}, dearest ${fmtRatio(top)}.`;
  return <figure className="swarm">
    <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="xMidYMax meet" role="img" aria-label={label}>
      <rect className="swarm-band" x={x(q1)} y={0} width={x(q3) - x(q1)} height={base + 4} />
      {marks.map(t => <g key={t}>
        <line className={t === 1 ? "swarm-one" : "swarm-tick"} x1={x(t)} x2={x(t)} y1={t === 1 ? 0 : base - 4} y2={base + 6} />
        <text className="swarm-label" x={x(t)} y={H - 4} textAnchor="middle">{t < 1 ? (t === 0.5 ? "\u00bd" : "\u00bc") : t}{"\u00d7"}</text>
      </g>)}
      {dots.map((d, i) => <circle key={i} cx={d.cx} cy={base - r - d.n * step} r={r} className={d.v < 0.995 ? "swarm-dot is-cheap" : d.v > 1.005 ? "swarm-dot is-dear" : "swarm-dot"} />)}
    </svg>
    <figcaption>
      <span><i className="key is-cheap" aria-hidden="true" />below median</span>
      <span><i className="key is-dear" aria-hidden="true" />above median</span>
    </figcaption>
  </figure>;
}

/** A plain line over a time series; returns null below MIN_SERIES_POINTS so a short history never draws as a dead line. */
export function Sparkline({ series, label, className = "spark", step = false, min = MIN_SERIES_POINTS }: { series: Point[]; label: string; className?: string; step?: boolean; min?: number }) {
  if (series.length < Math.max(2, min)) return null;
  const W = 320, H = 64, pad = 4;
  const t0 = series[0][0], t1 = series[series.length - 1][0];
  const vs = series.map(p => p[1]);
  let lo = Math.min(...vs), hi = Math.max(...vs);
  const band = Math.max(Math.abs(hi) * 0.01, 1e-6);
  if (hi - lo < band) { lo -= band; hi += band; }
  const x = (t: number) => (t - t0) / Math.max(1, t1 - t0) * W;
  const y = (v: number) => H - pad - (v - lo) / (hi - lo) * (H - 2 * pad);
  const d = series.map(([t, v], i) => i === 0 ? `M${x(t).toFixed(1)} ${y(v).toFixed(1)}` : step ? `H${x(t).toFixed(1)} V${y(v).toFixed(1)}` : `L${x(t).toFixed(1)} ${y(v).toFixed(1)}`).join(" ");
  return <svg className={className} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" role="img" aria-label={label}>
    <path d={`${d} V${H} H0 Z`} className="spark-fill" />
    <path d={d} className="spark-line" vectorEffect="non-scaling-stroke" />
  </svg>;
}

export const sinceLabel = (series: Point[]) => series.length ? `since ${day(series[0][0])}` : "";

/** Quotes a model's min and max come from: up and priced. */
export const upPriced = (m: MarketModel) => m.quotes.filter(q => q.up && q.price !== null);

/**
 * How dollars per billion G_n spread across models: the feed's median and middle half when it sends them
 * (else computed here), the cheapest and dearest model, and a log position for a value. Null without data.
 */
export function computeSpread(market: ComputeMarket) {
  const vals = market.models.map(m => m.usd_per_bn_gn).filter((v): v is number => typeof v === "number" && Number.isFinite(v) && v > 0).sort((a, b) => a - b);
  if (!vals.length) return null;
  const cp = market.compute_price;
  const num = (v: number | null | undefined, fallback: number) => typeof v === "number" && Number.isFinite(v) ? v : fallback;
  const lo = vals[0], hi = vals[vals.length - 1];
  const span = Math.log(hi / lo) || 1;
  return {
    n: num(cp?.n_models, vals.length), median: num(cp?.median_usd_per_bn_gn, quantile(vals, 0.5)),
    p25: num(cp?.p25, quantile(vals, 0.25)), p75: num(cp?.p75, quantile(vals, 0.75)), lo, hi,
    at: (v: number) => hi > lo ? Math.min(100, Math.max(0, Math.log(Math.max(v, lo) / lo) / span * 100)) : 50,
  };
}

export const DEFAULT_MIX: MixKey = "avg_15_1";

/** A model's median at a token mix: the 15:1 median, or the feed's per-mix median (null when the feed has none). */
export function medianAt(m: MarketModel, mix: MixKey = DEFAULT_MIX): number | null {
  const v = m.mix?.[mix];
  if (typeof v === "number" && Number.isFinite(v)) return v;
  return mix === DEFAULT_MIX ? m.median : null;
}

/** True when the feed carries medians for the other mixes. */
export const hasMixes = (market: ComputeMarket) => market.models.some(m => medianAt(m, "chat_3_1") !== null || medianAt(m, "agent_150_1") !== null);

/**
 * The going rate for an open model: median of the medians of models with 3 or more sellers (not thin),
 * at a token mix, with the cheapest and dearest model. Counts follow the median's own set: one price per
 * provider up now, plus providers held at their last price.
 */
export function typicalPrice(market: ComputeMarket, mix: MixKey = DEFAULT_MIX) {
  const pool = market.models.filter(m => !m.thin && (medianAt(m, mix) ?? 0) > 0);
  if (!pool.length) return null;
  const at = (m: MarketModel) => medianAt(m, mix)!;
  const byMedian = [...pool].sort((a, b) => at(a) - at(b));
  const top = market.mixes?.[mix];
  const sets = pool.map(m => new Set(upPriced(m).map(q => q.provider)));
  const held = pool.reduce((n, m) => n + (m.quotes_held ?? 0), 0);
  return {
    median: mix !== DEFAULT_MIX && typeof top === "number" && Number.isFinite(top) ? top : quantile(byMedian.map(at), 0.5),
    cheapest: byMedian[0], dearest: byMedian[byMedian.length - 1], models: pool.length, at,
    quotes: sets.reduce((n, s) => n + s.size, 0) + held, held, providers: new Set(sets.flatMap(s => [...s])).size,
  };
}

const num = (v: number | null | undefined): v is number => typeof v === "number" && Number.isFinite(v) && v > 0;

/** The benchmark model and what a request pays for it now: its routed price, 1 h change and series. Every part may be absent. */
export function benchmarkRouted(market: ComputeMarket) {
  const id = market.routed?.benchmark ?? PERP.benchmarkId;
  const model = market.models.find(m => m.id === id) ?? null;
  const routed = model?.routed ?? null;
  const price = num(routed?.price) ? routed!.price : num(market.routed?.price) ? market.routed!.price : null;
  return {
    id, model, name: model?.name ?? PERP.benchmark, routed, price,
    series: routed?.series ?? market.routed?.series ?? [],
    change1h: routed?.change_1h_pct ?? null,
    median: model && num(model.median) ? model.median : null,
  };
}
