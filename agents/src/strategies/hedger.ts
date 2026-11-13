import { envNumber, MAX_FILL } from "./basis";
import type { HouseStrategy, Plan } from "./types";

// COMPUTE-PERP initial margin (perps_markets: max_leverage 2), and the most of
// equity a hedge may tie up as initial margin.
const INITIAL_MARGIN = 0.5, MAX_MARGIN_USE = 0.8;

// A standing hedge for a month of inference. Long: a buyer with an inference bill
// locking in its price. Short: a seller hedging its revenue. Holds DRIP_HEDGE_MTOK
// (millions of tokens) and re-sizes only when the held notional drifts from
// budget x mark by more than the band, one matcher fill per tick: it adds when
// short of the budget and trades the excess back when over.
export function hedger(env: NodeJS.ProcessEnv): HouseStrategy {
  const hedgeSide = env.DRIP_HEDGE_SIDE;
  if (hedgeSide !== "long" && hedgeSide !== "short") throw new Error("Set DRIP_HEDGE_SIDE to long or short");
  const mtok = envNumber(env, "DRIP_HEDGE_MTOK", 100);
  const band = envNumber(env, "DRIP_HEDGE_BAND", 0.1);
  const direction = hedgeSide === "long" ? 1 : -1;
  return {
    name: "hedger",
    house: true,
    async observe({ mark, current, size, equity }) {
      const held = current === direction ? size : 0;
      // |held x mark - budget x mark| / (budget x mark); the mark cancels out.
      const drift = Math.abs(held - mtok) / mtok;
      const inputs = { side: hedgeSide, mtok, mark, equity, target_notional: mtok * mark, held_notional: held * mark, drift, band };
      const fill = (want: number) => Math.min(MAX_FILL, want);
      const grow = (fields: Omit<Plan, "inputs">, reason: string): Plan => {
        // Initial margin for the position after this fill; anything not finite means do not grow.
        const margin = ((fields.add ? held : 0) + (fields.size ?? 0)) * mark * INITIAL_MARGIN;
        if (!(margin <= MAX_MARGIN_USE * equity)) {
          // Never keep the wrong side because the right one does not fit: close it.
          return { target: current === direction ? current : 0, inputs: { ...inputs, reason: "margin_limit", margin_needed: margin } };
        }
        return { ...fields, inputs: { ...inputs, reason } };
      };
      if (current !== direction) return grow({ target: direction, size: fill(mtok) }, current === 0 ? "open" : "wrong_side");
      if (!(drift > band)) return { target: direction, inputs: { ...inputs, reason: "within_band" } };
      if (held < mtok) return grow({ target: direction, size: fill(mtok - held), add: true }, "build");
      return { target: direction, size: fill(held - mtok), reduce: true, inputs: { ...inputs, reason: "over_target" } };
    },
  };
}
