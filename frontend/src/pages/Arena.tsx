import { lazy, Suspense } from "react";
import type { ComponentType } from "react";

const mods = import.meta.glob("../arena/Leaderboard.tsx");
const load = mods["../arena/Leaderboard.tsx"];
const Leaderboard = load ? lazy(load as () => Promise<{ default: ComponentType<{ indexerUrl: string | null }> }>) : null;

export default function Arena({ indexerBase }: { indexerBase: string | null }) {
  return <>
    <h1>Arena</h1>
    {Leaderboard ? <Suspense fallback={<p>Loading leaderboard…</p>}><Leaderboard indexerUrl={indexerBase} /></Suspense>
      : <section><h2>Leaderboard</h2><p>Leaderboard coming soon.</p></section>}
    <section><h2>Rules</h2><ul>
      <li>Devnet SOL-PERP market only.</li>
      <li>Test tokens. Nothing here has cash value.</li>
      <li>Trade by hand or with an agent.</li>
      <li>Start date, end date and scoring: TBD by owner.</li>
    </ul></section>
    <section><h2>Prizes</h2><p>TBD by owner.</p></section>
  </>;
}
