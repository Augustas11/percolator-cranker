import { describe, expect, it, vi } from "vitest";
import { Keypair, PublicKey } from "@solana/web3.js";
import { ata, TOKEN_PROGRAM_ID } from "../../frontend/src/chain/pda";
import { ClaimError, DEFAULT_MINT, Faucet, WINDOW, checkGetBody, checkTokenAccount, mintTo, needsSol, parseState, parseWallet, pruneState, readBody, withTimeout, type State } from "./faucet";

const wallet = Keypair.generate().publicKey;
const other = Keypair.generate().publicKey;
const mint = new PublicKey(DEFAULT_MINT);
const start = Date.parse("2026-10-30T23:59:00Z") / 1000;
const empty = (): State => ({ wallets: {}, day: "2026-10-30", count: 0, sol_count: 0 });
async function* chunks(...values: string[]) { for (const value of values) yield Buffer.from(value); }
function setup(dailyCap = 200, solDailyCap = 20) {
  let now = start;
  const save = vi.fn(async (_state: State) => {});
  const sent = { signature: "signature", solLamports: 50_000_000 };
  const send = vi.fn(async (_wallet: PublicKey, _destination: PublicKey, _allowSol: boolean) => sent);
  const faucet = new Faucet(empty(), { mint, dailyCap, amountUsdc: 1000, solDailyCap, solLamports: 50_000_000, minPayerLamports: 3_000_000_000, payerLamports: 4_000_000_000, now: () => now, save, send });
  return { faucet, save, send, sent, setTime: (time: number) => { now = time; } };
}

describe("request validation", () => {
  it("rejects declared GET bodies, including chunked bodies, before reading them", () => {
    for (const headers of [{ "content-length": "1" }, { "transfer-encoding": "chunked" }, { "transfer-encoding": "" }]) {
      expect(() => checkGetBody("GET", headers)).toThrow(ClaimError);
    }
    expect(() => checkGetBody("GET", { "content-length": "0" })).not.toThrow();
    expect(() => checkGetBody("POST", { "content-length": "1025" })).not.toThrow();
  });
  it("accepts JSON up to exactly 1024 bytes", async () => {
    expect(await readBody(chunks('"' + "a".repeat(1022) + '"'))).toHaveLength(1022);
  });
  it("rejects streamed bodies at the byte limit before consuming more", async () => {
    let consumed = 0;
    async function* oversized() {
      for (const chunk of ["a".repeat(1024), "b", "unused"]) { consumed++; yield Buffer.from(chunk); }
    }
    await expect(readBody(oversized())).rejects.toMatchObject({ status: 413 });
    expect(consumed).toBe(2);
    await expect(readBody(chunks("é".repeat(513)))).rejects.toMatchObject({ status: 413 });
  });
  it("rejects non-JSON", async () => {
    await expect(readBody(chunks("nope"))).rejects.toMatchObject({ status: 400 });
  });
  it.each([null, [], {}, { wallet: 3 }, { wallet: wallet.toBase58(), extra: true }, { wallet: "invalid" }])("rejects invalid fields: %j", body => {
    expect(() => parseWallet(body)).toThrow(ClaimError);
  });
  it("accepts an on-curve wallet and rejects an off-curve PDA", () => {
    expect(parseWallet({ wallet: wallet.toBase58() }).equals(wallet)).toBe(true);
    expect(() => parseWallet({ wallet: ata(wallet, mint).toBase58() })).toThrow(/on-curve/);
  });
});

describe("limits and reservations", () => {
  it("enforces the rolling window, including across UTC midnight", async () => {
    const { faucet, setTime, send } = setup();
    expect(await faucet.claim(wallet)).toMatchObject({ next_claim_after: start + WINDOW, amount_usdc: 1000 });
    setTime(start + WINDOW - 1);
    await expect(faucet.claim(wallet)).rejects.toMatchObject({ status: 429, nextClaimAfter: start + WINDOW });
    setTime(start + WINDOW);
    await faucet.claim(wallet);
    expect(send).toHaveBeenCalledTimes(2);
  });
  it("rolls the global cap over at UTC midnight", async () => {
    const { faucet, setTime } = setup(1);
    await faucet.claim(wallet);
    await expect(faucet.claim(other)).rejects.toMatchObject({ status: 429, message: "Global daily cap reached" });
    setTime(start + 60);
    expect(faucet.health().claims_today).toBe(0);
    await faucet.claim(other);
    expect(faucet.health().sol_drips_today).toBe(1);
    expect(faucet.health().claims_today).toBe(1);
    await expect(faucet.claim(wallet)).rejects.toMatchObject({ status: 429 });
  });
  it("persists reservations before sending and releases failures", async () => {
    const { faucet, save, send } = setup(1);
    send.mockImplementationOnce(async () => {
      expect(save).toHaveBeenCalledWith(expect.objectContaining({ count: 1, sol_count: 1 }));
      throw new ClaimError(503, "simulation failed");
    });
    await expect(faucet.claim(wallet)).rejects.toMatchObject({ status: 503 });
    expect(save).toHaveBeenLastCalledWith(empty());
    expect(faucet.health().claims_today).toBe(0);
    await faucet.claim(wallet);
  });
  it("does not send if reservation persistence fails", async () => {
    const { faucet, save, send } = setup();
    save.mockRejectedValueOnce(new Error("disk full"));
    await expect(faucet.claim(wallet)).rejects.toThrow("disk full");
    expect(send).not.toHaveBeenCalled();
    await faucet.claim(wallet);
  });
  it("keeps ambiguous submissions reserved and prevents bypassing a daily cap of one", async () => {
    const { faucet, save, send } = setup(1);
    send.mockRejectedValueOnce(new ClaimError(503, "submission failed", undefined, true));
    await expect(faucet.claim(wallet)).rejects.toMatchObject({
      status: 503, nextClaimAfter: start + WINDOW, message: `Mint outcome unknown; try again after ${start + WINDOW}`,
    });
    expect(save).toHaveBeenCalledTimes(1);
    expect(faucet.health().sol_drips_today).toBe(1);
    expect(faucet.health().claims_today).toBe(1);
    await expect(faucet.claim(wallet)).rejects.toMatchObject({ status: 429, nextClaimAfter: start + WINDOW });
    await expect(faucet.claim(other)).rejects.toMatchObject({ status: 429, message: "Global daily cap reached" });
    expect(send).toHaveBeenCalledTimes(1);
  });
  it.each([false, true])("releases ambiguous USDC-only SOL slots even if saving fails: %s", async saveFails => {
    const { faucet, save, send } = setup(1);
    send.mockRejectedValueOnce(new ClaimError(503, "submission failed", undefined, true, 0));
    if (saveFails) save.mockResolvedValueOnce().mockRejectedValueOnce(new Error("disk full"));
    await expect(faucet.claim(wallet)).rejects.toMatchObject({
      status: 503, nextClaimAfter: start + WINDOW, submitted: true, solLamports: 0,
      message: `Mint outcome unknown; try again after ${start + WINDOW}`,
    });
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ count: 1, sol_count: 0 }));
    expect(faucet.health()).toMatchObject({ claims_today: 1, sol_drips_today: 0 });
    await expect(faucet.claim(wallet)).rejects.toMatchObject({ status: 429 });
    await expect(faucet.claim(other)).rejects.toMatchObject({ status: 429, message: "Global daily cap reached" });
    expect(send).toHaveBeenCalledTimes(1);
  });
  it("reserves a SOL slot and persists its release when no SOL is sent", async () => {
    const { faucet, save, send, sent } = setup();
    send.mockImplementationOnce(async () => {
      expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ sol_count: 1, count: 1 }));
      faucet.observePayer(3_000_000_000);
      return { ...sent, solLamports: 0 };
    });
    expect(await faucet.claim(wallet)).toMatchObject({ sol: 0 });
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ sol_count: 0, count: 1 }));
    expect(faucet.health()).toMatchObject({ sol_drips_today: 0, sol_daily_cap: 20, sol_paused: true });
    faucet.observePayer(4_000_000_000);
    expect(await faucet.claim(other)).toMatchObject({ sol: 0.05 });
    expect(faucet.health().sol_paused).toBe(false);
  });
  it("continues USDC claims with allowSol false at the SOL cap and resets at midnight", async () => {
    const { faucet, send, sent, setTime } = setup(200, 1);
    await faucet.claim(wallet);
    expect(faucet.health()).toMatchObject({ sol_drips_today: 1, sol_daily_cap: 1, sol_paused: true });
    send.mockResolvedValueOnce({ ...sent, solLamports: 0 });
    expect(await faucet.claim(other)).toMatchObject({ amount_usdc: 1000, sol: 0 });
    expect(send).toHaveBeenLastCalledWith(other, ata(other, mint), false);
    setTime(start + 60);
    expect(faucet.health()).toMatchObject({ sol_drips_today: 0, claims_today: 0, sol_paused: false });
  });
  it("keeps the confirmed claim if persisting a released SOL slot fails", async () => {
    const { faucet, save, send, sent } = setup();
    send.mockResolvedValueOnce({ ...sent, solLamports: 0 });
    save.mockResolvedValueOnce().mockRejectedValueOnce(new Error("disk full"));
    await expect(faucet.claim(wallet)).resolves.toMatchObject({ ok: true, sol: 0, signature: "signature" });
    expect(faucet.health()).toMatchObject({ claims_today: 1, sol_drips_today: 0 });
    await expect(faucet.claim(wallet)).rejects.toMatchObject({ status: 429 });
    expect(send).toHaveBeenCalledTimes(1);
  });
  it("rejects overflow immediately and releases queue slots after failures", async () => {
    const { faucet, send } = setup();
    let release!: () => void;
    send.mockImplementationOnce(() => new Promise<{ signature: string; solLamports: number }>(resolve => { release = () => resolve({ signature: "signature", solLamports: 50_000_000 }); }));
    const queued = Array.from({ length: 8 }, () => faucet.claim(wallet).catch(error => error));
    await expect(faucet.claim(other)).rejects.toMatchObject({ status: 503, message: "Faucet busy" });
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    release();
    await Promise.all(queued);
    await expect(faucet.claim(other)).resolves.toMatchObject({ ok: true });
  });
  it("serializes concurrent requests so the cap cannot be raced", async () => {
    const { faucet, send } = setup(1);
    const results = await Promise.allSettled([faucet.claim(wallet), faucet.claim(other)]);
    expect(results.map(result => result.status)).toEqual(["fulfilled", "rejected"]);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith(wallet, ata(wallet, mint), true);
  });
});

describe("RPC timeouts", () => {
  it("bounds a stalled RPC and clears timers on success and failure", async () => {
    vi.useFakeTimers();
    try {
      const result = withTimeout(new Promise<never>(() => {}), 60_000);
      const rejected = expect(result).rejects.toThrow("RPC timed out");
      await vi.advanceTimersByTimeAsync(60_000);
      await rejected;
      expect(await withTimeout(Promise.resolve("ok"), 10_000)).toBe("ok");
      await expect(withTimeout(Promise.reject(new Error("failed")), 10_000)).rejects.toThrow("failed");
      expect(vi.getTimerCount()).toBe(0);
    } finally { vi.useRealTimers(); }
  });
});

describe("state", () => {
  it("prunes expired wallets at the exact boundary", () => {
    const state = { wallets: { [wallet.toBase58()]: start - WINDOW, [other.toBase58()]: start - 1 }, day: "2026-10-29", count: 200, sol_count: 20 };
    expect(pruneState(state, start)).toEqual({ wallets: { [other.toBase58()]: start - 1 }, day: "2026-10-30", count: 0, sol_count: 0 });
  });
  it("prunes on persisted claims", async () => {
    const { faucet, save, setTime } = setup();
    await faucet.claim(wallet);
    setTime(start + WINDOW);
    await faucet.claim(other);
    expect(save).toHaveBeenLastCalledWith({ wallets: { [other.toBase58()]: start + WINDOW }, day: "2026-10-31", count: 1, sol_count: 1 });
  });
  it("validates saved state", () => {
    expect(parseState(empty())).toEqual(empty());
    const { sol_count, ...legacy } = empty();
    expect(parseState(legacy)).toEqual(empty());
    for (const sol_count of [-1, 1.5, Number.MAX_SAFE_INTEGER + 1, null, "1"]) {
      expect(() => parseState({ ...empty(), sol_count })).toThrow();
    }
    for (const value of [null, {}, { ...empty(), count: -1 }, { ...empty(), wallets: [] }, { ...empty(), wallets: { bad: 3 } }]) {
      expect(() => parseState(value)).toThrow();
    }
  });
});

describe("SPL token checks", () => {
  function account() {
    const data = Buffer.alloc(165);
    mint.toBuffer().copy(data, 0);
    wallet.toBuffer().copy(data, 32);
    data[108] = 1;
    return { owner: TOKEN_PROGRAM_ID, data };
  }
  it("accepts only the claimant's initialized token account", () => {
    expect(() => checkTokenAccount(account(), wallet, mint)).not.toThrow();
    const invalid = [null, { ...account(), owner: other }, { ...account(), data: Buffer.alloc(164) }];
    for (const offset of [0, 32, 108]) {
      const value = account();
      value.data[offset] ^= 1;
      invalid.push(value);
    }
    for (const value of invalid) expect(() => checkTokenAccount(value, wallet, mint)).toThrow(/token account first/);
  });
  it("encodes exactly one 9-byte MintTo with u64 little endian base units", () => {
    const destination = ata(wallet, mint);
    const ix = mintTo(mint, destination, wallet, 100000);
    expect(ix.programId.equals(TOKEN_PROGRAM_ID)).toBe(true);
    expect([...ix.data]).toEqual([7, 0, 232, 118, 72, 23, 0, 0, 0]);
    expect(ix.data.readBigUInt64LE(1)).toBe(100000000000n);
    expect(ix.keys).toEqual([
      { pubkey: mint, isWritable: true, isSigner: false },
      { pubkey: destination, isWritable: true, isSigner: false },
      { pubkey: wallet, isWritable: false, isSigner: true },
    ]);
  });
});

describe("SOL decision", () => {
  it.each([
    [0, 3_050_000_000, true, true],
    [49_999_999, 4_000_000_000, true, true],
    [50_000_000, 4_000_000_000, true, false],
    [0, 3_049_999_999, true, false],
    [0, 4_000_000_000, false, false],
  ])("decides for wallet %s, payer %s, allowed %s", (walletBalance, payerBalance, allowSol, expected) => {
    expect(needsSol(walletBalance, payerBalance, 50_000_000, 3_000_000_000, allowSol)).toBe(expected);
  });
});
