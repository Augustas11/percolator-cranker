import { useEffect, useState } from "react";
import type { Fills, IndexerClient, Positions, Stats } from "./indexer/client";

const numberFormatter = new Intl.NumberFormat("en-US", { maximumFractionDigits: 6 });
const usdFormatter = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const number = (value: number) => Number.isNaN(value) ? "—" : numberFormatter.format(value);
const usd = (value: number) => Number.isNaN(value) ? "—" : usdFormatter.format(value);
const fixed = (value: string) => {
  try { return Number(BigInt(value)) / 1e6; } catch { return NaN; }
};

export function IndexerPanels({ indexer, owner }: { indexer: IndexerClient | null; owner: string | null }) {
  const [stats, setStats] = useState<Stats | null>(null);
  const [positions, setPositions] = useState<Positions | null>(null);
  const [fills, setFills] = useState<(Fills["fills"][number] & { portfolio: string })[] | null>(null);
  const [moreHistory, setMoreHistory] = useState(false);
  const [statsError, setStatsError] = useState("");
  const [positionsError, setPositionsError] = useState("");
  const [fillsError, setFillsError] = useState("");

  useEffect(() => {
    let active = true;
    let pending = false;
    setStats(null);
    setPositions(null);
    setFills(null);
    setMoreHistory(false);
    setStatsError("");
    setPositionsError("");
    setFillsError("");
    if (!indexer) return;
    const message = (error: unknown) => error instanceof Error ? error.message : "indexer request failed";
    async function refresh() {
      if (pending) return;
      pending = true;
      await Promise.all([
        (async () => {
          try {
            const result = await indexer!.stats();
            if (active) { setStats(result); setStatsError(""); }
          } catch (error) { if (active) setStatsError(message(error)); }
        })(),
        (async () => {
          if (!owner) return;
          let result: Positions;
          try {
            result = await indexer!.positions(owner);
            if (!active) return;
            setPositions(result);
            setPositionsError("");
          } catch (error) {
            if (active) { setPositionsError(message(error)); setFillsError(message(error)); }
            return;
          }
          try {
            const histories = await Promise.all(result.portfolios.map(async p => ({ ...await indexer!.fills(p.portfolio), portfolio: p.portfolio })));
            const unique = new Map(histories.flatMap(h => h.fills.map(f => ({ ...f, portfolio: h.portfolio }))).map(f => [`${f.portfolio}:${f.signature}:${f.ix_index}:${f.leg}`, f]));
            if (active) { setFills([...unique.values()].sort((a, b) => b.slot - a.slot)); setMoreHistory(histories.some(h => h.next_before !== null)); setFillsError(""); }
          } catch (error) { if (active) setFillsError(message(error)); }
        })(),
      ]);
      pending = false;
    }
    void refresh();
    const timer = setInterval(() => void refresh(), 15000);
    return () => { active = false; clearInterval(timer); };
  }, [indexer, owner]);

  return <>
    <section aria-labelledby="indexer-stats-heading" className="fills">
      <h2 id="indexer-stats-heading">Market stats {indexer?.mock ? "(mock data, not devnet)" : "(devnet, test tokens)"}</h2>
      {!indexer ? <p>Indexer not configured</p> : <>
        {statsError && <p role="status">{statsError}</p>}
        {stats && <>
          {stats.as_of === null && <p>Indexer is still backfilling</p>}
          <dl>
            <dt>Cranks landed (all signers)</dt><dd>{number(stats.cranks.total)}</dd>
            <dt>Crank success rate <small>(landed transactions only)</small></dt><dd>{stats.cranks.success_rate === null ? "—" : `${number(stats.cranks.success_rate * 100)}%`}</dd>
            <dt>Crank uptime</dt><dd>{stats.uptime.uptime_pct === null ? "—" : `${number(stats.uptime.uptime_pct)}%`}</dd>
            <dt>Possible liquidations (heuristic)</dt><dd>{stats.liquidations_observed} ({stats.liquidations_since_slot === null ? "—" : `since slot ${stats.liquidations_since_slot}`})</dd>
          </dl>
          <p>Open interest</p>
          {Object.entries(stats.open_interest).map(([asset, interest]) => <p key={asset}>{asset}: long {number(fixed(interest.long_q))} SOL / short {number(fixed(interest.short_q))} SOL</p>)}
          <p>As of: {stats.as_of === null ? "not indexed yet" : new Date(stats.as_of * 1000).toLocaleString()}</p>
        </>}
      </>}
    </section>
    <section aria-labelledby="indexer-positions-heading" className="fills">
      <h2 id="indexer-positions-heading">Your positions {indexer?.mock ? "(mock data, not devnet)" : "(indexer)"}</h2>
      {!indexer ? <p>Indexer not configured</p> : !owner ? <p>Connect wallet to see positions</p> : <>
        {positionsError && <p role="status">{positionsError}</p>}
        <p>Size: stored basis, not effective. {indexer.mock ? "Mock data." : "Devnet, test tokens."}</p>
        {positions?.portfolios.map(p => <div key={p.portfolio}>
          <p>Portfolio: {p.portfolio}</p>
          <p>Capital: {usd(fixed(p.capital))} USD · PnL: {usd(fixed(p.pnl))} USD</p>
          {p.positions.map(leg => <p key={`${leg.asset_index}:${leg.market_id}`}>Asset {leg.asset_index}: {leg.side} {number(Math.abs(fixed(leg.basis_pos_q)))} SOL</p>)}
        </div>)}
        {positions && !positions.portfolios.some(p => p.positions.length) && <p>No indexed positions</p>}
      </>}
    </section>
    <section aria-labelledby="indexer-history-heading" className="fills">
      <h2 id="indexer-history-heading">Your trade history {indexer?.mock ? "(mock data, not devnet)" : "(indexer)"}</h2>
      {!indexer ? <p>Indexer not configured</p> : !owner ? <p>Connect wallet to see trades</p> : <>
        {fillsError && <p role="status">{fillsError}</p>}
        <p>Prices are estimates (last pushed mark). Requested = TradeCpi request; executed size not decoded.</p>
        <p>{indexer.mock ? "Mock data." : "Devnet, test tokens."}</p>
        <p>Latest 50 trades per current portfolio.</p>
        {moreHistory && <p>More history exists; not shown.</p>}
        <div className="table-scroll"><table>
          <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col">Size (SOL)</th><th scope="col">Mark price (est.)</th><th scope="col">Status</th><th scope="col">Tx</th></tr></thead>
          <tbody>{fills?.map(fill => <tr key={`${fill.portfolio}:${fill.signature}:${fill.ix_index}:${fill.leg}`}>
            <td>{fill.block_time === null ? "—" : new Date(fill.block_time * 1000).toLocaleString()}</td>
            <td>{fill.side}</td><td>{number(Math.abs(fixed(fill.size_q)))}</td>
            <td>{fill.mark_price === null || !Number.isSafeInteger(fill.mark_price) ? "—" : usd(fill.mark_price / 1e6)}</td><td>{fill.status}</td>
            <td><a href={`https://explorer.solana.com/tx/${encodeURIComponent(fill.signature)}?cluster=devnet`} target="_blank" rel="noreferrer">{fill.signature.slice(0, 7)}…{fill.signature.slice(-4)}</a></td>
          </tr>)}</tbody>
        </table></div>
        {fills !== null && fills.length === 0 && !fillsError && <p>No indexed trades for current portfolios</p>}
      </>}
    </section>
  </>;
}
