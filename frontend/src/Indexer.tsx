import { useEffect, useState } from "react";
import type { Fills, IndexerClient, Season, Stats } from "./indexer/client";
import { PERP, perpPrice } from "./market/types";

const numberFormatter = new Intl.NumberFormat("en-US", { maximumFractionDigits: 2 });
const usdFormatter = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const number = (value: number) => Number.isFinite(value) ? numberFormatter.format(value) : "—";
const fixed = (value: string) => {
  try { return Number(BigInt(value)) / 1e6; } catch { return NaN; }
};
const usdAtoms = (value: string) => {
  const n = fixed(value);
  return Number.isFinite(n) ? usdFormatter.format(n) : "—";
};
const UNREACHABLE = "The indexer is unreachable right now. Try again in a minute.";

type OwnFill = Fills["fills"][number] & { portfolio: string };

/** The trade page's indexed panels: market open interest and volume, and the connected wallet's own fills. */
export function IndexerPanels({ indexer, owner }: { indexer: IndexerClient | null; owner: string | null }) {
  const [stats, setStats] = useState<Stats | null>(null);
  const [season, setSeason] = useState<Season | null>(null);
  const [fills, setFills] = useState<OwnFill[] | null>(null);
  const [moreHistory, setMoreHistory] = useState(false);
  const [statsError, setStatsError] = useState("");
  const [fillsError, setFillsError] = useState("");

  useEffect(() => {
    let active = true;
    let pending = false;
    setStats(null);
    setSeason(null);
    setFills(null);
    setMoreHistory(false);
    setStatsError("");
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
          try {
            const result = await indexer!.season?.();
            if (active && result) setSeason(result);
          } catch { /* volume is optional; keep the last value */ }
        })(),
        (async () => {
          if (!owner) return;
          try {
            const positions = await indexer!.positions(owner);
            const histories = await Promise.all(positions.portfolios.map(async p => ({ ...await indexer!.fills(p.portfolio), portfolio: p.portfolio })));
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

  const interest = stats ? Object.values(stats.open_interest)[0] : undefined;
  return <>
    <section aria-labelledby="indexer-stats-heading" className="fills">
      <h2 id="indexer-stats-heading">{PERP.name} stats</h2>
      {!indexer ? <p role="status">{UNREACHABLE}</p> : <>
        {statsError && <p role="status">{statsError}</p>}
        {stats && <dl>
          <dt>Open interest</dt><dd>{interest ? `long ${number(fixed(interest.long_q))} / short ${number(fixed(interest.short_q))} × ${PERP.unit}` : "—"}</dd>
          {season && <><dt>Volume</dt><dd>{usdAtoms(season.volume_usd)}</dd></>}
          {season && <><dt>Fills</dt><dd>{season.trades.toLocaleString("en-US")}</dd></>}
          <dt>As of</dt><dd>{stats.as_of === null ? "Indexing" : new Date(stats.as_of * 1000).toLocaleString()}</dd>
        </dl>}
      </>}
    </section>
    <section aria-labelledby="indexer-history-heading" className="fills">
      <h2 id="indexer-history-heading">Your fills</h2>
      {!indexer ? <p role="status">{UNREACHABLE}</p> : !owner ? <p>Connect a wallet to see your fills.</p> : <>
        {fillsError && <p role="status">{fillsError}</p>}
        <div className="table-scroll"><table>
          <thead><tr><th scope="col">Time</th><th scope="col">Side</th><th scope="col">Size ({PERP.unitShort})</th><th scope="col">Price</th><th scope="col">Status</th><th scope="col">Tx</th></tr></thead>
          <tbody>{fills?.map(fill => <tr key={`${fill.portfolio}:${fill.signature}:${fill.ix_index}:${fill.leg}`}>
            <td>{fill.block_time === null ? "—" : new Date(fill.block_time * 1000).toLocaleString()}</td>
            <td>{fill.side === "buy" ? "long" : "short"}</td><td>{number(Math.abs(fixed(fill.size_q)))}</td>
            <td>{fill.mark_price === null || !Number.isSafeInteger(fill.mark_price) ? "—" : perpPrice(fill.mark_price / 1e6)}</td><td>{fill.status}</td>
            <td><a href={`https://explorer.solana.com/tx/${encodeURIComponent(fill.signature)}?cluster=devnet`} target="_blank" rel="noreferrer">{fill.signature.slice(0, 7)}…{fill.signature.slice(-4)}</a></td>
          </tr>)}</tbody>
        </table></div>
        {moreHistory && <p>Older fills are on the explorer.</p>}
        {fills !== null && fills.length === 0 && !fillsError && <p>No fills yet.</p>}
      </>}
    </section>
  </>;
}
