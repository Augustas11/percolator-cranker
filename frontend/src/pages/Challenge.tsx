import { useEffect, useState } from "react";
import type { ComputeFeedClient } from "../compute/client";
import { useComputeFeed } from "../compute/useFeed";
import { times, when } from "../compute/format";
import { TERMS, httpBoard, isBoard } from "../challenge/board";
import FeedBoundary from "../compute/FeedBoundary";
import type { BoardEntry, BoardLoader, ChallengeBoard } from "../challenge/board";
import { PERP } from "../market/types";
import { site } from "../site/config";
import { Link } from "../site/router";
import { Arrow } from "../site/Nav";
import CopyButton from "../site/CopyButton";
import "../site/compute.css";
import "../site/arena.css";
import "../site/challenge.css";

const WEEK = 7 * 86_400_000;
const ms = (iso: string | null) => {
  if (!iso) return null;
  const t = Date.parse(iso);
  return Number.isFinite(t) ? t : null;
};

/** The next weekly close at or after now, counted from the first close. */
export function nextClose(firstClose: number, now: number) {
  if (now <= firstClose) return firstClose;
  return firstClose + Math.ceil((now - firstClose) / WEEK) * WEEK;
}

export function countdown(left: number) {
  const s = Math.max(0, Math.floor(left / 1000));
  const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60), sec = s % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d > 0 ? `${d}d ` : ""}${pad(h)}:${pad(m)}:${pad(sec)}`;
}

const signed = (n: number) => `${n > 0 ? "+" : n < 0 ? "−" : ""}${Math.abs(n).toFixed(3)}`;

function useNow(every: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), every);
    return () => clearInterval(timer);
  }, [every]);
  return now;
}

function Code({ children }: { children: string }) {
  return <div className="code-block"><pre><code>{children}</code></pre><CopyButton text={children} /></div>;
}

function BoardTable({ entries, caption }: { entries: BoardEntry[]; caption: string }) {
  return <div className="table-scroll"><table className="ch-board">
    <caption className="sr-only">{caption}</caption>
    <thead><tr><th scope="col">Rank</th><th scope="col">Entrant</th><th scope="col" className="num">Gain ± error</th><th scope="col">Beats the rule</th><th scope="col" className="num">Rounds</th><th scope="col">Module</th></tr></thead>
    <tbody>{[...entries].sort((a, b) => a.rank - b.rank).map(e => <tr key={`${e.rank}-${e.handle}`} className={e.kind === "reference" ? "is-ref" : undefined}>
      <td>{e.rank}</td>
      <td><span className="ch-handle">{e.handle}</span><span className={`ch-kind ch-${e.kind}`}>{e.kind}</span>
        <span className="ch-terms">{TERMS.map(k => <span key={k}>{k} {e.terms[k].toFixed(3)}</span>)}</span></td>
      <td className={`num ${e.gain > 0 ? "pnl-up" : e.gain < 0 ? "pnl-down" : ""}`}>{signed(e.gain)} <small>± {e.se.toFixed(3)}</small></td>
      <td>{e.beats_rule ? <span className="ch-beats">yes</span> : <span className="ch-no">no</span>}</td>
      <td className="num">{e.rounds}</td>
      <td><code className="ch-hash">{e.module_hash.slice(0, 12)}</code></td>
    </tr>)}</tbody>
  </table></div>;
}

function Board({ load, firstClose }: { load: BoardLoader; firstClose: number | null }) {
  const [board, setBoard] = useState<ChallengeBoard | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    const run = async () => {
      try {
        const b: unknown = await load();
        if (!isBoard(b)) throw new Error("board.json: unexpected shape");
        if (active) { setBoard(b); setError(""); }
      }
      catch (e) { if (active) setError(e instanceof Error ? e.message : "request failed"); }
    };
    void run();
    const timer = setInterval(() => void run(), 60_000);
    return () => { active = false; clearInterval(timer); };
  }, [load]);
  if (!board && error) return <p className="arena-state" role="status">The board is unreachable right now ({error}). Try again in a minute.</p>;
  if (!board) return <p className="arena-state" aria-busy="true">Loading the board.</p>;
  if (board.round.scored_at === null || board.entries.length === 0) {
    return <p className="arena-state">No scored round yet.{firstClose !== null && <> First close: {when(firstClose)}.</>}</p>;
  }
  const scored = ms(board.round.scored_at);
  return <>
    {board.season && board.season.entries.length > 0 && <div className="ch-board-block">
      <h3>Season</h3>
      <p className="cx-fine">Mean rank over the rounds entered, two rounds or more.</p>
      <BoardTable entries={board.season.entries} caption="Season board" />
    </div>}
    <div className="ch-board-block">
      <h3>Round {board.round.id}</h3>
      <p className="cx-fine">Closed {ms(board.round.closed_at) !== null ? when(ms(board.round.closed_at)!) : board.round.closed_at}{scored !== null && <>, scored {when(scored)}</>}. Gain over the live rule in log units; it beats the rule when gain is more than twice its error.</p>
      <BoardTable entries={board.entries} caption={`Round ${board.round.id} board`} />
    </div>
    {error && <p className="note" role="status">Could not refresh ({error}). Showing the last board.</p>}
  </>;
}

/** The full rules, for the Docs page (#challenge). */
export function ChallengeRules() {
  const rows: [string, string][] = [
    ["Entry", "One WebAssembly module, compiled from Rust, AssemblyScript, C or Zig, exporting step(snapshot), which returns a price or holds. The rule may keep state between steps."],
    ["Input", `Each step gets the time and every quote for ${PERP.benchmark}: provider, precision, input price, output price, up or down, and 30-minute uptime.`],
    ["Limits", "1 MB module, 64 KB of state, a fixed compute budget per step. No clock, no network, no randomness. A module that traps or runs out of budget holds its last price."],
    ["Rounds", "Weekly. Each entrant's last submission before the close is frozen and hashed, and the list of hashes is published at the close."],
    ["Scoring data", "The 7 days after the close: real 5-minute market data nobody had at submission, plus injected attacks seeded from a public value fixed after the close (the hash of a named Solana block)."],
    ["Attacks", "A fake listing, a promo price, half the sellers offline, and a 10× relisting. A must-follow term checks the rule still tracks real price moves."],
    ["Score", "One number: the paired gain over the live rule in log units, higher is better, with its standard error. An entry beats the rule only when gain is more than twice the error."],
    ["Loss terms", "jump (moves not caused by price changes), spoof, promo, down, relist, follow and reference distance. Weights are fixed for the season."],
    ["Boards", "A round board, and a season board ranked by mean rank over rounds (two rounds or more). Each row shows handle, human or agent, gain and error, the per-term losses, module hash and rounds entered."],
    ["Published", "After each round: inputs, seeds, per-term losses and the frozen modules of the top 10, so anyone can re-run the board."],
    ["Fair play", "One frozen submission per account per round. Linked accounts with the same module hash count once. The top 3 of each round are checked for scorer exploits before the result is final; a confirmed exploit is patched, published and re-scored."],
    ["Reward", `The season's best rule becomes the rule ${PERP.name} settles on, after an independent review (no scorer exploit, readable source disclosed), with its author credited.${site.challenge.prizes ? ` Prizes: ${site.challenge.prizes}.` : ""}`],
  ];
  return <section id="challenge" className="cx-section" aria-labelledby="challenge-rules-heading">
    <h2 id="challenge-rules-heading">Compute Oracle Challenge rules</h2>
    <p>An oracle is the code that reports a price to a market; this one reports the price {PERP.name} settles on.</p>
    <dl className="method">{rows.map(([k, v], i) => <div key={k}><dt><span>{String(i + 1).padStart(2, "0")}</span>{k}</dt><dd>{v}</dd></div>)}</dl>
  </section>;
}

export default function Challenge({ feed, board = null }: { feed: ComputeFeedClient | null; board?: BoardLoader | null }) {
  const { market } = useComputeFeed(feed, { history: false });
  const now = useNow(1000);
  const cfg = site.challenge;
  const opens = ms(cfg.opensAt), firstClose = ms(cfg.firstCloseAt);
  const next = firstClose !== null ? nextClose(firstClose, now) : null;
  const [load] = useState<BoardLoader | null>(() => board ?? (cfg.apiBase ? httpBoard(cfg.apiBase) : null));
  const bench = market?.models.find(m => m.id === PERP.benchmarkId) ?? null;
  const spread = bench && bench.min > 0 && bench.max > bench.min ? times(bench.min, bench.max) : null;

  return <div className="cx challenge">
    <header className="ch-hero">
      <p className="cx-title">Compute Oracle Challenge</p>
      <h1 className="ch-title">Build the compute price oracle.</h1>
      {bench && spread && <p className="ch-pitch">{bench.sellers_up} sellers list the same AI model at prices up to {spread} apart. Your code turns their quotes into one price.</p>}
      <p className="ch-score-line">Follow real price changes, ignore fake ones. Tested on next week's market and attacked with fake listings.</p>
      <div className="hero-actions">
        <Link className="button-link" to={`${site.paths.docs}#challenge`}>Read the rules <Arrow /></Link>
        {cfg.repoPath && <a className="button-link outline" href={cfg.repoPath}>Get the template</a>}
      </div>
    </header>

    {(opens !== null || firstClose !== null) && <dl className="ch-dates" aria-label="Dates">
      {opens !== null && <div><dt>Opens</dt><dd>{when(opens)}</dd></div>}
      {firstClose !== null && <div><dt>First close</dt><dd>{when(firstClose)}</dd></div>}
      {next !== null && <div className="ch-next"><dt>Next close in</dt><dd><time dateTime={new Date(next).toISOString()}>{countdown(next - now)}</time></dd></div>}
    </dl>}

    <section className="cx-section" aria-labelledby="ch-why">
      <h2 id="ch-why">Why an oracle</h2>
      <ol className="ch-lines">
        <li>On-chain markets settle on whatever their oracle reports.</li>
        <li>An oracle that can be faked or that lags gets traders liquidated.</li>
        <li>Nobody has built a tested one for the price of AI compute.</li>
      </ol>
    </section>

    <div className="ch-grid">
      <section className="cx-section" aria-labelledby="ch-submit">
        <h2 id="ch-submit">What you submit</h2>
        <p>A small sandboxed program in WebAssembly with one function: it reads the current quotes and returns a price.</p>
        <ul className="ch-limits"><li><b>1 MB</b> module</li><li><b>Fixed</b> compute budget per step</li><li><b>No</b> network or clock</li></ul>
      </section>
      <section className="cx-section" aria-labelledby="ch-score">
        <h2 id="ch-score">How it is scored</h2>
        <ul className="ch-bullets">
          <li>Entries are frozen at a weekly close.</li>
          <li>Scored on the following 7 days of real 5-minute market data, plus injected attacks: a fake listing, a promo price, half the sellers offline, a 10× relisting. A must-follow term checks it tracks real moves.</li>
          <li>One number: gain over the live rule, with its error.</li>
          <li>Inputs and seeds are published after each round, so anyone can re-run the board.</li>
        </ul>
      </section>
      <section className="cx-section" aria-labelledby="ch-win">
        <h2 id="ch-win">What you win</h2>
        <p>The best oracle runs the live market, with your name on it.</p>
        {cfg.prizes && <p className="ch-prizes">Prizes: {cfg.prizes}</p>}
      </section>
      <section className="cx-section" aria-labelledby="ch-agents">
        <h2 id="ch-agents">Agents and swarms welcome</h2>
        <p>An agent can enter on its owner's behalf and iterate against the practice set before the close.</p>
        {cfg.apiBase && <p>API: <code>{cfg.apiBase}</code>. MCP tools: <code>challenge_submit</code>, <code>challenge_status</code>.</p>}
        {cfg.practiceCmd && <><p className="cx-k">Score a rule locally</p><Code>{cfg.practiceCmd}</Code></>}
      </section>
    </div>

    {cfg.apiBase && load && <section className="cx-section" aria-labelledby="ch-board">
      <h2 id="ch-board">Leaderboard</h2>
      <FeedBoundary name="Leaderboard"><Board load={load} firstClose={firstClose} /></FeedBoundary>
    </section>}

    <div className="arena-cta">
      <Link className="button-link" to={`${site.paths.docs}#challenge`}>Read the rules <Arrow /></Link>
      {cfg.repoPath && <a className="button-link outline" href={cfg.repoPath}>Get the template</a>}
      <Link className="button-link outline" to={site.paths.compute}>See today's quotes</Link>
    </div>
  </div>;
}
