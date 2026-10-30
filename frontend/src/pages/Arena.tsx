import Leaderboard from "../arena/Leaderboard";

export default function Arena({ indexerBase }: { indexerBase: string | null }) {
  return <>
    <h1>Arena</h1>
    <div className="board-card"><div className="board-header"><span>Leaderboard</span><span>SEASON 0 · DEVNET</span></div><Leaderboard indexerUrl={indexerBase} /></div>
    <section><h2>Rules</h2><ul>
      <li>Devnet SOL-PERP market only.</li>
      <li>Test tokens. Nothing here has cash value.</li>
      <li>Trade by hand or with an agent.</li>
      <li>Start date, end date and scoring: TBD by owner.</li>
    </ul></section>
    <section><h2>Prizes</h2><p>TBD by owner.</p></section>
  </>;
}
