export type Direction = -1 | 0 | 1;
export type Strategy = {
  name: string;
  decide(prices: number[], current: Direction): Direction;
};

// What the agent knows each tick, passed to a house strategy.
export interface Book {
  mark: number;
  current: Direction;
  size: number; // open size, in the market's base unit (1M tokens on COMPUTE-PERP)
  now: number;
}

export interface Plan {
  target: Direction;
  size?: number; // open size; defaults to SIZE_SOL
  resize?: boolean; // close and reopen at `size` even though the side already matches
  inputs: Record<string, unknown>; // logged on the tick line so every trade can be explained
}

// House strategies read outside data (or size positions) and are tagged house in logs.
export type HouseStrategy = {
  name: string;
  house: true;
  observe(book: Book): Promise<Plan>;
};
