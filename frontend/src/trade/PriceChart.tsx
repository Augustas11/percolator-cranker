// The mark (settlement price pushed on chain) as a step line, with the all-seller index as a second line.
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent } from "react";
import type { Point } from "../compute/client";
import { perpPrice } from "../market/types";
import { cleanPoints, valueAt } from "./quotes";

const PAD = { top: 30, right: 72, bottom: 28, left: 12 };

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

export default function PriceChart({ mark, index, from, to }: { mark: Point[]; index: Point[] | null; from: number; to: number }) {
  const [ref, { width, height: H }] = useSize();
  const gradient = useId();
  const [hover, setHover] = useState<number | null>(null);
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
  const values = [...markIn, ...indexIn].map(p => p[1]);
  if (markIn.length === 0 || values.length === 0) return <div ref={ref} className="tx-chart-empty" role="status">No mark pushed in this range.</div>;
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
  const hoverMark = hoverT === null ? null : valueAt(markIn, hoverT);
  const hoverIndex = hoverT === null || indexIn.length === 0 || hoverT < indexIn[0][0] ? null : valueAt(indexIn, hoverT);

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

  return <div ref={ref} className="tx-chart-frame">
    <div className="tx-readout" aria-live="polite">
      {hoverT !== null
        ? <><span>{fmtFull(hoverT)}</span><span><i className="k-mark" />Mark <b>{perpPrice(hoverMark)}</b></span>{hoverIndex !== null && <span><i className="k-index" />Index <b>{perpPrice(hoverIndex)}</b></span>}</>
        : <><span><i className="k-mark" />Mark</span>{indexIn.length > 0 && <span><i className="k-index" />Index (all sellers)</span>}</>}
    </div>
    <svg className="tx-chart" width={width} height={H} viewBox={`0 0 ${width} ${H}`} role="img" tabIndex={0}
      aria-label={`Mark price from ${fmtFull(from)} to ${fmtFull(to)}: last ${perpPrice(last)}${indexIn.length ? `, all-seller index last ${perpPrice(indexIn[indexIn.length - 1][1])}` : ""}. Arrow keys move the readout.`}
      onPointerMove={move} onPointerLeave={() => setHover(null)} onKeyDown={key} onBlur={() => setHover(null)}>
      <defs><linearGradient id={gradient} x1="0" x2="0" y1="0" y2="1"><stop offset="0" stopColor="var(--site-accent)" stopOpacity="0.22" /><stop offset="1" stopColor="var(--site-accent)" stopOpacity="0" /></linearGradient></defs>
      {niceTicks(lo + pad, hi - pad).map(v => <g key={v} className="tx-grid">
        <line x1={PAD.left} x2={PAD.left + iw} y1={y(v)} y2={y(v)} />
        <text x={PAD.left + iw + 8} y={y(v) + 4}>{v.toFixed(4)}</text>
      </g>)}
      {xTicks.map(t => <text key={t} className="tx-xlabel" x={x(t)} y={H - 8} textAnchor="middle">{fmtTime(t, span)}</text>)}
      <path d={`${markPath}V${PAD.top + ih}H${x(markIn[0][0]).toFixed(1)}Z`} fill={`url(#${gradient})`} />
      {indexIn.length > 1 && <path className="tx-index-line" d={line(indexIn)} />}
      <path className="tx-mark-line" d={markPath} />
      <g className="tx-last"><line x1={PAD.left} x2={PAD.left + iw} y1={y(last)} y2={y(last)} /><rect x={PAD.left + iw + 2} y={y(last) - 10} width={PAD.right - 4} height={20} rx={4} /><text x={PAD.left + iw + 8} y={y(last) + 4}>{last.toFixed(4)}</text></g>
      {hoverT !== null && <g className="tx-cross">
        <line x1={x(hoverT)} x2={x(hoverT)} y1={PAD.top} y2={PAD.top + ih} />
        {hoverMark !== null && <><line x1={PAD.left} x2={PAD.left + iw} y1={y(hoverMark)} y2={y(hoverMark)} /><circle cx={x(hoverT)} cy={y(hoverMark)} r={4} /></>}
        {hoverIndex !== null && <circle className="ix" cx={x(hoverT)} cy={y(hoverIndex)} r={3.5} />}
      </g>}
    </svg>
  </div>;
}
