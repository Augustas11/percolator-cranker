import { trend } from "./trend";
import { meanrev } from "./meanrev";
import { breakout } from "./breakout";
import type { Strategy } from "./types";

export const strategies: Record<string, Strategy> = { trend, meanrev, breakout };
