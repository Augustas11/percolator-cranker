// The Arena leaderboard: the indexer's GET /leaderboard, sorted by PnL, with each wallet's open position.
import { useEffect, useState } from "react";
import type { IndexerClient, Leaderboard as Board } from "../indexer/client";
import { PERP } from "../market/types";
import { Link } from "../site/router";
import { houseName } from "./house";
import "../site/arena.css";

const usdFormatter = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const atomsToNumber = (atoms: string) => {
  try { return Number(BigInt(atoms)) / 1e6; } catch { return NaN; }
};
export const usdAtoms = (atoms: string) => {
  const n = atomsToNumber(atoms);
  // Round to cents first so a sub-cent loss reads $0.00, never -$0.00.
  return Number.isFinite(n) ? usdFormatter.format(Math.round(n * 100) === 0 ? 0 : n) : "—";
};
const short = (wallet: string) => wallet.length > 8 ? `${wallet.slice(0, 4)}…${wallet.slice(-4)}` : wallet;
const explorer = (wallet: string) => `https://explorer.solana.com/address/${encodeURIComponent(wallet)}?cluster=devnet`;
const pctOf = (pnl: string, deposits: string) => {
  const p = atomsToNumber(pnl), d = atomsToNumber(deposits);
  if (!Number.isFinite(p) || !Number.isFinite(d) || d <= 0) return "—";
  const v = p / d * 100;
  const text = Math.abs(v).toFixed(1);
  return `${text !== "0.0" && v > 0 ? "+" : text !== "0.0" && v < 0 ? "−" : ""}${text}%`;
};

type Open = { side: "long" | "short"; units: number };
const MAX_POSITION_LOOKUPS = 25;

export function Unreachable({ error }: { error?: string }) {
  return <p className="arena-state" role="status">The leaderboard is unreachable right now{error ? ` (${error})` : ""}. It reads the Drip indexer; try again in a minute.</p>;
}

export function NoFills() {
  return <div className="arena-state">
    <p>No fills yet. The first trade puts a wallet on the board.</p>
    <div className="arena-actions"><Link className="button-link" to="/trade">Trade</Link><Link className="button-link outline" to="/agents">Connect an agent</Link></div>
  </div>;
}

export default function Leaderboard({ indexer, owner = null, limit, onCount }: { indexer: IndexerClient | null; owner?: string | null; limit?: number; onCount?: (traders: number | null) => void }) {
  const [board, setBoard] = useState<Board | null>(null);
  const [open, setOpen] = useState<Map<string, Open | null>>(new Map());
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    setBoard(null);
    setError("");
    if (!indexer) return;
    const refresh = async () => {
      try {
        const result = await indexer.leaderboard();
        if (!active) return;
        setBoard(result);
        setError("");
        const holders = result.entries.filter(e => e.open_positions > 0).slice(0, MAX_POSITION_LOOKUPS);
        const found = await Promise.all(holders.map(async e => {
          try {
            const pos = await indexer.positions(e.wallet);
            const legs = pos.portfolios.flatMap(p => p.positions).filter(l => l.asset_index === 0);
            const q = legs.reduce((n, l) => n + Math.abs(atomsToNumber(l.basis_pos_q)) * (l.side === "long" ? 1 : -1), 0);
            return [e.wallet, q === 0 || !Number.isFinite(q) ? null : { side: q > 0 ? "long" : "short", units: Math.abs(q) }] as const;
          } catch { return [e.wallet, null] as const; }
        }));
        if (active) setOpen(new Map(found as [string, Open | null][]));
      } catch (e) {
        if (active) setError(e instanceof Error ? e.message : "indexer request failed");
      }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 15000);
    return () => { active = false; clearInterval(timer); };
  }, [indexer]);

  // A wallet is on the board after its first fill: rows with no trades are neither ranked nor shown.
  const traded = board ? board.entries.filter(e => e.trades > 0) : null;
  useEffect(() => { onCount?.(traded ? traded.length : null); }, [traded?.length]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!indexer) return <Unreachable />;
  if (error && !board) return <Unreachable error={error} />;
  if (!board) return <p className="arena-state" aria-busy="true">Loading the leaderboard.</p>;
  if (traded!.length === 0) return <NoFills />;
  const rows = [...traded!].sort((a, b) => atomsToNumber(b.pnl) - atomsToNumber(a.pnl)).slice(0, limit);
  return <div className="arena-board">
    <div className="table-scroll"><table>
      <caption className="sr-only">{PERP.name} leaderboard, highest PnL first</caption>
      <thead><tr><th scope="col">Rank</th><th scope="col">Wallet</th><th scope="col">PnL</th><th scope="col">PnL / deposits</th><th scope="col">Trades</th><th scope="col">Open position</th></tr></thead>
      <tbody>{rows.map((entry, i) => {
        const house = houseName(entry.wallet);
        const pos = open.get(entry.wallet);
        const pnl = atomsToNumber(entry.pnl);
        return <tr key={entry.wallet}>
          <td>{i + 1}</td>
          <td><a href={explorer(entry.wallet)} target="_blank" rel="noreferrer">{short(entry.wallet)}</a>
            {house && <span className="house-tag" title="Drip starter agent, tagged as a house agent">house · {house}</span>}
            {entry.wallet === owner && <strong> (you)</strong>}</td>
          <td className={pnl > 0 ? "pnl-up" : pnl < 0 ? "pnl-down" : undefined}>{usdAtoms(entry.pnl)}</td>
          <td>{pctOf(entry.pnl, entry.net_deposits)}</td>
          <td>{entry.trades}</td>
          <td>{pos ? `${pos.side} ${pos.units.toLocaleString("en-US", { maximumFractionDigits: 2 })} ${PERP.unitShort}` : entry.open_positions > 0 ? "open" : "—"}</td>
        </tr>;
      })}</tbody>
    </table></div>
    {error && <p className="note" role="status">Could not refresh ({error}). Showing the last board.</p>}
  </div>;
}
