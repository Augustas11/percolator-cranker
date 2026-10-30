import { LAMPORTS_PER_SOL, PublicKey, TransactionInstruction } from "@solana/web3.js";
import { ata, TOKEN_PROGRAM_ID } from "../../frontend/src/chain/pda";

export const WINDOW = 86400;
export const DEFAULT_MINT = "Ev8AkZC9pT2QCncYFut8GyGB2ffWN1yvnoVbiUTz77q7";
export const DEVNET_GENESIS = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";
export interface State { wallets: Record<string, number>; day: string; count: number; sol_count: number }
export class ClaimError extends Error {
  constructor(public status: number, message: string, public nextClaimAfter?: number, public submitted = false, public solLamports?: number) { super(message); }
}

export async function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([promise, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error("RPC timed out")), ms);
    })]);
  } finally { clearTimeout(timer); }
}

export function checkGetBody(method: string | undefined, headers: { "content-length"?: string; "transfer-encoding"?: string }): void {
  if (method === "GET" && (Number(headers["content-length"]) > 0 || headers["transfer-encoding"] !== undefined)) {
    throw new ClaimError(413, "GET requests must not declare a body");
  }
}

export async function readBody(chunks: AsyncIterable<Uint8Array>): Promise<unknown> {
  const buffers: Buffer[] = [];
  let size = 0;
  for await (const chunk of chunks) {
    size += chunk.byteLength;
    if (size > 1024) throw new ClaimError(413, "Body exceeds 1024 bytes");
    buffers.push(Buffer.from(chunk));
  }
  try { return JSON.parse(Buffer.concat(buffers).toString("utf8")); }
  catch { throw new ClaimError(400, "Expected JSON body"); }
}

export function parseWallet(body: unknown): PublicKey {
  if (!body || typeof body !== "object" || Array.isArray(body) ||
      Object.keys(body).length !== 1 || !("wallet" in body) || typeof body.wallet !== "string") {
    throw new ClaimError(400, "Expected only a wallet field");
  }
  try {
    const wallet = new PublicKey(body.wallet);
    if (!PublicKey.isOnCurve(wallet.toBytes())) throw new Error();
    return wallet;
  } catch { throw new ClaimError(400, "Invalid on-curve wallet public key"); }
}

export function parseState(value: unknown): State {
  if (!value || typeof value !== "object") throw new Error("Invalid faucet state");
  const state = value as State;
  if (!state.wallets || typeof state.wallets !== "object" || Array.isArray(state.wallets) ||
      typeof state.day !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(state.day) ||
      new Date(`${state.day}T00:00:00Z`).toISOString().slice(0, 10) !== state.day ||
      !Number.isSafeInteger(state.count) || state.count < 0 ||
      (state.sol_count !== undefined && (!Number.isSafeInteger(state.sol_count) || state.sol_count < 0))) throw new Error("Invalid faucet state");
  for (const [wallet, time] of Object.entries(state.wallets)) {
    parseWallet({ wallet });
    if (!Number.isSafeInteger(time) || time < 0) throw new Error("Invalid faucet state");
  }
  return { ...state, sol_count: state.sol_count ?? 0 };
}

export function pruneState(state: State, now: number): State {
  const day = new Date(now * 1000).toISOString().slice(0, 10);
  return {
    wallets: Object.fromEntries(Object.entries(state.wallets).filter(([, time]) => time + WINDOW > now)),
    day, count: state.day === day ? state.count : 0,
    sol_count: state.day === day ? state.sol_count : 0,
  };
}

export function checkTokenAccount(account: { owner: PublicKey; data: Buffer } | null, wallet: PublicKey, mint: PublicKey): void {
  if (!account || !account.owner.equals(TOKEN_PROGRAM_ID) || account.data.length !== 165 ||
      !account.data.subarray(0, 32).equals(mint.toBuffer()) ||
      !account.data.subarray(32, 64).equals(wallet.toBuffer()) || account.data[108] !== 1) {
    throw new ClaimError(400, "Create the test-USDC token account first (the faucet creates it only for wallets that receive SOL)");
  }
}

export function mintTo(mint: PublicKey, destination: PublicKey, authority: PublicKey, amountUsdc: number): TransactionInstruction {
  const data = Buffer.alloc(9);
  data[0] = 7;
  data.writeBigUInt64LE(BigInt(amountUsdc) * 1_000_000n, 1);
  return new TransactionInstruction({ programId: TOKEN_PROGRAM_ID, data, keys: [
    { pubkey: mint, isWritable: true, isSigner: false },
    { pubkey: destination, isWritable: true, isSigner: false },
    { pubkey: authority, isWritable: false, isSigner: true },
  ] });
}

export function needsSol(walletLamports: number, payerLamports: number, dripLamports: number, floor: number, allowSol: boolean): boolean {
  return allowSol && walletLamports < dripLamports && payerLamports >= floor + dripLamports;
}

export class Faucet {
  private tail: Promise<unknown> = Promise.resolve();
  private pending = 0;
  constructor(private state: State, private options: {
    mint: PublicKey; dailyCap: number; amountUsdc: number;
    solDailyCap: number; solLamports: number; minPayerLamports: number; payerLamports: number;
    now: () => number; save: (state: State) => Promise<void>;
    send: (wallet: PublicKey, destination: PublicKey, allowSol: boolean) => Promise<{ signature: string; solLamports: number }>;
  }) {}

  observePayer(lamports: number) { this.options.payerLamports = lamports; }

  health() {
    const state = pruneState(this.state, this.options.now());
    return { ok: true, claims_today: state.count,
      daily_cap: this.options.dailyCap, amount_usdc: this.options.amountUsdc,
      sol_drips_today: state.sol_count, sol_daily_cap: this.options.solDailyCap,
      sol_paused: state.sol_count >= this.options.solDailyCap || this.options.payerLamports < this.options.minPayerLamports + this.options.solLamports };
  }

  claim(wallet: PublicKey) {
    if (this.pending >= 8) return Promise.reject(new ClaimError(503, "Faucet busy"));
    this.pending++;
    const result = this.tail.then(async () => {
      const now = this.options.now();
      this.state = pruneState(this.state, now);
      const key = wallet.toBase58();
      const last = this.state.wallets[key];
      if (last !== undefined) throw new ClaimError(429, "Wallet already claimed in the last 24 hours", last + WINDOW);
      if (this.state.count >= this.options.dailyCap) throw new ClaimError(429, "Global daily cap reached");
      const previous = this.state;
      const allowSol = previous.sol_count < this.options.solDailyCap;
      this.state = { ...previous, wallets: { ...previous.wallets, [key]: now }, count: previous.count + 1,
        sol_count: previous.sol_count + Number(allowSol) };
      let result;
      try {
        await this.options.save(this.state);
        result = await this.options.send(wallet, ata(wallet, this.options.mint), allowSol);
      } catch (error) {
        if (error instanceof ClaimError && error.submitted) {
          if (allowSol && error.solLamports === 0) {
            this.state = { ...this.state, sol_count: this.state.sol_count - 1 };
            await this.options.save(this.state).catch(() => {});
          }
          throw new ClaimError(503, `Mint outcome unknown; try again after ${now + WINDOW}`, now + WINDOW, true, error.solLamports);
        }
        this.state = previous;
        await this.options.save(previous);
        throw error;
      }
      if (allowSol && result.solLamports === 0) {
        this.state = { ...this.state, sol_count: this.state.sol_count - 1 };
        await this.options.save(this.state).catch(() => {});
      }
      return { ok: true, wallet: key, amount_usdc: this.options.amountUsdc, signature: result.signature,
        sol: result.solLamports / LAMPORTS_PER_SOL, next_claim_after: now + WINDOW };
    }).finally(() => { this.pending--; });
    this.tail = result.catch(() => {});
    return result;
  }
}
