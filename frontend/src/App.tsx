import { useCallback, useState, useSyncExternalStore } from "react";
import type { MarketClient, Side } from "./market/types";
import "./App.css";

const usd = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const number = new Intl.NumberFormat("en-US", { maximumFractionDigits: 4 });
const percent = new Intl.NumberFormat("en-US", { style: "percent", maximumFractionDigits: 4 });

export function useSnapshot(client: MarketClient) {
  const subscribe = useCallback((notify: () => void) => client.subscribe(notify), [client]);
  const getSnapshot = useCallback(() => client.getSnapshot(), [client]);
  return useSyncExternalStore(subscribe, getSnapshot);
}

export default function App({ client }: { client: MarketClient }) {
  const { market, mark, funding, portfolio, fills } = useSnapshot(client);
  const [amount, setAmount] = useState("");
  const [size, setSize] = useState("1");
  const [leverage, setLeverage] = useState("5");
  const [side, setSide] = useState<Side>("long");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const leg = portfolio.legs[0];
  const health = portfolio.health;

  async function call(action: () => Promise<unknown>) {
    setPending(true);
    setError("");
    try { await action(); }
    catch (err) { setError(err instanceof Error ? err.message : "request failed"); }
    finally { setPending(false); }
  }

  return <>
    <div className="banner">PoC demo on devnet. Test tokens only, no real funds.</div>
    <main>
      <header>
        <h1>{market.symbol}</h1>
        <button title="placeholder, no wallet library yet" disabled={pending || !!portfolio.owner} onClick={() => void call(() => client.connect())}>
          {portfolio.owner ? `${portfolio.owner.slice(0, 7)}…${portfolio.owner.slice(-4)}` : "Connect wallet"}
        </button>
      </header>
      <p className="error" role="status">{error}</p>
      <div className="panels">
        <section aria-labelledby="market-heading">
          <h2 id="market-heading">Market</h2>
          <dl>
            <dt>Mark price</dt><dd>{usd.format(mark.price)}</dd>
            <dt>Funding</dt><dd>{percent.format(funding.rate)} per hour</dd>
            <dt>Next funding</dt><dd>{new Date(funding.nextFundingTs).toLocaleTimeString()}</dd>
          </dl>
        </section>
        <section aria-labelledby="position-heading">
          <h2 id="position-heading">Your position</h2>
          {leg ? <dl>
            <dt>Side</dt><dd>{leg.side}</dd>
            <dt>Size</dt><dd>{number.format(leg.size)} SOL</dd>
            <dt>Entry price</dt><dd>{usd.format(leg.entryPrice)}</dd>
            <dt>Unrealized PnL</dt><dd>{usd.format(leg.unrealizedPnl)}</dd>
          </dl> : <p>No open position</p>}
          <p>Realized PnL: {usd.format(portfolio.realizedPnl)}</p>
        </section>
        <section aria-labelledby="health-heading">
          <h2 id="health-heading">Margin &amp; health</h2>
          <dl>
            <dt>Collateral</dt><dd>{usd.format(portfolio.collateral)}</dd>
            <dt>Equity</dt><dd>{usd.format(health.equity)}</dd>
            <dt>Initial margin used</dt><dd>{usd.format(health.initialMarginUsed)}</dd>
            <dt>Maintenance margin</dt><dd>{usd.format(health.maintenanceMargin)}</dd>
            <dt>Free collateral</dt><dd>{usd.format(health.freeCollateral)}</dd>
            <dt>Health ratio</dt><dd>{number.format(health.ratio)}</dd>
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
          <fieldset disabled={pending}>
            <div className="actions" role="group" aria-label="Position side">
              <button aria-pressed={side === "long"} onClick={() => setSide("long")}>Long</button>
              <button aria-pressed={side === "short"} onClick={() => setSide("short")}>Short</button>
            </div>
            <label htmlFor="size">Size (SOL)</label>
            <input id="size" type="number" min="0" step="any" value={size} onChange={e => setSize(e.target.value)} />
            <label htmlFor="leverage">Leverage (1–{market.maxLeverage})</label>
            <input id="leverage" type="number" min="1" max={market.maxLeverage} step="any" value={leverage} onChange={e => setLeverage(e.target.value)} />
            <div className="actions">
              <button onClick={() => void call(() => client.openPosition(side, Number(size), Number(leverage)))}>Open</button>
              <button onClick={() => void call(() => client.closePosition())}>Close position</button>
            </div>
          </fieldset>
        </section>
      </div>
      <section aria-labelledby="fills-heading" className="fills">
        <h2 id="fills-heading">Recent fills</h2>
        <div className="table-scroll"><table>
          <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col">Size (SOL)</th><th scope="col">Price</th><th scope="col">Trader</th></tr></thead>
          <tbody>{fills.map(fill => <tr key={fill.id}>
            <td>{new Date(fill.ts).toLocaleTimeString()}</td><td>{fill.side}</td><td>{number.format(fill.size)}</td><td>{usd.format(fill.price)}</td><td>{fill.mine ? "you" : "bot"}</td>
          </tr>)}</tbody>
        </table></div>
        {fills.length === 0 && <p>No fills yet</p>}
      </section>
    </main>
  </>;
}
