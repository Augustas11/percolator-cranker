// Devnet agent-arena leaderboard: reads the indexer's GET /leaderboard. Self-contained so the site can mount it anywhere.
import { useEffect, useMemo, useState } from "react";
import { devnetConfig } from "../chain/config";
import { HttpIndexerClient } from "../indexer/client";
import type { Leaderboard as Board } from "../indexer/client";

const usdFormatter = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const usd = (atoms: string) => {
  try { return usdFormatter.format(Number(BigInt(atoms)) / 1e6); } catch { return "—"; }
};

export default function Leaderboard({ indexerUrl, owner = null }: { indexerUrl: string | null; owner?: string | null }) {
  const indexer = useMemo(() => indexerUrl ? new HttpIndexerClient(indexerUrl,
    { market: devnetConfig.market.toBase58(), program: devnetConfig.programId.toBase58() }) : null, [indexerUrl]);
  const [board, setBoard] = useState<Board | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    setBoard(null);
    setError("");
    if (!indexer) return;
    const refresh = () => indexer.leaderboard().then(
      result => { if (active) { setBoard(result); setError(""); } },
      e => { if (active) setError(e instanceof Error ? e.message : "indexer request failed"); });
    void refresh();
    const timer = setInterval(() => void refresh(), 15000);
    return () => { active = false; clearInterval(timer); };
  }, [indexer]);

  return <section aria-labelledby="arena-leaderboard-heading" className="fills">
    <h2 id="arena-leaderboard-heading">Leaderboard (devnet, test tokens)</h2>
    {!indexer ? <p>Indexer not configured</p> : <>
      {board?.note && <p>{board.note}</p>}
      {error && <p role="status">{error}</p>}
      <div className="table-scroll"><table>
        <thead><tr><th scope="col">Rank</th><th scope="col">Wallet</th><th scope="col">PnL (USD)</th><th scope="col">Trades</th><th scope="col">Open positions</th></tr></thead>
        <tbody>{board?.entries.map(entry => <tr key={entry.wallet}>
          <td>{entry.rank}</td>
          <td><a href={`https://explorer.solana.com/address/${encodeURIComponent(entry.wallet)}?cluster=devnet`} target="_blank" rel="noreferrer">{entry.wallet.slice(0, 4)}…{entry.wallet.slice(-4)}</a>{entry.wallet === owner && <strong> (you)</strong>}</td>
          <td>{usd(entry.pnl)}</td><td>{entry.trades}</td><td>{entry.open_positions}</td>
        </tr>)}</tbody>
      </table></div>
      {board && board.entries.length === 0 && !error && <p>No traders yet</p>}
    </>}
  </section>;
}
