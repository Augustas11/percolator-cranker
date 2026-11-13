// Plain sentences for the common ways a devnet send fails. The raw text stays on the error as `details`, for a
// "Details" disclosure. Codes: matcher/program/src/lib.rs (PAUSED 7 … INVENTORY_CAP 12), wrapper errors as named
// in cranker/src/submit.rs (0x13 EngineStale, 0x1B OracleStale).

export class ChainError extends Error {
  constructor(message: string, public details: string) { super(message); }
}

const MATCHER: Record<number, string> = {
  7: "The liquidity account has paused fills. Try again later.",
  9: "The liquidity account's reference price is too old, so it refused the fill. Try again in a minute.",
  10: "The mark is too far from the liquidity account's reference price, so it refused the fill. Try again after the next price update.",
  11: "The order is larger than the liquidity account fills at once. Split it into smaller trades.",
  12: "The liquidity account is at its inventory limit on this side. Trade a smaller size.",
};

/** The custom error code a given program failed with, from the transaction logs. */
function failedWith(logs: string[], program: string) {
  for (const line of logs) {
    const m = line.match(/^Program (\S+) failed: custom program error: 0x([0-9a-f]+)/i);
    if (m && m[1] === program) return parseInt(m[2], 16);
  }
  return null;
}

/** A plain sentence for a failed send, or null when the failure is not one of the common cases. */
export function plainChainError(raw: string, logs: string[], programs: { engine?: string; matcher?: string } = {}): string | null {
  const text = `${raw}\n${logs.join("\n")}`;
  if (/user rejected the request|\b4001\b|transaction cancell?ed/i.test(text)) return "You declined the transaction in your wallet.";
  // SOL for fees only. The token program's "Error: insufficient funds" is about the token balance, below.
  if (/InsufficientFundsForFee|no record of a prior credit|insufficient lamports/i.test(text)) return "Your wallet does not have enough devnet SOL to pay the network fee.";
  if (/Program log: Error: insufficient funds/i.test(text)) return "You do not have that much test USDC in your wallet.";
  if (/blockhash not found|BlockhashNotFound|block height exceeded|TransactionExpiredBlockheightExceeded/i.test(text)) return "The transaction expired before it landed. Try again.";
  const matcher = programs.matcher ? failedWith(logs, programs.matcher) : null;
  if (matcher !== null && MATCHER[matcher]) return MATCHER[matcher];
  const engine = programs.engine ? failedWith(logs, programs.engine) : null;
  if (engine === 0x13 || engine === 0x1b || /EngineStale|OracleStale/.test(text)) return "The market price is stale right now, so the trade was refused. Try again in a minute.";
  if (/undercollateral|insufficient (free )?(margin|collateral)|margin (check|requirement)/i.test(text)) return "Not enough free collateral for this size. Deposit more test USDC or trade smaller.";
  if (/simulation failed|Simulation failed/i.test(text)) return "The network rejected the transaction before sending it.";
  return null;
}

/** Wrap any failure from a send: a plain message when one fits, the raw text kept as details. */
export function toChainError(err: unknown, logs: string[] = [], programs: { engine?: string; matcher?: string } = {}) {
  const raw = err instanceof Error ? err.message : String(err);
  const details = [raw, ...logs].filter(Boolean).join("\n");
  return new ChainError(plainChainError(raw, logs, programs) ?? "The transaction failed.", details);
}
