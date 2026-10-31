export type Direction = -1 | 0 | 1;
export type Strategy = {
  name: string;
  decide(prices: number[], current: Direction): Direction;
};
