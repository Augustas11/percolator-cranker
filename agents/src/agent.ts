import type { Mcp } from "./mcp";
import type { Direction, HouseStrategy, Strategy } from "./strategies/types";

export interface Position {
  wallet: string;
  sol_balance: number;
  wallet_usdc: number;
  portfolio: string | null;
  capital: number;
  equity: number;
  mark_price: number;
  position: { side: "long" | "short"; size: number } | null;
}

export interface Config {
  intervalS: number;
  depositUsdc: number;
  sizeSol: number;
  maxTradesPerHour: number;
  minSol: number;
  signal: AbortSignal;
  log?: (event: Record<string, unknown>) => void;
}

export interface Clock {
  now(): number;
  sleep(ms: number, signal: AbortSignal): Promise<void>;
}

export const clock: Clock = {
  now: () => Date.now(),
  sleep: (ms, signal) => new Promise(resolve => {
    if (signal.aborted) return resolve();
    const finish = () => {
      clearTimeout(timer);
      signal.removeEventListener("abort", finish);
      resolve();
    };
    const timer = setTimeout(finish, ms);
    signal.addEventListener("abort", finish, { once: true });
  }),
};

export function config(env: NodeJS.ProcessEnv, signal: AbortSignal): Config {
  const number = (key: string, fallback: number, allowZero = false) => {
    const value = Number(env[key] ?? fallback);
    if (!Number.isFinite(value) || (allowZero ? value < 0 : value <= 0)) throw new Error(`Invalid ${key}`);
    return value;
  };
  const maxTradesPerHour = number("MAX_TRADES_PER_HOUR", 6, true);
  if (!Number.isInteger(maxTradesPerHour)) throw new Error("MAX_TRADES_PER_HOUR must be an integer");
  return {
    intervalS: number("INTERVAL_S", 60), depositUsdc: number("DEPOSIT_USDC", 500),
    sizeSol: number("SIZE_SOL", 0.5), maxTradesPerHour, minSol: number("MIN_SOL", 0.005, true), signal,
  };
}

export async function runAgent(strategy: Strategy | HouseStrategy, cfg: Config, mcp: Mcp, time: Clock = clock): Promise<void> {
  const log = (event: string, fields: Record<string, unknown> = {}) => {
    const line = { agent: strategy.name, event, ...fields };
    if (cfg.log) cfg.log(line);
    else process.stdout.write(JSON.stringify(line) + "\n");
  };
  const history: number[] = [], trades: number[] = [];
  const interval = cfg.intervalS * 1000;
  let delay = interval, bootstrapped = false, lowSolLogged = false;
  let closing: Promise<void> | undefined;
  const close = () => closing ??= mcp.close();
  const shutdown = () => { void close().catch(() => {}); };
  cfg.signal.addEventListener("abort", shutdown, { once: true });
  log("start", "house" in strategy ? { house: true } : {});
  try {
    while (!cfg.signal.aborted) {
      try {
        if (!bootstrapped) {
          const initial = await mcp.call("get_position") as Position;
          if (cfg.signal.aborted) break;
          // Fund a missing portfolio, and one with no capital (half-done deposit, liquidation).
          if (initial.portfolio === null || !(initial.capital > 0)) {
            if (initial.wallet_usdc < cfg.depositUsdc) await mcp.call("claim_test_funds");
            if (cfg.signal.aborted) break;
            await mcp.call("deposit", { amount_usdc: cfg.depositUsdc });
          }
          bootstrapped = true;
          log("bootstrap");
        }
        if (cfg.signal.aborted) break;
        const position = await mcp.call("get_position") as Position;
        if (cfg.signal.aborted) break;
        if (!Number.isFinite(position.mark_price) || position.mark_price <= 0 || !Number.isFinite(position.sol_balance)) {
          throw new Error("Invalid market price or SOL balance");
        }
        history.push(position.mark_price);
        if (history.length > 200) history.shift();
        const current: Direction = position.position ? position.position.side === "long" ? 1 : -1 : 0;
        const held = position.position ? position.position.size : 0;
        if (position.position && !(Number.isFinite(held) && held > 0)) throw new Error("Invalid position size");
        let target: Direction, size = cfg.sizeSol, add = false, reduce = false;
        if ("observe" in strategy) {
          if (!Number.isFinite(position.equity)) throw new Error("Invalid equity");
          const plan = await strategy.observe({ mark: position.mark_price, current, size: held, equity: position.equity, now: time.now() });
          if (cfg.signal.aborted) break;
          if (plan.size !== undefined && !(Number.isFinite(plan.size) && plan.size > 0)) throw new Error("Invalid plan size");
          target = plan.target;
          size = plan.size ?? cfg.sizeSol;
          add = plan.add === true && target !== 0 && target === current;
          reduce = !add && plan.reduce === true && target !== 0 && target === current;
          if (reduce && !(size < held)) throw new Error("Reduce must be smaller than the position");
          log("tick", { house: true, ...plan.inputs, mark: position.mark_price, target, current, size, add, reduce });
        } else {
          target = strategy.decide(history, current);
          log("tick", { mark: position.mark_price, target, current });
        }
        if (position.sol_balance < cfg.minSol) {
          if (!lowSolLogged) log("low_sol", { sol_balance: position.sol_balance });
          lowSolLogged = true;
        } else {
          lowSolLogged = false;
          while (trades.length && trades[0] <= time.now() - 3_600_000) trades.shift();
          // add / reduce: one order on top of the open position (the engine nets it).
          const adjust = add || reduce;
          const cost = adjust ? 1 : Number(current !== 0) + Number(target !== 0);
          if (target !== current || adjust) {
            if (trades.length + cost > cfg.maxTradesPerHour) {
              log("skip_budget", { target, current });
            } else {
              // Count attempts too: a timed-out request may already have executed.
              if (current !== 0 && !adjust) {
                trades.push(time.now());
                await mcp.call("close_position");
                log("trade", { action: "close" });
              }
              if (target !== 0 && !cfg.signal.aborted) {
                const side = (reduce ? -target : target) === 1 ? "long" : "short";
                trades.push(time.now());
                await mcp.call("open_position", { side, size });
                log("trade", { action: reduce ? "reduce" : "open", side, size });
              }
            }
          }
        }
        delay = interval;
      } catch (error) {
        if (cfg.signal.aborted) break;
        log("error", { message: error instanceof Error ? error.message : "Unknown MCP error" });
        delay = Math.min(delay * 2, 600_000);
      }
      await time.sleep(delay, cfg.signal);
    }
  } finally {
    cfg.signal.removeEventListener("abort", shutdown);
    await close();
  }
}
