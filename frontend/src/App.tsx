import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { PERP, perpPrice, perpSubtitle } from "./market/types";
import type { MarketClient, Side } from "./market/types";
import { Link } from "./site/router";
import type { IndexerClient } from "./indexer/client";
import { IndexerPanels } from "./Indexer";
import "./App.css";

const usd = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const number = new Intl.NumberFormat("en-US", { maximumFractionDigits: 4 });
const percent = new Intl.NumberFormat("en-US", { style: "percent", maximumFractionDigits: 4 });
const dollars = (value: number) => Number.isFinite(value) ? usd.format(value) : "—";

export function useSnapshot(client: MarketClient) {
  const subscribe = useCallback((notify: () => void) => client.subscribe(notify), [client]);
  const getSnapshot = useCallback(() => client.getSnapshot(), [client]);
  return useSyncExternalStore(subscribe, getSnapshot);
}

export default function App({ client, indexer, requestedSide, sideRequest }: { client: MarketClient; indexer?: IndexerClient | null; requestedSide?: Side; sideRequest?: number }) {
  const { market, mark, funding, portfolio, fills } = useSnapshot(client);
  const [amount, setAmount] = useState("");
  const [size, setSize] = useState(String(PERP.defaultSize));
  const [leverage, setLeverage] = useState("1");
  const [side, setSide] = useState<Side>("long");
  useEffect(() => { if (requestedSide) setSide(requestedSide); }, [requestedSide, sideRequest]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const leg = portfolio.legs[0];
  const health = portfolio.health;
  const units = Number(size);
  const sized = size.trim() !== "" && Number.isFinite(units) && units > 0;
  const overFill = sized && units > PERP.maxFillUnits;
  const notional = sized && Number.isFinite(mark.price) ? units * mark.price : NaN;

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

  return <>
    <main className="trade">
      <header>
        <div className="trade-title"><h1>{market.symbol}</h1><p className="trade-sub">{perpSubtitle} · <Link to="/compute">What moves this price</Link></p></div>
        <button title="Connect wallet" disabled={pending || !!portfolio.owner} onClick={() => void call(() => client.connect())}>
          {portfolio.owner ? `${portfolio.owner.slice(0, 7)}…${portfolio.owner.slice(-4)}` : "Connect wallet"}
        </button>
      </header>
      <p className="error" role="status">{error}</p>
      <div className="panels">
        <section aria-labelledby="market-heading">
          <h2 id="market-heading">Market</h2>
          <dl>
            <dt>Mark price</dt><dd>{perpPrice(mark.price)}</dd>
            <dt>Funding</dt><dd>{percent.format(funding.rate)} per hour</dd>
            <dt>Next funding</dt><dd>{new Date(funding.nextFundingTs).toLocaleTimeString()}</dd>
          </dl>
        </section>
        <section aria-labelledby="position-heading">
          <h2 id="position-heading">Your position</h2>
          {leg ? <dl>
            <dt>Side</dt><dd>{leg.side}</dd>
            <dt>Size</dt><dd>{number.format(leg.size)} × {PERP.unit}</dd>
            <dt>Entry price</dt><dd>{perpPrice(leg.entryPrice)}</dd>
            <dt>Unrealized PnL</dt><dd>{dollars(leg.unrealizedPnl)}</dd>
          </dl> : <p>No open position</p>}
          <p>Realized PnL: {dollars(portfolio.realizedPnl)}</p>
        </section>
        <section aria-labelledby="health-heading">
          <h2 id="health-heading">Margin &amp; health</h2>
          <dl>
            <dt>Collateral</dt><dd>{dollars(portfolio.collateral)}</dd>
            <dt>Equity</dt><dd>{dollars(health.equity)}</dd>
            <dt>Initial margin used</dt><dd>{dollars(health.initialMarginUsed)}</dd>
            <dt>Maintenance margin</dt><dd>{dollars(health.maintenanceMargin)}</dd>
            <dt>Free collateral</dt><dd>{dollars(health.freeCollateral)}</dd>
            <dt>Health ratio</dt><dd>{Number.isFinite(health.ratio) ? number.format(health.ratio) : "—"}</dd>
            <dt>Status</dt><dd><span className={`badge ${health.status}`}>{health.status}</span></dd>
          </dl>
        </section>
        <section aria-labelledby="collateral-heading">
          <h2 id="collateral-heading">Collateral</h2>
          <fieldset disabled={pending}>
            <label htmlFor="amount">Amount (test USDC)</label>
            <input id="amount" type="number" min="0" step="any" value={amount} onChange={e => setAmount(e.target.value)} />
            <div className="actions">
              <button onClick={() => void call(() => client.deposit(Number(amount)))}>Deposit</button>
              <button onClick={() => void call(() => client.withdraw(Number(amount)))}>Withdraw</button>
            </div>
          </fieldset>
        </section>
        <section aria-labelledby="trade-heading">
          <h2 id="trade-heading">Trade</h2>
          {client.tradingReady === false && <p className="trade-help" role="status">Trading is off in this build: no matcher is set for {PERP.name}. Deposits and withdrawals still work.</p>}
          <fieldset disabled={pending}>
            <div className="actions" role="group" aria-label="Position side">
              <button aria-pressed={side === "long"} onClick={() => setSide("long")}>Long</button>
              <button aria-pressed={side === "short"} onClick={() => setSide("short")}>Short</button>
            </div>
            <ul className="trade-cases">
              <li><b>Long</b> hedges an inference bill: it gains when prices rise.</li>
              <li><b>Short</b> hedges a seller's or miner's revenue: it gains when prices fall.</li>
            </ul>
            <label htmlFor="size">Size (units of {PERP.unit})</label>
            <input id="size" type="number" min="0" max={PERP.maxFillUnits} step="any" value={size} aria-describedby="size-help" onChange={e => setSize(e.target.value)} />
            <p id="size-help" className={`trade-help${overFill ? " is-over" : ""}`}>
              <span className="trade-notional">{Number.isFinite(notional) ? `${dollars(notional)} notional` : "— notional"}</span>{" · "}up to {PERP.maxFillUnits} units per fill
            </p>
            <label htmlFor="leverage">Leverage (1–{number.format(market.maxLeverage)})</label>
            <input id="leverage" type="number" min="1" max={market.maxLeverage} step="any" value={leverage} onChange={e => setLeverage(e.target.value)} />
            <div className="actions">
              <button disabled={client.tradingReady === false} onClick={open}>Open</button>
              <button disabled={client.tradingReady === false} onClick={() => void call(() => client.closePosition())}>Close position</button>
              <span className="trade-note">Test tokens</span>
            </div>
          </fieldset>
        </section>
      </div>
      <section aria-labelledby="fills-heading" className="fills">
        <h2 id="fills-heading">Recent fills</h2>
        <div className="table-scroll"><table>
          <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col">Size ({PERP.unitShort})</th><th scope="col">Price</th><th scope="col">Trader</th></tr></thead>
          <tbody>{fills.map(fill => <tr key={fill.id}>
            <td>{new Date(fill.ts).toLocaleTimeString()}</td><td>{fill.side}</td><td>{number.format(fill.size)}</td><td>{perpPrice(fill.price)}</td><td>{fill.wallet ? `${fill.wallet.slice(0, 4)}…${fill.wallet.slice(-4)}` : "—"}{fill.mine && " (you)"}</td>
          </tr>)}</tbody>
        </table></div>
        {fills.length === 0 && <p>No fills yet</p>}
      </section>
      <IndexerPanels indexer={indexer ?? null} owner={portfolio.owner} />
    </main>
  </>;
}
