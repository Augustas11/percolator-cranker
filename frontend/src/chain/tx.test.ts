// @vitest-environment node
import { readFileSync } from "node:fs";
import { Connection, PublicKey } from "@solana/web3.js";
import { afterEach, expect, it, vi } from "vitest";
import { devnetConfig } from "./config";
import { buildDeposit, buildTrade, buildWithdraw, findPortfolio, withFreshCrank } from "./tx";
import { parsePortfolio } from "./accounts";
import { i128, u64 } from "./bytes";
import { layout } from "./layout";
import { matcherDelegate } from "./pda";
import pdas from "./fixtures/pdas.json";

const conn = new Connection(devnetConfig.rpcUrl);
const cfg = { ...devnetConfig, matcherProgram: new PublicKey(pdas.matcherDelegate.matcherProgram), matcherContext: new PublicKey(pdas.matcherDelegate.matcherContext) };
const portfolio = new PublicKey(new Uint8Array(32).fill(4));
const data = readFileSync(new URL("./fixtures/portfolio.bin", import.meta.url));
const lp = readFileSync(new URL("./fixtures/lp_portfolio.bin", import.meta.url));
const market = readFileSync(new URL("./fixtures/market.bin", import.meta.url));
const owner = parsePortfolio(data).owner;
const account = (data: Buffer) => ({ data, owner: cfg.programId, executable: false, lamports: 1, rentEpoch: 0 });

function mockCranks() {
  vi.spyOn(conn, "getSlot").mockResolvedValue(123);
  vi.spyOn(conn, "getLatestBlockhash").mockResolvedValue({ blockhash: cfg.market.toBase58(), lastValidBlockHeight: 1234 });
  return vi.spyOn(conn, "simulateTransaction").mockResolvedValue({ context: { slot: 123 }, value: { err: null, logs: [] } });
}
afterEach(() => vi.restoreAllMocks());

it("finds portfolios using the owner offset and account length", async () => {
  const get = vi.spyOn(conn, "getProgramAccounts").mockResolvedValue([{ pubkey: portfolio, account: account(data) }]);
  expect(await findPortfolio(conn, cfg.programId, owner)).toEqual(portfolio);
  expect(get).toHaveBeenCalledWith(cfg.programId, { filters: [
    { dataSize: layout.PORTFOLIO_ACCOUNT_LEN }, { memcmp: { offset: layout.HEADER_LEN + layout["pf.owner"], bytes: owner.toBase58() } },
  ] });
  get.mockResolvedValue([]);
  expect(await findPortfolio(conn, cfg.programId, owner)).toBeNull();
});

it("simulates each crank after previously kept cranks and drops a no-op", async () => {
  const simulate = mockCranks();
  simulate.mockResolvedValueOnce({ context: { slot: 123 }, value: { err: { InstructionError: [2, "InvalidArgument"] }, logs: [] } });
  const ixs = await withFreshCrank(conn, owner, cfg.programId, cfg.market, [portfolio, cfg.lpPortfolio, portfolio], []);
  expect(ixs.map(ix => ix.data[0])).toEqual([2, 1, 5, 5]);
  expect(simulate.mock.calls.map(call => call[0].message.compiledInstructions.length)).toEqual([3, 3, 4]);
  expect(simulate.mock.calls[0][1]).toEqual({ sigVerify: false, replaceRecentBlockhash: true });
});

it("reads a new sequence on every deposit and withdraw build", async () => {
  mockCranks();
  const changed = Buffer.from(data);
  changed.writeBigUInt64LE(987n, layout.PORTFOLIO_MATCHER_SEQUENCE_OFF);
  const read = vi.spyOn(conn, "getAccountInfo").mockResolvedValueOnce(account(data)).mockResolvedValue(account(changed));
  const first = await buildDeposit(conn, cfg, owner, portfolio, 123n);
  const second = await buildDeposit(conn, cfg, owner, portfolio, 456n);
  const withdraw = await buildWithdraw(conn, cfg, owner, portfolio, 789n);
  expect(u64(first.at(-1)!.data, 9)).toBe(1n);
  expect(u64(second.at(-1)!.data, 9)).toBe(987n);
  expect(u64(withdraw.at(-1)!.data, 9)).toBe(987n);
  expect(read).toHaveBeenCalledTimes(3);
  expect(withdraw.at(-2)!.data[0]).toBe(5);
});

it("builds a fresh TradeCpi binding with the LP's runtime owner and no LP signer", async () => {
  mockCranks();
  const read = vi.spyOn(conn, "getMultipleAccountsInfo").mockResolvedValue([account(data), account(lp), account(market)]);
  const ixs = await buildTrade(conn, cfg, owner, portfolio, -123n, 100000000n, 12n);
  const trade = ixs.at(-1)!;
  expect(trade.data[0]).toBe(10);
  expect(u64(trade.data, 1)).toBe(2n);
  expect(u64(trade.data, 9)).toBe(15n);
  expect(u64(trade.data, 17)).toBe(1n);
  expect(u64(trade.data, 25)).toBe(15n);
  expect(u64(trade.data, 33)).toBe(1n);
  expect(i128(trade.data, 51)).toBe(-123n);
  expect(trade.keys.filter(key => key.isSigner).map(key => key.pubkey)).toEqual([owner]);
  expect(trade.keys[6].pubkey).toEqual(matcherDelegate(cfg.programId, cfg.market, cfg.lpPortfolio,
    parsePortfolio(lp).owner, cfg.matcherProgram, cfg.matcherContext));
  expect(read).toHaveBeenCalledTimes(1);
  await expect(buildTrade(conn, { ...cfg, matcherContext: undefined }, owner, portfolio, 1n, 1n, 12n))
    .rejects.toThrow("Trading is waiting on the matcher program (devnet PoC)");
});
