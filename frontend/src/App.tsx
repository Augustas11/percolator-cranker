import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { ReactNode } from "react";
import { PERP, fillsPaused, perpPrice, perpSubtitle } from "./market/types";
import type { MarketClient, MarketSnapshot, Side } from "./market/types";
import { Link } from "./site/router";
import { site } from "./site/config";
import type { IndexerClient } from "./indexer/client";
import type { ComputeFeedClient } from "./compute/client";
import { useComputeFeed, latestStale, marketStale, modelStale } from "./compute/useFeed";
import { OTHER_MARKET, OwnFills, UNREACHABLE, entryFromFills, fixed, number as num, openInterestText, openUnits, useTradeIndexer } from "./Indexer";
import { DRIP_BELOW_SOL, claimFaucet, faucetHealth } from "./faucet";
import type { FaucetHealth } from "./faucet";
import type { TradeIndexer } from "./Indexer";
import PriceChart from "./trade/PriceChart";
import { availableRanges, benchmarkModel, change24h, cleanPoints, entryPrice, quoteSides, sellerLadder } from "./trade/quotes";
import FeedBoundary from "./compute/FeedBoundary";
import "./App.css";
import "./site/compute.css";
import "./site/trade.css";

const usd = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const number = new Intl.NumberFormat("en-US", { maximumFractionDigits: 4 });
const dollars = (value: number) => Number.isFinite(value) ? usd.format(value) : "—";
const shortKey = (w: string) => w.length > 8 ? `${w.slice(0, 4)}…${w.slice(-4)}` : w;
const usdAtoms = (atoms: string) => { const n = fixed(atoms); return Number.isFinite(n) ? usd.format(n) : null; };
const ago = (ms: number, now: number) => {
  const s = Math.max(0, Math.floor((now - ms) / 1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`;
};
const explorer = (kind: "tx" | "address", key: string) => `https://explorer.solana.com/${kind}/${encodeURIComponent(key)}?cluster=devnet`;
/** Seconds per slot on Solana, for turning the matcher's slot limit into a time a reader knows. */
const SLOT_S = 0.4;
const pausedText = (reason: ReturnType<typeof fillsPaused>, liq: MarketSnapshot["liquidity"]) =>
  reason === "paused" ? "Fills are paused by the liquidity account."
  : reason === "stale" ? `Fills are paused: the liquidity account's reference price is older than its ${num(liq?.maxAgeSlots ?? NaN)}-slot limit.`
  : reason === "deviates" ? `Fills are paused: the mark is more than ${num((liq?.deviationBps ?? NaN) / 100)}% from the liquidity account's reference price.`
  : null;
const QUICK = [10, 25, 50];

export function useSnapshot(client: MarketClient) {
  const subscribe = useCallback((notify: () => void) => client.subscribe(notify), [client]);
  const getSnapshot = useCallback(() => client.getSnapshot(), [client]);
  return useSyncExternalStore(subscribe, getSnapshot);
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
        {pausedText(fillsPaused(liq, snapshot.mark.price), liq) && <p className="tx-warn tx-paused" role="status">{pausedText(fillsPaused(liq, snapshot.mark.price), liq)}</p>}
        <dl className="tx-sides">
          <div className="ask"><dt>Ask <small>long fills</small></dt><dd>{sides ? perpPrice(sides.ask) : "—"}</dd><dd className="lim">up to {num(liq.maxLong)} {PERP.unitShort}</dd></div>
          <div className="bid"><dt>Bid <small>short fills</small></dt><dd>{sides ? perpPrice(sides.bid) : "—"}</dd><dd className="lim">up to {num(liq.maxShort)} {PERP.unitShort}</dd></div>
        </dl>
        <p className="tx-fine">Spread {num(liq.spreadBps)} bps: the account fills at the mark{fee !== undefined ? <>, plus a {fee} bps fee</> : null}. Max {num(liq.maxFill)} {PERP.unitShort} per fill. Fills stop if the mark moves more than {num(liq.deviationBps / 100)}% from the account's reference price{liq.maxAgeSlots !== undefined && <>, or if that reference price is older than {num(liq.maxAgeSlots)} slots (about {num(Math.round(liq.maxAgeSlots * SLOT_S / 60))} minutes)</>}.</p>
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

type TradeRow = { key: string; ts: number | null; side: Side; size: number; price: number | null; wallet: string | null; href: string | null };

function Trades({ data, indexer, snapshot, now }: { data: TradeIndexer; indexer: IndexerClient | null; snapshot: MarketSnapshot; now: number }) {
  const other = data.sameMarket === false;
  const price = (e6: number | null) => e6 !== null && Number.isSafeInteger(e6) && e6 > 0 ? e6 / 1e6 : null;
  // Newest source first: market-wide /trades, then the older recent-fills feed, then this session's fills.
  const rows: TradeRow[] = !other && data.trades !== null
    ? data.trades.map(t => ({ key: `${t.signature}:${t.ix_index}:${t.leg}`, ts: t.block_time === null ? null : t.block_time * 1000, side: t.side === "buy" ? "long" : "short",
      size: Math.abs(fixed(t.size)), price: price(t.price), wallet: t.wallet, href: explorer("tx", t.signature) }))
    : !other && data.recent && data.recent.length
    ? data.recent.map(f => ({ key: `${f.signature}:${f.ix_index}:${f.leg}`, ts: f.block_time === null ? null : f.block_time * 1000, side: f.side === "buy" ? "long" : "short",
      size: Math.abs(fixed(f.size_q)), price: price(f.mark_price), wallet: f.wallet, href: explorer("tx", f.signature) }))
    : other ? [] : snapshot.fills.map(f => ({ key: f.id, ts: f.ts, side: f.side, size: f.size, price: Number.isFinite(f.price) ? f.price : null, wallet: f.wallet ?? null, href: null }));
  return <>
    {!indexer ? null : other ? <p className="tx-warn" role="status">{OTHER_MARKET}</p> : data.statsError && !data.recent && !data.trades ? <p className="tx-warn" role="status">{UNREACHABLE}</p> : null}
    {rows.length === 0 ? !other && <p className="tx-empty">No trades yet.</p>
      : <div className="table-scroll"><table className="tx-table tx-trades">
        <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col" className="num">Size</th><th scope="col" className="num">Price</th><th scope="col">Wallet</th></tr></thead>
        <tbody>{rows.map(r => <tr key={r.key}>
          <td>{r.href ? <a href={r.href} target="_blank" rel="noreferrer">{r.ts === null ? "tx" : ago(r.ts, now)}</a> : r.ts === null ? "" : ago(r.ts, now)}</td>
          <td className={r.side === "long" ? "up" : "down"}>{r.side === "long" ? "Long" : "Short"}</td><td className="num">{num(r.size)}</td><td className="num">{r.price === null ? "" : perpPrice(r.price)}</td>
          <td>{r.wallet ? <a href={explorer("address", r.wallet)} target="_blank" rel="noreferrer" title={r.wallet}>{shortKey(r.wallet)}</a> : ""}</td>
        </tr>)}</tbody>
      </table></div>}
  </>;
}

/** Wallet balances and the faucet's limits, for the "Get test USDC" flow. */
function useWalletFunds(client: MarketClient, owner: string | null) {
  const [balances, setBalances] = useState<{ sol: number; usdc: number | null } | null>(null);
  const [limits, setLimits] = useState<FaucetHealth | null>(null);
  const reload = useCallback(async () => {
    if (!owner || !client.walletBalances) return;
    try { setBalances(await client.walletBalances()); } catch { /* keep the last read */ }
  }, [client, owner]);
  useEffect(() => { setBalances(null); void reload(); }, [reload]);
  useEffect(() => {
    if (!owner || !site.faucetApi) return;
    let active = true;
    void faucetHealth(site.faucetApi).then(h => { if (active) setLimits(h); });
    return () => { active = false; };
  }, [owner]);
  return { balances, limits, reload };
}

export default function App({ client, indexer, computeFeed = null, requestedSide, sideRequest }: { client: MarketClient; indexer?: IndexerClient | null; computeFeed?: ComputeFeedClient | null; requestedSide?: Side; sideRequest?: number }) {
  const snapshot = useSnapshot(client);
  const { market, mark, portfolio, fills } = snapshot;
  const feed = useComputeFeed(computeFeed);
  const idx = useTradeIndexer(indexer ?? null, portfolio.owner);
  const funds = useWalletFunds(client, portfolio.owner);
  const [amount, setAmount] = useState("");
  const [size, setSize] = useState(String(PERP.defaultSize));
  const [side, setSide] = useState<Side>("long");
  useEffect(() => { if (requestedSide) setSide(requestedSide); }, [requestedSide, sideRequest]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [errorDetails, setErrorDetails] = useState("");
  const [notice, setNotice] = useState("");
  const [bookTab, setBookTab] = useState<"quotes" | "trades">("quotes");
  const [bottomTab, setBottomTab] = useState<"positions" | "fills" | "account">("positions");
  const [mobileTab, setMobileTab] = useState<"trade" | "quotes" | "trades" | "positions">("trade");
  const [rangeLabel, setRangeLabel] = useState<string | null>(null);
  const leg = portfolio.legs[0];
  const health = portfolio.health;
  const liq = snapshot.liquidity ?? null;
  // The matcher's own per-fill limit when it has been read from chain; the configured value until then.
  const maxFill = liq && liq.maxFill > 0 ? liq.maxFill : PERP.maxFillUnits;
  const paused = fillsPaused(liq, mark.price);
  const units = Number(size);
  const sized = size.trim() !== "" && Number.isFinite(units) && units > 0;
  const overFill = sized && units > maxFill;
  const notional = sized && Number.isFinite(mark.price) ? units * mark.price : NaN;
  const entry = liq ? entryPrice(side, mark.price, liq.spreadBps) : NaN;
  const fee = market.feeBps !== undefined && Number.isFinite(notional) ? notional * market.feeBps / 10_000 : NaN;
  // Leverage is not an order argument: the engine holds initial margin at its own ratio of notional (initial_margin_bps).
  const imr = market.initialMarginRatio ?? (market.maxLeverage > 0 ? 1 / market.maxLeverage : NaN);
  const margin = Number.isFinite(notional) && imr > 0 ? notional * imr : NaN;
  const perUnit = mark.price * (imr + (market.feeBps ?? 0) / 10_000);
  const maxForCollateral = portfolio.owner && Number.isFinite(health.freeCollateral) && health.freeCollateral >= 0 && perUnit > 0
    ? Math.floor(health.freeCollateral / perUnit * 100) / 100 : NaN;

  // The benchmark model in market.json: the all-seller index and the seller ladder.
  const bench = useMemo(() => benchmarkModel(feed.market, PERP.benchmarkId), [feed.market]);
  const index = bench ? bench.median : null;
  const feedNow = feed.now;
  const benchStale = bench ? modelStale(bench, feedNow) || !!marketStale(feed.market, feedNow) : false;
  const ladder = useMemo(() => bench && bench.quotes.length ? sellerLadder(bench, Number.isFinite(mark.price) ? mark.price : null, index) : null, [bench, mark.price, index]);
  const feedNote = feed.failed.market && !feed.market ? "The compute feed is unreachable right now." : benchStale ? "These quotes are over 30 minutes old." : feed.loaded && feed.market && !bench?.quotes.length ? `The feed has no quotes for ${PERP.benchmark} right now.` : null;

  // The chart: pushed marks from history.json, the live mark at the end, the index when it has 12 or more reads.
  const history = feed.history;
  const markSeries = useMemo(() => {
    const pts = cleanPoints(history?.points);
    return Number.isFinite(mark.price) && pts.length ? [...pts, [Math.max(Date.now(), pts[pts.length - 1][0]), mark.price] as [number, number]] : pts;
  }, [history, mark.price]);
  const ranges = availableRanges(markSeries);
  const range = ranges.find(r => r.label === rangeLabel) ?? ranges[0] ?? null;
  const chartTo = markSeries.length ? markSeries[markSeries.length - 1][0] : 0;
  const chartFrom = range ? chartTo - range.ms : markSeries.length ? markSeries[0][0] : 0;
  const indexSeries = bench && bench.series.length >= 12 ? bench.series : null;
  // What a request pays now across the sellers that are up (market.json routed); a line once it has 12 reads.
  const routed = bench?.routed ?? null;
  const routedSeries = routed && routed.series.length >= 12 ? routed.series : null;
  const historyStale = latestStale(feed.latest, feedNow);

  // Market bar: each stat only when its source has a value.
  const change = history ? change24h(markSeries, chartTo) : null;
  const same = idx.sameMarket === true;
  const oi = same ? openInterestText(idx.season?.open_interest_usd, openUnits(idx.stats)) : null;
  const vol24 = same && idx.stats?.volume_24h != null ? usdAtoms(idx.stats.volume_24h) : null;
  const fills24 = same && Number.isSafeInteger(idx.stats?.fills_24h) ? idx.stats!.fills_24h! : null;
  const volAll = same && vol24 === null && idx.season ? usdAtoms(idx.season.volume_usd) : null;
  const stats: Stat[] = [
    ...(Number.isFinite(mark.price) ? [["Mark", perpPrice(mark.price), "mark"] as Stat] : []),
    ...(index !== null ? [["Index (all sellers)", <>{perpPrice(index)}{benchStale && <em className="tag"> stale</em>}</>] as Stat] : []),
    ...(routed ? [["Paid now (all sellers)", <>{perpPrice(routed.price)}{benchStale && <em className="tag"> stale</em>}</>, "routed"] as Stat] : []),
    ...(change !== null ? [["24 h change", `${change > 0 ? "+" : change < 0 ? "−" : ""}${Math.abs(change).toFixed(2)}%`, change > 0 ? "up" : change < 0 ? "down" : ""] as Stat] : []),
    ...(vol24 !== null ? [["24 h volume", vol24] as Stat] : []),
    ...(fills24 !== null ? [["24 h fills", num(fills24)] as Stat] : []),
    ...(volAll !== null ? [["Volume (all time)", volAll] as Stat] : []),
    ...(oi ? [["Open interest", oi] as Stat] : []),
  ];

  // The open position. Entry and PnL only from a real source: the client, or the wallet's own executed fills.
  const signed = leg ? (leg.side === "long" ? leg.size : -leg.size) : 0;
  const legEntry = leg && Number.isFinite(leg.entryPrice) ? leg.entryPrice
    : leg && same ? entryFromFills(idx.fills, signed, !idx.moreHistory) : null;
  const legPnl = leg && Number.isFinite(leg.unrealizedPnl) ? leg.unrealizedPnl
    : leg && legEntry !== null && Number.isFinite(mark.price) ? (mark.price - legEntry) * signed : null;
  const legMargin = leg && Number.isFinite(health.initialMarginUsed) ? health.initialMarginUsed : null;
  const indexedPnl = same && idx.portfolios && idx.portfolios.length ? idx.portfolios.reduce((sum, p) => sum + fixed(p.pnl), 0) : NaN;

  function open() {
    if (overFill) { setError(`The liquidity account fills up to ${num(maxFill)} units (${num(maxFill)} × ${PERP.unit}) per trade. Split a larger order into several trades.`); return; }
    void call(() => client.openPosition(side, units, market.maxLeverage));
  }

  async function call(action: () => Promise<unknown>) {
    setPending(true);
    setError("");
    setErrorDetails("");
    setNotice("");
    try { await action(); }
    catch (err) {
      setError(err instanceof Error ? err.message : "The request failed. Try again.");
      // Chain failures carry the raw text; it stays one click away.
      const details = (err as { details?: unknown })?.details;
      if (typeof details === "string" && details && details !== (err as Error).message) setErrorDetails(details);
    }
    finally { setPending(false); void funds.reload(); }
  }

  const connected = !!portfolio.owner;
  const tradeOff = client.tradingReady === false;
  // A wallet with SOL gets no drip, so the faucet will not create its token account: it creates it itself first.
  const needsAccount = !!funds.balances && funds.balances.usdc === null && funds.balances.sol >= DRIP_BELOW_SOL && !!client.createTokenAccount;
  const limits = funds.limits;
  const faucetHelp = limits ? `${num(limits.amount_usdc)} test USDC per claim, one claim per wallet every 24 h, ${num(limits.daily_cap)} claims a day across all wallets.` : null;

  async function claim() {
    if (!site.faucetApi || !portfolio.owner) return;
    const base = site.faucetApi, owner = portfolio.owner;
    await call(async () => {
      const got = await claimFaucet(base, owner);
      setNotice(`Sent ${num(got.amount_usdc)} test USDC to your wallet.`);
    });
  }

  const accountRows: [string, string][] = connected ? [
    ...(Number.isFinite(portfolio.collateral) ? [["Collateral", dollars(portfolio.collateral)] as [string, string]] : []),
    ...(Number.isFinite(health.equity) ? [["Equity", dollars(health.equity)] as [string, string]] : []),
    ...(Number.isFinite(indexedPnl) ? [["PnL (last indexed)", dollars(indexedPnl)] as [string, string]] : []),
    ...(Number.isFinite(health.initialMarginUsed) && leg ? [["Initial margin used", dollars(health.initialMarginUsed)] as [string, string]] : []),
    ...(Number.isFinite(health.maintenanceMargin) && leg ? [["Maintenance margin", dollars(health.maintenanceMargin)] as [string, string]] : []),
    ...(Number.isFinite(health.freeCollateral) ? [["Free collateral", dollars(health.freeCollateral)] as [string, string]] : []),
    ...(Number.isFinite(health.ratio) ? [["Health ratio", number.format(health.ratio)] as [string, string]] : []),
  ] : [];
  const status = connected && health.status !== "unknown" ? health.status : null;

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
        {markSeries.length >= 2 ? <FeedBoundary name="Price chart" data={history}><PriceChart mark={markSeries} index={indexSeries} routed={routedSeries} from={chartFrom} to={chartTo} /></FeedBoundary>
          : feed.failed.history || computeFeed === null ? <p className="tx-chart-empty" role="status">The price history is unreachable right now.</p>
          : history !== null ? <p className="tx-chart-empty" role="status">{markSeries.length === 1
            ? `One mark pushed so far: ${perpPrice(markSeries[0][1])} at ${new Date(markSeries[0][0]).toLocaleString("en-GB", { timeZone: "UTC", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" })} UTC. The chart starts with the second.`
            : "No mark has been pushed yet."}</p>
          : <div className="tx-chart-skel" aria-busy="true" aria-label="Loading the price history" />}
      </section>

      <section className={`tx-card tx-book m-quotes m-trades${mobileTab === "trades" ? " m-show-trades" : ""}`} aria-label="Quotes and trades">
        <Tabs id="book" label="Quotes and trades" value={bookTab} onChange={setBookTab} tabs={[["quotes", "Quotes"], ["trades", "Trades"]]} />
        <Panel id="book" k="quotes" value={mobileTab === "trades" ? "trades" : mobileTab === "quotes" ? "quotes" : bookTab}>
          <FeedBoundary name="Quotes" data={feed.market}><Quotes snapshot={snapshot} client={client} ladder={ladder} feedNote={feedNote} /></FeedBoundary>
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
          <div className="tx-input"><input id="size" type="number" inputMode="decimal" min="0" max={maxFill} step="any" value={size} aria-describedby="size-help" onChange={e => setSize(e.target.value)} /><span>{PERP.unitShort}</span></div>
          <div className="tx-quick" role="group" aria-label="Quick size">
            {QUICK.map(q => <button key={q} type="button" onClick={() => setSize(String(q))}>{q}</button>)}
            <button type="button" onClick={() => setSize(String(liq ? Math.min(maxFill, side === "long" ? liq.maxLong : liq.maxShort) : maxFill))}>Max</button>
          </div>
          <p id="size-help" className={`trade-help${overFill ? " is-over" : ""}`}>
            <span className="trade-notional">{Number.isFinite(notional) ? `${dollars(notional)} notional` : "— notional"}</span>{" · "}up to {num(maxFill)} units per fill
          </p>
          <dl className="tx-summary" aria-label="Order summary">
            {Number.isFinite(entry) && <><dt>Entry price</dt><dd>{perpPrice(entry)}</dd></>}
            {Number.isFinite(fee) && <><dt>Fee ({market.feeBps} bps)</dt><dd>{dollars(fee)}</dd></>}
            {Number.isFinite(margin) && <><dt>Margin required ({number.format(imr * 100)}%)</dt><dd>{dollars(margin)}</dd></>}
            {Number.isFinite(market.maxLeverage) && market.maxLeverage > 0 && <><dt>Max leverage</dt><dd>{number.format(market.maxLeverage)}x</dd></>}
            {liq && <><dt>Max this side</dt><dd>{num(side === "long" ? liq.maxLong : liq.maxShort)} {PERP.unitShort}</dd></>}
            {Number.isFinite(maxForCollateral) && <><dt>Max for your collateral</dt><dd>{num(maxForCollateral)} {PERP.unitShort}</dd></>}
          </dl>
          {tradeOff && <p className="trade-help" role="status">Trading is off in this build: no matcher is set for {PERP.name}. Deposits and withdrawals still work.</p>}
          {!tradeOff && paused && <p className="trade-help tx-paused" role="status">{pausedText(paused, liq)}</p>}
          {connected
            ? <button type="button" className={`tx-submit ${side}`} disabled={tradeOff || !!paused} onClick={open}>{side === "long" ? "Open long" : "Open short"}</button>
            : <button type="button" className="tx-submit" title="Connect wallet" onClick={() => void call(() => client.connect())}>Connect wallet</button>}
          <p className="error" role="status">{error}</p>
          {errorDetails && <details className="tx-error-details"><summary>Details</summary><pre>{errorDetails}</pre></details>}
          <p className="trade-note">Test tokens</p>
        </fieldset>

        <div className="tx-account" aria-label="Account">
          {!connected ? <p className="tx-empty">Connect a wallet to see your balance and deposit test USDC.</p> : <>
            <dl>{accountRows.map(([k, v]) => <div key={k} className="tx-row"><dt>{k}</dt><dd>{v}</dd></div>)}
              {status && <div className="tx-row"><dt>Health</dt><dd><span className={`badge ${status}`}>{status}</span></dd></div>}
              {funds.balances && <div className="tx-row"><dt>In your wallet</dt><dd>{funds.balances.usdc === null ? "no test USDC account" : `${num(funds.balances.usdc)} test USDC`}</dd></div>}
            </dl>
            <fieldset disabled={pending}>
              <label htmlFor="amount">Amount (test USDC)</label>
              <div className="actions">
                <input id="amount" type="number" inputMode="decimal" min="0" step="any" value={amount} onChange={e => setAmount(e.target.value)} />
                <button type="button" onClick={() => void call(() => client.deposit(Number(amount)))}>Deposit</button>
                <button type="button" className="ghost" onClick={() => void call(() => client.withdraw(Number(amount)))}>Withdraw</button>
              </div>
              {site.faucetApi && <div className="tx-faucet">
                {needsAccount
                  ? <><button type="button" className="ghost" onClick={() => void call(async () => { await client.createTokenAccount!(); setNotice("Your test USDC account is ready. Now claim from the faucet."); })}>Create your test USDC account</button>
                    <p className="trade-help">Step 1 of 2: one transaction from your wallet, a small SOL fee. Then claim.</p></>
                  : <button type="button" className="ghost" aria-describedby={faucetHelp ? "faucet-help" : undefined} onClick={() => void claim()}>Get test USDC</button>}
                {faucetHelp && <p id="faucet-help" className="trade-help">{faucetHelp}</p>}
              </div>}
              {notice && <p className="tx-notice" role="status">{notice}</p>}
            </fieldset>
          </>}
        </div>
      </section>
    </div>

    <section className="tx-card tx-bottom m-positions" aria-label="Positions, fills and account">
      <Tabs id="bottom" label="Positions, fills and account" value={bottomTab} onChange={setBottomTab} tabs={[["positions", "Positions"], ["fills", "Fills"], ["account", "Account"]]} />
      <Panel id="bottom" k="positions" value={bottomTab}>
        <div aria-labelledby="position-heading">
          <h3 id="position-heading" className="sr-only">Your position</h3>
          {!connected ? <p className="tx-empty">Connect a wallet to see your position.</p>
            : leg ? <div className="table-scroll"><table className="tx-table">
            <thead><tr><th scope="col">Market</th><th scope="col">Side</th><th scope="col" className="num">Size</th>{legEntry !== null && <th scope="col" className="num">Entry price</th>}<th scope="col" className="num">Mark</th>{legPnl !== null && <th scope="col" className="num">Unrealized PnL</th>}{legMargin !== null && <th scope="col" className="num">Margin</th>}<th scope="col"><span className="sr-only">Action</span></th></tr></thead>
            <tbody><tr>
              <td>{PERP.name}</td><td className={leg.side === "long" ? "up" : "down"}>{leg.side}</td><td className="num">{number.format(leg.size)} × {PERP.unit}</td>
              {legEntry !== null && <td className="num">{perpPrice(legEntry)}</td>}<td className="num">{perpPrice(mark.price)}</td>
              {legPnl !== null && <td className={`num ${legPnl > 0 ? "up" : legPnl < 0 ? "down" : ""}`}>{dollars(legPnl)}</td>}
              {legMargin !== null && <td className="num">{dollars(legMargin)}</td>}
              <td><button type="button" className="ghost sm" disabled={tradeOff || pending} onClick={() => void call(() => client.closePosition())}>Close position</button></td>
            </tr></tbody>
          </table></div> : <p className="tx-empty">No open position</p>}
          {connected && Number.isFinite(portfolio.realizedPnl) && <p className="tx-fine">Realized PnL: {dollars(portfolio.realizedPnl)}</p>}
        </div>
      </Panel>
      <Panel id="bottom" k="fills" value={bottomTab}>
        {indexer ? <OwnFills data={idx} connected={connected} reachable={!idx.statsError || !!idx.stats} />
          : fills.some(f => f.mine) ? <div className="table-scroll"><table className="tx-table">
            <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col" className="num">Size ({PERP.unitShort})</th><th scope="col" className="num">Price</th></tr></thead>
            <tbody>{fills.filter(f => f.mine).map(f => <tr key={f.id}><td>{new Date(f.ts).toLocaleTimeString()}</td><td className={f.side === "long" ? "up" : "down"}>{f.side}</td><td className="num">{number.format(f.size)}</td><td className="num">{perpPrice(f.price)}</td></tr>)}</tbody>
          </table></div> : <p className="tx-empty">{connected ? "No fills yet." : "Connect a wallet to see your fills."}</p>}
      </Panel>
      <Panel id="bottom" k="account" value={bottomTab}>
        <div aria-labelledby="health-heading">
          <h3 id="health-heading" className="sr-only">Margin &amp; health</h3>
          {!connected ? <p className="tx-empty">Connect a wallet to see your account.</p>
            : <dl className="tx-kv tx-kv-wide">
              {accountRows.map(([k, v]) => <div key={k} className="tx-row"><dt>{k}</dt><dd>{v}</dd></div>)}
              {status && <div className="tx-row"><dt>Status</dt><dd><span className={`badge ${status}`}>{status}</span></dd></div>}
            </dl>}
        </div>
      </Panel>
    </section>
  </main>;
}
