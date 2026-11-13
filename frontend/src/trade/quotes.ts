// Pure maths for the trade screen: the liquidity account's two sides, the seller ladder and the mark's 24 h move.
import type { ComputeMarket, MarketModel, MarketQuote, Point } from "../compute/client";
import type { Side } from "../market/types";

/** Bid and ask around the mark. The matcher fills at mark ± spread (its spread is 0, so both equal the mark). */
export function quoteSides(mark: number, spreadBps: number) {
  if (!Number.isFinite(mark) || mark <= 0 || !Number.isFinite(spreadBps) || spreadBps < 0) return null;
  return { bid: mark * (1 - spreadBps / 10_000), ask: mark * (1 + spreadBps / 10_000) };
}

/** What an order pays per unit: the ask for a long, the bid for a short. */
export function entryPrice(side: Side, mark: number, spreadBps: number) {
  const q = quoteSides(mark, spreadBps);
  return q ? (side === "long" ? q.ask : q.bid) : NaN;
}

export type LadderRow =
  | { kind: "quote"; provider: string; quantization: string; price: number; up: boolean }
  | { kind: "mark" | "index"; price: number };

/**
 * The benchmark model's seller quotes, dearest first like an order book, with the mark and the all-seller index
 * placed where they fall. Quotes with no price are left out; a marker without a value is left out.
 */
export function sellerLadder(model: Pick<MarketModel, "quotes">, mark: number | null, index: number | null): LadderRow[] {
  const rows: LadderRow[] = model.quotes
    .filter((q): q is typeof q & { price: number } => typeof q.price === "number" && Number.isFinite(q.price))
    .map(q => ({ kind: "quote" as const, provider: q.provider, quantization: q.quantization, price: q.price, up: q.up }));
  if (mark !== null && Number.isFinite(mark)) rows.push({ kind: "mark", price: mark });
  if (index !== null && Number.isFinite(index)) rows.push({ kind: "index", price: index });
  // Dearest first; at a tie a marker sits above the quotes at that price, the mark above the index.
  const rank = (r: LadderRow) => r.kind === "mark" ? 0 : r.kind === "index" ? 1 : 2;
  return rows.sort((a, b) => b.price - a.price || rank(a) - rank(b));
}

/** The value of a step series at time t (the last point at or before t), or null before the first point. */
export function valueAt(points: Point[], t: number) {
  let value: number | null = null;
  for (const [time, v] of points) {
    if (time > t) break;
    value = v;
  }
  return value;
}

/** Percent change of a step series over the last 24 h; null without a point 24 h back. */
export function change24h(points: Point[], now: number) {
  if (points.length < 2) return null;
  const then = valueAt(points, now - 86_400_000);
  const last = points[points.length - 1][1];
  return then === null || then <= 0 ? null : (last / then - 1) * 100;
}

export const RANGES = [
  { label: "24 h", ms: 86_400_000 },
  { label: "7 d", ms: 7 * 86_400_000 },
] as const;

/** The ranges the data can fill: a range shows once the series spans at least 90% of it. */
export function availableRanges(points: Point[]) {
  if (points.length < 2) return [];
  const span = points[points.length - 1][0] - points[0][0];
  return RANGES.filter(r => span >= r.ms * 0.9);
}

const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/** Finite [time, value] pairs from untrusted JSON, oldest first. Anything else is dropped. */
export function cleanPoints(points: unknown): Point[] {
  if (!Array.isArray(points)) return [];
  return points.filter((p): p is Point => Array.isArray(p) && finite(p[0]) && finite(p[1])).sort((a, b) => a[0] - b[0]);
}

/**
 * The benchmark model from market.json, checked field by field before the page reads it: quotes and series that
 * are not arrays become empty, malformed rows are dropped, a non-finite median becomes null. Null without the model.
 */
export function benchmarkModel(market: ComputeMarket | null, id: string) {
  const models: unknown = market?.models;
  if (!Array.isArray(models)) return null;
  const m = models.find((x): x is MarketModel => !!x && typeof x === "object" && (x as MarketModel).id === id);
  if (!m) return null;
  const quotes: MarketQuote[] = (Array.isArray(m.quotes) ? m.quotes : []).filter((q: unknown): q is MarketQuote => {
    const x = q as MarketQuote;
    return !!x && typeof x === "object" && typeof x.provider === "string" && typeof x.quantization === "string"
      && (x.price === null || finite(x.price)) && typeof x.up === "boolean";
  });
  const r = m.routed && typeof m.routed === "object" ? m.routed : null;
  const routed = r && finite(r.price) && r.price > 0
    ? { price: r.price, change_1h_pct: finite(r.change_1h_pct) ? r.change_1h_pct : null, series: cleanPoints(r.series) } : null;
  return { ...m, median: finite(m.median) ? m.median : null, quotes, series: cleanPoints(m.series), routed };
}
