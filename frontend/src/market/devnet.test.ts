// @vitest-environment node
import { readFileSync } from "node:fs";
import { Connection, Keypair, SendTransactionError, Transaction } from "@solana/web3.js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { DevnetMarketClient } from "./devnet";
import { devnetConfig } from "../chain/config";
import { layout } from "../chain/layout";
import { i128 } from "../chain/bytes";

const tester = Keypair.generate(), portfolio = Keypair.generate().publicKey;
const market = readFileSync(new URL("../chain/fixtures/market.bin", import.meta.url));
const lp = readFileSync(new URL("../chain/fixtures/lp_portfolio.bin", import.meta.url));
let data: Buffer;
let client: DevnetMarketClient;
const account = (data: Buffer) => ({ data, owner: devnetConfig.programId, executable: false, lamports: 1, rentEpoch: 0 });

beforeEach(() => {
  vi.useFakeTimers();
  data = Buffer.from(readFileSync(new URL("../chain/fixtures/portfolio.bin", import.meta.url)));
  tester.publicKey.toBuffer().copy(data, layout.HEADER_LEN + layout["pf.owner"]);
  vi.spyOn(Connection.prototype, "getMultipleAccountsInfo").mockImplementation(async keys => keys.map(key =>
    account(key.equals(devnetConfig.market) ? market : key.equals(devnetConfig.lpPortfolio) ? lp : data)));
  vi.spyOn(Connection.prototype, "getProgramAccounts").mockResolvedValue([{ pubkey: portfolio, account: account(data) }]);
  vi.spyOn(Connection.prototype, "getAccountInfo").mockImplementation(async key => account(key.equals(devnetConfig.market) ? market : data));
  vi.spyOn(Connection.prototype, "getLatestBlockhash").mockResolvedValue({ blockhash: devnetConfig.market.toBase58(), lastValidBlockHeight: 1234 });
  vi.spyOn(Connection.prototype, "getSlot").mockResolvedValue(123);
  vi.spyOn(Connection.prototype, "simulateTransaction").mockResolvedValue({ context: { slot: 123 }, value: { err: "InvalidArgument", logs: [] } });
  vi.spyOn(Connection.prototype, "sendRawTransaction").mockResolvedValue("signature");
  vi.spyOn(Connection.prototype, "confirmTransaction").mockResolvedValue({ context: { slot: 123 }, value: { err: null } });
  vi.stubGlobal("window", { solflare: {
    publicKey: null as typeof tester.publicKey | null,
    async connect() { this.publicKey = tester.publicKey; },
    async signTransaction(tx: Transaction) { tx.partialSign(tester); return tx; },
  } });
});
afterEach(() => { client?.dispose(); vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

it("polls engine values and leaves entry price and unrealized PnL unknown", async () => {
  client = new DevnetMarketClient({ ...devnetConfig, matcherProgram: undefined, matcherContext: undefined });
  expect(await client.connect()).toBe(tester.publicKey.toBase58());
  const s = client.getSnapshot();
  expect(client.tradingReady).toBe(false);
  expect(s.market.maxLeverage).toBe(5);
  expect(s.mark.price).toBe(99.938164);
  expect(s.portfolio.collateral).toBe(29982.010738);
  expect(s.portfolio.legs[0]).toEqual({ side: "long", size: 10, entryPrice: NaN, unrealizedPnl: NaN });
  expect(s.portfolio.health).toEqual({
    equity: s.portfolio.collateral, initialMarginUsed: NaN, maintenanceMargin: NaN, freeCollateral: NaN, ratio: NaN, status: "unknown",
  });
  const reads = vi.mocked(Connection.prototype.getMultipleAccountsInfo);
  const before = reads.mock.calls.length;
  await vi.advanceTimersByTimeAsync(2000);
  expect(reads.mock.calls.length).toBe(before + 1);
  client.dispose();
  await vi.advanceTimersByTimeAsync(2000);
  expect(reads.mock.calls.length).toBe(before + 1);
});

it("creates a portfolio with its keypair signature before depositing and creating the ATA", async () => {
  vi.mocked(Connection.prototype.getProgramAccounts).mockResolvedValue([]);
  const send = vi.mocked(Connection.prototype.sendRawTransaction);
  client = new DevnetMarketClient({ ...devnetConfig, matcherProgram: undefined, matcherContext: undefined });
  await client.connect();
  await expect(client.deposit(0.0000001)).rejects.toThrow(/positive/);
  expect(send).not.toHaveBeenCalled();
  await client.deposit(1.25);
  const [create, deposit] = send.mock.calls.map(call => Transaction.from(call[0]));
  expect(create.signatures).toHaveLength(2);
  expect(create.verifySignatures()).toBe(true);
  expect(deposit.verifySignatures()).toBe(true);
  expect(deposit.signatures).toHaveLength(1);
  expect(deposit.instructions[0].programId.toBase58()).toBe("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
  expect(deposit.instructions.at(-1)!.data.readBigUInt64LE(17)).toBe(1250000n);
});

it.each(["long", "short"] as const)("closes a %s using exact bigint size and surfaces simulation logs", async side => {
  const q = 9007199254740993n;
  const leg = layout.HEADER_LEN + layout["pf.legs"];
  data[leg + layout["leg.side"]] = side === "long" ? 0 : 1;
  data.writeBigInt64LE(side === "long" ? q : -q, leg + layout["leg.basis_pos_q"]);
  data.writeBigInt64LE(side === "long" ? 0n : -1n, leg + layout["leg.basis_pos_q"] + 8);
  client = new DevnetMarketClient({ ...devnetConfig, matcherProgram: tester.publicKey, matcherContext: portfolio });
  await client.connect();
  await client.closePosition();
  const send = vi.mocked(Connection.prototype.sendRawTransaction);
  const tx = Transaction.from(send.mock.calls[0][0]);
  expect(i128(tx.instructions.at(-1)!.data, 51)).toBe(side === "long" ? -q : q);
  expect(tx.signatures).toHaveLength(1);
  expect(client.getSnapshot().fills).toHaveLength(0);
  await expect(client.openPosition("long", 1, 6)).rejects.toThrow(/leverage/);
  send.mockRejectedValue(new SendTransactionError({ action: "send", signature: "", transactionMessage: "simulation failed", logs: ["first", "matcher rejected"] }));
  await expect(client.openPosition("long", 1, 2)).rejects.toThrow(/matcher rejected/);
  expect(client.getSnapshot().fills).toHaveLength(0);
});

it("ignores positive PnL when computing health", async () => {
  data.writeBigInt64LE(1_000_000n, layout.HEADER_LEN + layout["pf.pnl"]);
  client = new DevnetMarketClient(devnetConfig);
  await client.connect();
  expect(client.getSnapshot().portfolio.health.equity).toBe(29982.010738);
});

it("keeps a confirmed new portfolio and resolves deposits when refresh fails", async () => {
  vi.mocked(Connection.prototype.getProgramAccounts).mockResolvedValue([]);
  client = new DevnetMarketClient(devnetConfig);
  await client.connect();
  vi.mocked(Connection.prototype.getMultipleAccountsInfo).mockRejectedValue(new Error("RPC unavailable"));
  vi.mocked(Connection.prototype.getAccountInfo).mockResolvedValue(account(data));
  await expect(client.deposit(1)).resolves.toBeUndefined();
  await expect(client.deposit(1)).resolves.toBeUndefined();
  const txs = vi.mocked(Connection.prototype.sendRawTransaction).mock.calls.map(call => Transaction.from(call[0]));
  expect(txs).toHaveLength(3);
  expect(txs[0].signatures).toHaveLength(2);
  expect(txs[1].instructions.at(-1)!.keys[2].pubkey).toEqual(txs[0].instructions.at(-1)!.keys[2].pubkey);
  expect(txs[2].signatures).toHaveLength(1);
});

it("waits for a slow poll to finish before scheduling another", async () => {
  let finish!: (accounts: ReturnType<typeof account>[]) => void;
  const reads = vi.mocked(Connection.prototype.getMultipleAccountsInfo);
  reads.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  client = new DevnetMarketClient(devnetConfig);
  await vi.advanceTimersByTimeAsync(6000);
  expect(reads).toHaveBeenCalledTimes(1);
  finish([account(market)]);
  await vi.advanceTimersByTimeAsync(0);
  expect(client.getSnapshot().mark.price).toBe(99.938164);
  await vi.advanceTimersByTimeAsync(1999);
  expect(reads).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(reads).toHaveBeenCalledTimes(2);
});

it.each(["missing leg", "zero size", "expired epoch"])("rejects a close without sending when the post-crank portfolio has %s", async flat => {
  client = new DevnetMarketClient({ ...devnetConfig, matcherProgram: tester.publicKey, matcherContext: portfolio });
  await client.connect();
  expect(client.getSnapshot().portfolio.legs).toHaveLength(1);
  const postCrank = Buffer.from(data);
  const leg = layout.HEADER_LEN + layout["pf.legs"];
  if (flat === "missing leg") postCrank[leg + layout["leg.active"]] = 0;
  else if (flat === "zero size") postCrank.fill(0, leg + layout["leg.basis_pos_q"], leg + layout["leg.basis_pos_q"] + 16);
  else postCrank.writeBigUInt64LE(999n, leg + layout["leg.epoch_snap"]);
  vi.mocked(Connection.prototype.simulateTransaction).mockResolvedValue({ context: { slot: 123 }, value: {
    err: null, logs: [], accounts: [postCrank, lp, market].map(data => ({
      ...account(data), owner: devnetConfig.programId.toBase58(), data: [data.toString("base64"), "base64"],
    })),
  } });
  const reads = vi.mocked(Connection.prototype.getMultipleAccountsInfo);
  reads.mockClear();
  await expect(client.closePosition()).rejects.toThrow("no open position");
  expect(Connection.prototype.sendRawTransaction).not.toHaveBeenCalled();
  expect(reads).not.toHaveBeenCalled();
});

it("keeps flat health margins at zero and free collateral available", async () => {
  const leg = layout.HEADER_LEN + layout["pf.legs"];
  data[leg + layout["leg.active"]] = 0;
  client = new DevnetMarketClient(devnetConfig);
  await client.connect();
  const { collateral, legs, health } = client.getSnapshot().portfolio;
  expect(legs).toEqual([]);
  expect(health).toEqual({ equity: collateral, initialMarginUsed: 0, maintenanceMargin: 0, freeCollateral: collateral, ratio: Infinity, status: "healthy" });
});
