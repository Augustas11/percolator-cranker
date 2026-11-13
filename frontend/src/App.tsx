import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { ReactNode } from "react";
import { PERP, perpPrice, perpSubtitle } from "./market/types";
import type { MarketClient, MarketSnapshot, Side } from "./market/types";
import { Link } from "./site/router";
import { site } from "./site/config";
import type { IndexerClient } from "./indexer/client";
import type { ComputeFeedClient } from "./compute/client";
import { useComputeFeed, latestStale, marketStale, modelStale } from "./compute/useFeed";
import { OTHER_MARKET, OwnFills, UNREACHABLE, fixed, number as num, useTradeIndexer } from "./Indexer";
import type { TradeIndexer } from "./Indexer";
import PriceChart from "./trade/PriceChart";
import { availableRanges, change24h, entryPrice, quoteSides, sellerLadder } from "./trade/quotes";
import "./App.css";
import "./site/compute.css";
import "./site/trade.css";

const usd = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const number = new Intl.NumberFormat("en-US", { maximumFractionDigits: 4 });
const percent = new Intl.NumberFormat("en-US", { style: "percent", maximumFractionDigits: 4 });
const dollars = (value: number) => Number.isFinite(value) ? usd.format(value) : "—";
const shortKey = (w: string) => w.length > 8 ? `${w.slice(0, 4)}…${w.slice(-4)}` : w;
const usdAtoms = (atoms: string) => { const n = fixed(atoms); return Number.isFinite(n) ? usd.format(n) : null; };
const ago = (ms: number, now: number) => {
  const s = Math.max(0, Math.floor((now - ms) / 1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`;
};
const countdown = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${String(Math.floor(s / 3600)).padStart(2, "0")}:${String(Math.floor(s % 3600 / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
};
const QUICK = [10, 25, 50];

export function useSnapshot(client: MarketClient) {
  const subscribe = useCallback((notify: () => void) => client.subscribe(notify), [client]);
  const getSnapshot = useCallback(() => client.getSnapshot(), [client]);
  return useSyncExternalStore(subscribe, getSnapshot);
}

function useNow(every: number, on: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const timer = setInterval(() => setNow(Date.now()), every);
    return () => clearInterval(timer);
  }, [every, on]);
  return now;
}

/** Accessible tabs: every panel stays in the DOM (hidden when not selected), so state and tests survive switching. */
function Tabs<K extends string>({ id, label, tabs, value, onChange, className = "" }: { id: string; label: string; tabs: [K, string][]; value: K; onChange: (k: K) => void; className?: string }) {
  return <div className={`tx-tabs ${className}`} role="tablist" aria-label={label}>
    {tabs.map(([k, text], i) => <button key={k} type="button" role="tab" id={`${id}-tab-${k}`} aria-controls={`${id}-panel-${k}`} aria-selected={value === k} tabIndex={value === k ? 0 : -1}
      onClick={() => onChange(k)}
      onKeyDown={e => {
        if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
        const next = tabs[(i + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length][0];
        onChange(next);
        document.getElementById(`${id}-tab-${next}`)?.focus();
      }}>{text}</button>)}
  </div>;
}
function Panel({ id, k, value, children, className = "" }: { id: string; k: string; value: string; children: ReactNode; className?: string }) {
  return <div role="tabpanel" id={`${id}-panel-${k}`} aria-labelledby={`${id}-tab-${k}`} hidden={value !== k} className={className}>{children}</div>;
}

type Stat = [label: string, value: ReactNode, cls?: string];

function Quotes({ snapshot, client, ladder, feedNote }: { snapshot: MarketSnapshot; client: MarketClient; ladder: ReturnType<typeof sellerLadder> | null; feedNote: string | null }) {
  const liq = snapshot.liquidity ?? null;
  const sides = liq ? quoteSides(snapshot.mark.price, liq.spreadBps) : null;
  const fee = snapshot.market.feeBps;
  // Centre the ladder on the mark, the way a book opens on the spread.
  const list = useRef<HTMLOListElement>(null);
  const hasLadder = ladder !== null;
  useEffect(() => {
    const ol = list.current, row = ol?.querySelector<HTMLElement>(".m-mark"), box = ol?.parentElement;
    if (row && box && box.clientHeight > 0) box.scrollTop = row.offsetTop - box.clientHeight / 2;
  }, [hasLadder]);
  return <div className="tx-quotes">
    <h3 className="tx-sub">Liquidity account</h3>
    {client.tradingReady === false ? <p className="tx-empty">No matcher is set for {PERP.name} in this build.</p>
      : !liq ? <p className="tx-empty" aria-busy="true">Reading the liquidity account.</p>
      : <>
        {liq.paused && <p className="tx-warn" role="status">Fills are paused by the liquidity account.</p>}
        <dl className="tx-sides">
          <div className="ask"><dt>Ask <small>long fills</small></dt><dd>{sides ? perpPrice(sides.ask) : "—"}</dd><dd className="lim">up to {num(liq.maxLong)} {PERP.unitShort}</dd></div>
          <div className="bid"><dt>Bid <small>short fills</small></dt><dd>{sides ? perpPrice(sides.bid) : "—"}</dd><dd className="lim">up to {num(liq.maxShort)} {PERP.unitShort}</dd></div>
        </dl>
        <p className="tx-fine">Spread {num(liq.spreadBps)} bps: the account fills at the mark{fee !== undefined ? <>, plus a {fee} bps fee</> : null}. Max {num(liq.maxFill)} {PERP.unitShort} per fill. Fills stop if the mark moves more than {num(liq.deviationBps / 100)}% from the account's reference price.</p>
      </>}
    <h3 className="tx-sub">Seller quotes <small>{PERP.benchmark}, USD per 1M tokens</small></h3>
    {feedNote && <p className="tx-warn" role="status">{feedNote}</p>}
    {ladder === null ? !feedNote && <p className="tx-empty" aria-busy="true">Loading seller quotes.</p>
      : <div className="tx-ladder-scroll"><ol ref={list} className="tx-ladder" aria-label={`${PERP.benchmark} seller quotes, dearest first, with the mark and the all-seller index`}>
        <li className="tx-ladder-head" aria-hidden="true"><span>Price</span><span>Seller</span><span>Prec.</span></li>
        {ladder.map((r, i) => r.kind === "quote"
          ? <li key={`${r.provider}-${r.quantization}-${i}`} className={`q ${snapshot.mark.price > 0 && r.price > snapshot.mark.price ? "above" : "below"}${r.up ? "" : " down"}`}>
            <span className="p">{perpPrice(r.price)}</span><span className="s">{r.provider}</span><span className="z">{r.quantization}{!r.up && <em> down</em>}</span>
          </li>
          : <li key={r.kind} className={`m m-${r.kind}`}><span className="p">{perpPrice(r.price)}</span><span className="s">{r.kind === "mark" ? "Mark" : "Index (all sellers)"}</span><span className="z" /></li>)}
      </ol></div>}
  </div>;
}

function Trades({ data, indexer, snapshot, now }: { data: TradeIndexer; indexer: IndexerClient | null; snapshot: MarketSnapshot; now: number }) {
  const rows = data.sameMarket !== false && data.recent && data.recent.length
    ? data.recent.map(f => ({ key: `${f.signature}:${f.ix_index}:${f.leg}`, ts: f.block_time === null ? null : f.block_time * 1000, side: f.side === "buy" ? "long" as const : "short" as const,
      size: Math.abs(fixed(f.size_q)), price: f.mark_price === null ? NaN : f.mark_price / 1e6, wallet: f.wallet ?? f.taker_portfolio, href: `https://explorer.solana.com/tx/${encodeURIComponent(f.signature)}?cluster=devnet` }))
    : snapshot.fills.map(f => ({ key: f.id, ts: f.ts, side: f.side, size: f.size, price: f.price, wallet: f.wallet ?? null, href: null as string | null }));
  return <>
    {!indexer ? null : data.sameMarket === false ? <p className="tx-warn" role="status">{OTHER_MARKET}</p> : data.statsError && !data.recent ? <p className="tx-warn" role="status">{UNREACHABLE}</p> : null}
    {rows.length === 0 ? <p className="tx-empty">No trades yet.</p>
      : <div className="table-scroll"><table className="tx-table tx-trades">
        <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col" className="num">Size</th><th scope="col" className="num">Price</th><th scope="col">Wallet</th></tr></thead>
        <tbody>{rows.map(r => <tr key={r.key}>
          <td>{r.ts === null ? "—" : r.href ? <a href={r.href} target="_blank" rel="noreferrer">{ago(r.ts, now)}</a> : ago(r.ts, now)}</td>
          <td className={r.side === "long" ? "up" : "down"}>{r.side}</td><td className="num">{num(r.size)}</td><td className="num">{perpPrice(r.price)}</td>
          <td>{r.wallet ? shortKey(r.wallet) : "—"}</td>
        </tr>)}</tbody>
      </table></div>}
  </>;
}

export default function App({ client, indexer, computeFeed = null, requestedSide, sideRequest }: { client: MarketClient; indexer?: IndexerClient | null; computeFeed?: ComputeFeedClient | null; requestedSide?: Side; sideRequest?: number }) {
  const snapshot = useSnapshot(client);
  const { market, mark, funding, portfolio, fills } = snapshot;
  const feed = useComputeFeed(computeFeed);
  const idx = useTradeIndexer(indexer ?? null, portfolio.owner);
  const fundingShown = client.fundingKnown === true && Number.isFinite(funding.rate);
  const now = useNow(1000, fundingShown);
  const [amount, setAmount] = useState("");
  const [size, setSize] = useState(String(PERP.defaultSize));
  const [leverage, setLeverage] = useState("1");
  const [side, setSide] = useState<Side>("long");
  useEffect(() => { if (requestedSide) setSide(requestedSide); }, [requestedSide, sideRequest]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [bookTab, setBookTab] = useState<"quotes" | "trades">("quotes");
  const [bottomTab, setBottomTab] = useState<"positions" | "fills" | "funding" | "account">("positions");
  const [mobileTab, setMobileTab] = useState<"trade" | "quotes" | "trades" | "positions">("trade");
  const [rangeLabel, setRangeLabel] = useState<string | null>(null);
  const leg = portfolio.legs[0];
  const health = portfolio.health;
  const liq = snapshot.liquidity ?? null;
  const maxFill = liq?.maxFill ?? PERP.maxFillUnits;
  const units = Number(size);
  const sized = size.trim() !== "" && Number.isFinite(units) && units > 0;
  const overFill = sized && units > PERP.maxFillUnits;
  const notional = sized && Number.isFinite(mark.price) ? units * mark.price : NaN;
  const lev = Number(leverage);
  const entry = liq ? entryPrice(side, mark.price, liq.spreadBps) : NaN;
  const fee = market.feeBps !== undefined && Number.isFinite(notional) ? notional * market.feeBps / 10_000 : NaN;
  const margin = Number.isFinite(notional) && lev >= 1 ? notional / lev : NaN;

  // The benchmark model in market.json: the all-seller index and the seller ladder.
  const bench = feed.market?.models.find(m => m.id === PERP.benchmarkId) ?? null;
  const index = bench && Number.isFinite(bench.median) ? bench.median : null;
  const feedNow = feed.now;
  const benchStale = bench ? modelStale(bench, feedNow) || !!marketStale(feed.market, feedNow) : false;
  const ladder = useMemo(() => bench ? sellerLadder(bench, Number.isFinite(mark.price) ? mark.price : null, index) : null, [bench, mark.price, index]);
  const feedNote = feed.failed.market && !feed.market ? "The compute feed is unreachable right now." : benchStale ? "These quotes are over 30 minutes old." : feed.loaded && feed.market && !bench ? `The feed has no quotes for ${PERP.benchmark} right now.` : null;

  // The chart: pushed marks from history.json, the live mark at the end, the index when it has 12 or more reads.
  const history = feed.history;
  const markSeries = useMemo(() => {
    const pts = history?.points ?? [];
    return Number.isFinite(mark.price) && pts.length ? [...pts, [Math.max(Date.now(), pts[pts.length - 1][0]), mark.price] as [number, number]] : pts;
  }, [history, mark.price]);
  const ranges = availableRanges(markSeries);
  const range = ranges.find(r => r.label === rangeLabel) ?? ranges[0] ?? null;
  const chartTo = markSeries.length ? markSeries[markSeries.length - 1][0] : 0;
  const chartFrom = range ? chartTo - range.ms : markSeries.length ? markSeries[0][0] : 0;
  const indexSeries = bench && bench.series.length >= 12 ? bench.series : null;
  const historyStale = latestStale(feed.latest, feedNow);

  // Market bar: each stat only when its source has a value.
  const change = history ? change24h(markSeries, chartTo) : null;
  const same = idx.sameMarket === true;
  const oi = same && idx.stats ? Object.values(idx.stats.open_interest)[0] : undefined;
  const oiUsd = same && idx.season ? usdAtoms(idx.season.open_interest_usd) : null;
  const vol = same && idx.season ? usdAtoms(idx.season.volume_usd) : null;
  const stats: Stat[] = [
    ...(Number.isFinite(mark.price) ? [["Mark", perpPrice(mark.price), "mark"] as Stat] : []),
    ...(index !== null ? [["Index (all sellers)", <>{perpPrice(index)}{benchStale && <em className="tag"> stale</em>}</>] as Stat] : []),
    ...(change !== null ? [["24 h change", `${change > 0 ? "+" : change < 0 ? "−" : ""}${Math.abs(change).toFixed(2)}%`, change > 0 ? "up" : change < 0 ? "down" : ""] as Stat] : []),
    ...(vol !== null ? [["Volume (all time)", vol] as Stat] : []),
    ...(oi ? [["Open interest", <>{oiUsd ?? ""}{oiUsd ? " · " : ""}{num(fixed(oi.long_q))} {PERP.unitShort}</>] as Stat] : []),
    ...(fundingShown ? [["Funding / next", <>{percent.format(funding.rate)} per hour · {countdown(funding.nextFundingTs - now)}</>] as Stat] : []),
  ];

  function open() {
    if (overFill) { setError(`The matcher fills up to ${PERP.maxFillUnits} units (${PERP.maxFillUnits} × ${PERP.unit}) per trade. Split a larger order into several trades.`); return; }
    void call(() => client.openPosition(side, units, Number(leverage)));
  }

  async function call(action: () => Promise<unknown>) {
    setPending(true);
    setError("");
    try { await action(); }
    catch (err) { setError(err instanceof Error ? err.message : "request failed"); }
    finally { setPending(false); }
  }

  async function claim() {
    if (!site.faucetApi || !portfolio.owner) return;
    const owner = portfolio.owner;
    await call(async () => {
      const res = await fetch(`${site.faucetApi}/claim`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ wallet: owner }) });
      const body = await res.json().catch(() => ({})) as { error?: string };
      if (!res.ok) throw new Error(body.error ?? `faucet: HTTP ${res.status}`);
    });
  }

  const connected = !!portfolio.owner;
  const tradeOff = client.tradingReady === false;
  const bottomTabs: ["positions" | "fills" | "funding" | "account", string][] = [["positions", "Positions"], ["fills", "Fills"], ...(fundingShown ? [["funding", "Funding"] as ["funding", string]] : []), ["account", "Account"]];

  return <main className="trade tx" data-mobile={mobileTab}>
    <header className="tx-bar">
      <div className="tx-name">
        <h1>{market.symbol}</h1>
        <p className="trade-sub">{perpSubtitle}</p>
      </div>
      <h2 id="market-heading" className="sr-only">Market</h2>
      <dl className="tx-stats" aria-labelledby="market-heading">
        {stats.map(([k, v, cls]) => <div key={k} className={cls}><dt>{k}</dt><dd>{v}</dd></div>)}
        {stats.length === 0 && <div className="tx-stats-loading" aria-busy="true"><span className="sk" /><span className="sk" /><span className="sk" /></div>}
      </dl>
      {connected && <span className="tx-wallet" title={portfolio.owner!}><i className="live-dot" aria-hidden="true" />{shortKey(portfolio.owner!)}</span>}
    </header>

    <Tabs id="m" label="Trade screen sections" className="tx-mtabs" value={mobileTab} onChange={setMobileTab}
      tabs={[["trade", "Trade"], ["quotes", "Quotes"], ["trades", "Trades"], ["positions", "Positions"]]} />

    <div className="tx-main">
      <section className="tx-card tx-chart-card" aria-labelledby="chart-heading">
        <div className="tx-card-head">
          <div><h2 id="chart-heading">Mark price</h2><Link className="tx-why" to={site.paths.compute}>What moves this price</Link></div>
          {ranges.length > 1 && <div className="tx-ranges" role="group" aria-label="Chart range">{ranges.map(r => <button key={r.label} type="button" aria-pressed={range?.label === r.label} onClick={() => setRangeLabel(r.label)}>{r.label}</button>)}</div>}
        </div>
        {historyStale && <p className="tx-warn" role="status">{historyStale}</p>}
        {markSeries.length >= 2 ? <PriceChart mark={markSeries} index={indexSeries} from={chartFrom} to={chartTo} />
          : feed.failed.history ? <p className="tx-chart-empty" role="status">The price history is unreachable right now.</p>
          : computeFeed === null ? <p className="tx-chart-empty" role="status">The price history is unreachable right now.</p>
          : <div className="tx-chart-skel" aria-busy="true" aria-label="Loading the price history" />}
      </section>

      <section className={`tx-card tx-book m-quotes m-trades${mobileTab === "trades" ? " m-show-trades" : ""}`} aria-label="Quotes and trades">
        <Tabs id="book" label="Quotes and trades" value={bookTab} onChange={setBookTab} tabs={[["quotes", "Quotes"], ["trades", "Trades"]]} />
        <Panel id="book" k="quotes" value={mobileTab === "trades" ? "trades" : mobileTab === "quotes" ? "quotes" : bookTab}>
          <Quotes snapshot={snapshot} client={client} ladder={ladder} feedNote={feedNote} />
        </Panel>
        <Panel id="book" k="trades" value={mobileTab === "trades" ? "trades" : mobileTab === "quotes" ? "quotes" : bookTab}>
          <Trades data={idx} indexer={indexer ?? null} snapshot={snapshot} now={Date.now()} />
        </Panel>
      </section>

      <section className="tx-card tx-order m-trade" aria-labelledby="trade-heading">
        <h2 id="trade-heading" className="sr-only">Order</h2>
        <fieldset disabled={pending}>
          <div className="tx-side" role="group" aria-label="Position side">
            <button type="button" className="long" aria-pressed={side === "long"} onClick={() => setSide("long")}>Long</button>
            <button type="button" className="short" aria-pressed={side === "short"} onClick={() => setSide("short")}>Short</button>
          </div>
          <ul className="trade-cases">
            <li><b>Long</b> hedges an inference bill: it gains when prices rise.</li>
            <li><b>Short</b> hedges a seller's or miner's revenue: it gains when prices fall.</li>
          </ul>
          <label htmlFor="size">Size (units of {PERP.unit})</label>
          <div className="tx-input"><input id="size" type="number" inputMode="decimal" min="0" max={PERP.maxFillUnits} step="any" value={size} aria-describedby="size-help" onChange={e => setSize(e.target.value)} /><span>{PERP.unitShort}</span></div>
          <div className="tx-quick" role="group" aria-label="Quick size">
            {QUICK.map(q => <button key={q} type="button" onClick={() => setSize(String(q))}>{q}</button>)}
            <button type="button" onClick={() => setSize(String(liq ? Math.min(maxFill, side === "long" ? liq.maxLong : liq.maxShort) : PERP.maxFillUnits))}>Max</button>
          </div>
          <p id="size-help" className={`trade-help${overFill ? " is-over" : ""}`}>
            <span className="trade-notional">{Number.isFinite(notional) ? `${dollars(notional)} notional` : "— notional"}</span>{" · "}up to {PERP.maxFillUnits} units per fill
          </p>
          <label htmlFor="leverage">Leverage <output htmlFor="leverage">{number.format(lev)}x</output></label>
          <input id="leverage" className="tx-range" type="range" min="1" max={market.maxLeverage} step="0.5" value={leverage} aria-valuetext={`${number.format(lev)} times`} onChange={e => setLeverage(e.target.value)} />
          <div className="tx-range-ticks" aria-hidden="true"><span>1x</span><span>{number.format(market.maxLeverage)}x max</span></div>
          <dl className="tx-summary" aria-label="Order summary">
            {Number.isFinite(entry) && <><dt>Entry price</dt><dd>{perpPrice(entry)}</dd></>}
            {Number.isFinite(fee) && <><dt>Fee ({market.feeBps} bps)</dt><dd>{dollars(fee)}</dd></>}
            {Number.isFinite(margin) && <><dt>Margin required</dt><dd>{dollars(margin)}</dd></>}
            {liq && <><dt>Max this side</dt><dd>{num(side === "long" ? liq.maxLong : liq.maxShort)} {PERP.unitShort}</dd></>}
          </dl>
          {tradeOff && <p className="trade-help" role="status">Trading is off in this build: no matcher is set for {PERP.name}. Deposits and withdrawals still work.</p>}
          {connected
            ? <button type="button" className={`tx-submit ${side}`} disabled={tradeOff} onClick={open}>{side === "long" ? "Open long" : "Open short"}</button>
            : <button type="button" className="tx-submit" title="Connect wallet" onClick={() => void call(() => client.connect())}>Connect wallet</button>}
          <p className="error" role="status">{error}</p>
          <p className="trade-note">Test tokens</p>
        </fieldset>

        <div className="tx-account" aria-label="Account">
          <dl>
            <dt>Collateral</dt><dd>{dollars(portfolio.collateral)}</dd>
            <dt>Equity</dt><dd>{dollars(health.equity)}</dd>
            <dt>Free collateral</dt><dd>{dollars(health.freeCollateral)}</dd>
            <dt>Health</dt><dd><span className={`badge ${health.status}`}>{health.status}</span></dd>
          </dl>
          <fieldset disabled={pending}>
            <label htmlFor="amount">Amount (test USDC)</label>
            <div className="actions">
              <input id="amount" type="number" inputMode="decimal" min="0" step="any" value={amount} onChange={e => setAmount(e.target.value)} />
              <button type="button" onClick={() => void call(() => client.deposit(Number(amount)))}>Deposit</button>
              <button type="button" className="ghost" onClick={() => void call(() => client.withdraw(Number(amount)))}>Withdraw</button>
            </div>
            {site.faucetApi && connected && <button type="button" className="ghost tx-faucet" onClick={() => void claim()}>Get test USDC</button>}
          </fieldset>
        </div>
      </section>
    </div>

    <section className="tx-card tx-bottom m-positions" aria-label="Positions, fills and account">
      <Tabs id="bottom" label="Positions, fills and account" value={bottomTab} onChange={setBottomTab} tabs={bottomTabs} />
      <Panel id="bottom" k="positions" value={bottomTab}>
        <div aria-labelledby="position-heading">
          <h3 id="position-heading" className="sr-only">Your position</h3>
          {leg ? <div className="table-scroll"><table className="tx-table">
            <thead><tr><th scope="col">Market</th><th scope="col">Side</th><th scope="col" className="num">Size</th><th scope="col" className="num">Entry price</th><th scope="col" className="num">Mark</th><th scope="col" className="num">Unrealized PnL</th><th scope="col" className="num">Margin</th><th scope="col"><span className="sr-only">Action</span></th></tr></thead>
            <tbody><tr>
              <td>{PERP.name}</td><td className={leg.side === "long" ? "up" : "down"}>{leg.side}</td><td className="num">{number.format(leg.size)} × {PERP.unit}</td>
              <td className="num">{perpPrice(leg.entryPrice)}</td><td className="num">{perpPrice(mark.price)}</td>
              <td className={`num ${leg.unrealizedPnl > 0 ? "up" : leg.unrealizedPnl < 0 ? "down" : ""}`}>{dollars(leg.unrealizedPnl)}</td>
              <td className="num">{dollars(health.initialMarginUsed)}</td>
              <td><button type="button" className="ghost sm" disabled={tradeOff || pending} onClick={() => void call(() => client.closePosition())}>Close position</button></td>
            </tr></tbody>
          </table></div> : <p className="tx-empty">No open position</p>}
          <p className="tx-fine">Realized PnL: {dollars(portfolio.realizedPnl)}</p>
        </div>
      </Panel>
      <Panel id="bottom" k="fills" value={bottomTab}>
        {indexer ? <OwnFills data={idx} connected={connected} reachable={!idx.statsError || !!idx.stats} />
          : fills.some(f => f.mine) ? <div className="table-scroll"><table className="tx-table">
            <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col" className="num">Size ({PERP.unitShort})</th><th scope="col" className="num">Price</th></tr></thead>
            <tbody>{fills.filter(f => f.mine).map(f => <tr key={f.id}><td>{new Date(f.ts).toLocaleTimeString()}</td><td className={f.side === "long" ? "up" : "down"}>{f.side}</td><td className="num">{number.format(f.size)}</td><td className="num">{perpPrice(f.price)}</td></tr>)}</tbody>
          </table></div> : <p className="tx-empty">No fills yet.</p>}
      </Panel>
      {fundingShown && <Panel id="bottom" k="funding" value={bottomTab}>
        <dl className="tx-kv"><dt>Rate</dt><dd>{percent.format(funding.rate)} per hour</dd><dt>Next funding</dt><dd>{new Date(funding.nextFundingTs).toLocaleTimeString()} (in {countdown(funding.nextFundingTs - now)})</dd></dl>
      </Panel>}
      <Panel id="bottom" k="account" value={bottomTab}>
        <div aria-labelledby="health-heading">
          <h3 id="health-heading" className="sr-only">Margin &amp; health</h3>
          <dl className="tx-kv tx-kv-wide">
            <dt>Collateral</dt><dd>{dollars(portfolio.collateral)}</dd>
            <dt>Equity</dt><dd>{dollars(health.equity)}</dd>
            <dt>Initial margin used</dt><dd>{dollars(health.initialMarginUsed)}</dd>
            <dt>Maintenance margin</dt><dd>{dollars(health.maintenanceMargin)}</dd>
            <dt>Free collateral</dt><dd>{dollars(health.freeCollateral)}</dd>
            <dt>Health ratio</dt><dd>{Number.isFinite(health.ratio) ? number.format(health.ratio) : "—"}</dd>
            <dt>Status</dt><dd><span className={`badge ${health.status}`}>{health.status}</span></dd>
          </dl>
        </div>
      </Panel>
    </section>
  </main>;
}
