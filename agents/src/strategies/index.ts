import { trend } from "./trend";
import { meanrev } from "./meanrev";
import { breakout } from "./breakout";
import { basis } from "./basis";
import { hedger } from "./hedger";
import type { HouseStrategy, Strategy } from "./types";

export const strategies: Record<string, Strategy> = { trend, meanrev, breakout };

// Built from env at startup, so bad settings fail before any wallet connects.
export const houseStrategies: Record<string, (env: NodeJS.ProcessEnv) => HouseStrategy> = {
  basis: env => basis(env),
  hedger,
};
