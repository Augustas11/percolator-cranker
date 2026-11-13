// The mark (settlement price pushed on chain) as a step line, with the routed price and the all-seller index as
// optional lines. The legend toggles each line; the y range follows the lines that are on.
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent } from "react";
import type { Point } from "../compute/client";
import { perpPrice } from "../market/types";
import { cleanPoints, valueAt } from "./quotes";

const PAD = { top: 30, right: 72, bottom: 28, left: 12 };

export type LineKey = "mark" | "routed" | "index";
const LABEL: Record<LineKey, string> = { mark: "Mark", routed: "Paid now (all sellers)", index: "Index (all sellers)" };
/** Default on: the mark and what requests pay; the list-price index is one click away. */
const DEFAULT_ON: Record<LineKey, boolean> = { mark: true, routed: true, index: false };

const fmtTime = (t: number, span: number) => new Date(t).toLocaleString("en-GB", span > 2 * 86_400_000
  ? { timeZone: "UTC", day: "numeric", month: "short" }
  : { timeZone: "UTC", hour: "2-digit", minute: "2-digit" });
const fmtFull = (t: number) => new Date(t).toLocaleString("en-GB", { timeZone: "UTC", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }) + " UTC";

function niceTicks(lo: number, hi: number, n = 4) {
  const step = (hi - lo) / n;
  return Array.from({ length: n + 1 }, (_, i) => lo + step * i);
}

/** The frame's size; the chart fills it (300 px tall at least). jsdom has no layout, so it keeps 800 × 360. */
function useSize() {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 800, height: 360 });
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      if (width > 0) setSize({ width, height: Math.max(300, Math.floor(height)) });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, size] as const;
}

export default function PriceChart({ mark, index, routed = null, from, to }: { mark: Point[]; index: Point[] | null; routed?: Point[] | null; from: number; to: number }) {
  const [ref, { width, height: H }] = useSize();
  const gradient = useId();
  const [hover, setHover] = useState<number | null>(null);
  const [on, setOn] = useState(DEFAULT_ON);
  const valid = Number.isFinite(from) && Number.isFinite(to) && to > from;
  const markIn = useMemo(() => {
    if (!valid) return [];
    // Carry the last value before the window in, so the step line starts at the left edge.
    const clean = cleanPoints(mark);
    const before = valueAt(clean, from);
    const inside = clean.filter(([t]) => t >= from && t <= to);
    return before !== null ? [[from, before] as Point, ...inside] : inside;
  }, [mark, from, to, valid]);
  const indexIn = useMemo(() => valid && index ? cleanPoints(index).filter(([t]) => t >= from && t <= to) : [], [index, from, to, valid]);
  const routedIn = useMemo(() => valid && routed ? cleanPoints(routed).filter(([t]) => t >= from && t <= to) : [], [routed, from, to, valid]);
  const has: Record<LineKey, boolean> = { mark: markIn.length > 0, routed: routedIn.length > 1, index: indexIn.length > 1 };
  const shown = (k: LineKey) => has[k] && on[k];
  const values = [...(shown("mark") ? markIn : []), ...(shown("routed") ? routedIn : []), ...(shown("index") ? indexIn : [])].map(p => p[1]);
  const legend = <div className="tx-legend" role="group" aria-label="Chart lines">
    {(["mark", "routed", "index"] as LineKey[]).filter(k => has[k]).map(k =>
      <button key={k} type="button" className={`k-${k}`} aria-pressed={on[k]} onClick={() => setOn(o => ({ ...o, [k]: !o[k] }))}><i aria-hidden="true" />{LABEL[k]}</button>)}
  </div>;
  if (markIn.length === 0) return <div ref={ref} className="tx-chart-empty" role="status">No mark pushed in this range.</div>;
  if (values.length === 0) return <div ref={ref} className="tx-chart-frame">{legend}<p className="tx-chart-empty" role="status">Every line is off. Turn one on in the legend.</p></div>;
  let lo = Math.min(...values), hi = Math.max(...values);
  const pad = (hi - lo) * 0.12 || hi * 0.02 || 0.01;
  lo -= pad; hi += pad;
  const iw = width - PAD.left - PAD.right, ih = H - PAD.top - PAD.bottom;
  const x = (t: number) => PAD.left + (t - from) / (to - from) * iw;
  const y = (v: number) => PAD.top + (1 - (v - lo) / (hi - lo)) * ih;
  const step = (pts: Point[], end: number) => pts.map(([t, v], i) => `${i ? "H" : "M"}${x(t).toFixed(1)}${i ? `V${y(v).toFixed(1)}` : `,${y(v).toFixed(1)}`}`).join("") + `H${x(end).toFixed(1)}`;
  const line = (pts: Point[]) => pts.map(([t, v], i) => `${i ? "L" : "M"}${x(t).toFixed(1)},${y(v).toFixed(1)}`).join("");
  const markPath = step(markIn, to);
  const last = markIn[markIn.length - 1][1];
  const span = to - from;
  const xTicks = Array.from({ length: 5 }, (_, i) => from + span * (i + 0.5) / 5);
  const hoverT = hover === null ? null : from + hover * span;
  const at = (pts: Point[], k: LineKey) => hoverT === null || !shown(k) || hoverT < pts[0][0] ? null : valueAt(pts, hoverT);
  const hoverMark = shown("mark") ? at(markIn, "mark") : null;
  const hoverRouted = has.routed ? at(routedIn, "routed") : null;
  const hoverIndex = has.index ? at(indexIn, "index") : null;

  const move = (e: PointerEvent<SVGSVGElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    const f = ((e.clientX - box.left) / box.width * width - PAD.left) / iw;
    setHover(Math.min(1, Math.max(0, f)));
  };
  const key = (e: KeyboardEvent<SVGSVGElement>) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    setHover(h => Math.min(1, Math.max(0, (h ?? 1) + (e.key === "ArrowLeft" ? -0.02 : 0.02))));
  };
  const lastOf = (pts: Point[]) => perpPrice(pts[pts.length - 1][1]);

  return <div ref={ref} className="tx-chart-frame">
    {legend}
    <div className="tx-readout" aria-live="polite">
      {hoverT !== null && <><span>{fmtFull(hoverT)}</span>
        {hoverMark !== null && <span><i className="k-mark" />Mark <b>{perpPrice(hoverMark)}</b></span>}
        {hoverRouted !== null && <span><i className="k-routed" />Paid now <b>{perpPrice(hoverRouted)}</b></span>}
        {hoverIndex !== null && <span><i className="k-index" />Index <b>{perpPrice(hoverIndex)}</b></span>}</>}
    </div>
    <svg className="tx-chart" width={width} height={H} viewBox={`0 0 ${width} ${H}`} role="img" tabIndex={0}
      aria-label={`Mark price from ${fmtFull(from)} to ${fmtFull(to)}: last ${perpPrice(last)}${shown("routed") ? `, paid now (all sellers) last ${lastOf(routedIn)}` : ""}${shown("index") ? `, all-seller index last ${lastOf(indexIn)}` : ""}. Arrow keys move the readout.`}
      onPointerMove={move} onPointerLeave={() => setHover(null)} onKeyDown={key} onBlur={() => setHover(null)}>
      <defs><linearGradient id={gradient} x1="0" x2="0" y1="0" y2="1"><stop offset="0" stopColor="var(--site-accent)" stopOpacity="0.22" /><stop offset="1" stopColor="var(--site-accent)" stopOpacity="0" /></linearGradient></defs>
      {niceTicks(lo + pad, hi - pad).map(v => <g key={v} className="tx-grid">
        <line x1={PAD.left} x2={PAD.left + iw} y1={y(v)} y2={y(v)} />
        <text x={PAD.left + iw + 8} y={y(v) + 4}>{v.toFixed(4)}</text>
      </g>)}
      {xTicks.map(t => <text key={t} className="tx-xlabel" x={x(t)} y={H - 8} textAnchor="middle">{fmtTime(t, span)}</text>)}
      {shown("mark") && <path d={`${markPath}V${PAD.top + ih}H${x(markIn[0][0]).toFixed(1)}Z`} fill={`url(#${gradient})`} />}
      {shown("index") && <path className="tx-index-line" d={line(indexIn)} />}
      {shown("routed") && <path className="tx-routed-line" d={line(routedIn)} />}
      {shown("mark") && <path className="tx-mark-line" d={markPath} />}
      {shown("mark") && <g className="tx-last"><line x1={PAD.left} x2={PAD.left + iw} y1={y(last)} y2={y(last)} /><rect x={PAD.left + iw + 2} y={y(last) - 10} width={PAD.right - 4} height={20} rx={4} /><text x={PAD.left + iw + 8} y={y(last) + 4}>{last.toFixed(4)}</text></g>}
      {hoverT !== null && <g className="tx-cross">
        <line x1={x(hoverT)} x2={x(hoverT)} y1={PAD.top} y2={PAD.top + ih} />
        {hoverMark !== null && <><line x1={PAD.left} x2={PAD.left + iw} y1={y(hoverMark)} y2={y(hoverMark)} /><circle cx={x(hoverT)} cy={y(hoverMark)} r={4} /></>}
        {hoverRouted !== null && <circle className="rt" cx={x(hoverT)} cy={y(hoverRouted)} r={3.5} />}
        {hoverIndex !== null && <circle className="ix" cx={x(hoverT)} cy={y(hoverIndex)} r={3.5} />}
      </g>}
    </svg>
  </div>;
}
