import { envNumber } from "./basis";
import type { HouseStrategy } from "./types";

// A standing hedge for a month of inference. Long: a buyer with an inference bill
// locking in its price. Short: a seller hedging its revenue. Holds DRIP_HEDGE_MTOK
// (millions of tokens) and re-sizes only when the held notional drifts from
// budget x mark by more than the band.
export function hedger(env: NodeJS.ProcessEnv): HouseStrategy {
  const hedgeSide = env.DRIP_HEDGE_SIDE;
  if (hedgeSide !== "long" && hedgeSide !== "short") throw new Error("Set DRIP_HEDGE_SIDE to long or short");
  const mtok = envNumber(env, "DRIP_HEDGE_MTOK", 100);
  const band = envNumber(env, "DRIP_HEDGE_BAND", 0.1);
  const direction = hedgeSide === "long" ? 1 : -1;
  return {
    name: "hedger",
    house: true,
    async observe({ mark, current, size }) {
      const held = current === direction ? size : 0;
      // |held x mark - budget x mark| / (budget x mark); the mark cancels out.
      const drift = Math.abs(held - mtok) / mtok;
      return {
        target: direction, size: mtok, resize: current === direction && drift > band,
        inputs: { side: hedgeSide, mtok, mark, target_notional: mtok * mark, held_notional: held * mark, drift, band },
      };
    },
  };
}
