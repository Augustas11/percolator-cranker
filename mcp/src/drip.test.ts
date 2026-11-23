import { mkdtempSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Keypair, SystemProgram } from "@solana/web3.js";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { atoms, checkMatcher, DEVNET_GENESIS, Drip, genesisGuard, loadKeypair, marketOverrides, parseMatcher, redactRpc, safeError } from "./drip";
import { createServer } from "./index";
import { tools as registeredTools } from "./tools";
import * as tx from "../../frontend/src/chain/tx";
import closePosition, { closeChunk } from "./tools/close_position";
import claimTestFunds from "./tools/claim_test_funds";
import deposit from "./tools/deposit";
import { PORTFOLIO_LAMPORTS } from "../../frontend/src/chain/instructions";

const homes: string[] = [];
function home() { const path = mkdtempSync(join(tmpdir(), "drip-mcp-test-")); homes.push(path); return path; }
const key = Keypair.generate();
const secret = JSON.stringify(Array.from(key.secretKey));
const env = { DRIP_SECRET_KEY: secret };
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); for (const path of homes.splice(0)) rmSync(path, { recursive: true, force: true }); });

describe("keypair", () => {
  it("loads an explicit path, with precedence over JSON", () => {
    const path = join(home(), "wallet.json");
    writeFileSync(path, secret);
    expect(loadKeypair({ DRIP_KEYPAIR: path, DRIP_SECRET_KEY: "invalid" }).keypair.publicKey.equals(key.publicKey)).toBe(true);
  });
  it("loads env JSON", () => {
    const loaded = loadKeypair(env);
    expect(loaded.keypair.publicKey.equals(key.publicKey)).toBe(true);
    expect(loaded.generated).toBe(false);
  });
  it("generates a private default file without overwriting it", () => {
    const dir = home(), first = loadKeypair({}, dir), path = join(dir, ".config/drip/devnet-agent.json");
    const data = readFileSync(path, "utf8");
    expect(first.generated).toBe(true);
    expect(statSync(path).mode & 0o777).toBe(0o600);
    const second = loadKeypair({}, dir);
    expect(second.generated).toBe(false);
    expect(second.keypair.publicKey.equals(first.keypair.publicKey)).toBe(true);
    expect(readFileSync(path, "utf8")).toBe(data);
  });
  it("does not replace a malformed default file", () => {
    const dir = home(), parent = join(dir, ".config/drip"), path = join(parent, "devnet-agent.json");
    mkdirSync(parent, { recursive: true }); writeFileSync(path, "broken");
    expect(() => loadKeypair({}, dir)).toThrow("Cannot load default");
    expect(readFileSync(path, "utf8")).toBe("broken");
  });
  it.each(["sensitive-invalid-json", "[1,2,3]", JSON.stringify(Array(64).fill(256)), JSON.stringify(Array(64).fill(0))])("never echoes invalid secret data", value => {
    try { loadKeypair({ DRIP_SECRET_KEY: value }); throw new Error("Unexpected success"); }
    catch (error) {
      expect((error as Error).message).toBe("Invalid agent keypair: expected a Solana JSON secret-key array");
      expect((error as Error).message).not.toContain(value);
    }
  });
  it("does not echo a missing explicit path", () => {
    expect(() => loadKeypair({ DRIP_KEYPAIR: "secret-file-name" })).toThrow("Cannot read agent keypair file");
  });
});

describe("devnet guard", () => {
  it("retries transient failures and then caches success", async () => {
    const getGenesisHash = vi.fn().mockRejectedValueOnce(new Error("network error")).mockResolvedValue(DEVNET_GENESIS);
    const guard = genesisGuard({ getGenesisHash });
    const results = await Promise.allSettled([guard(), guard()]);
    expect(results.map(result => result.status)).toEqual(["rejected", "rejected"]);
    await guard(); await guard();
    expect(getGenesisHash).toHaveBeenCalledTimes(2);
  });
  it("checks only once, including concurrent callers", async () => {
    const getGenesisHash = vi.fn().mockResolvedValue(DEVNET_GENESIS), guard = genesisGuard({ getGenesisHash });
    await Promise.all([guard(), guard()]); await guard();
    expect(getGenesisHash).toHaveBeenCalledTimes(1);
  });
  it.each(["5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d", "4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY", "unknown"])("rejects non-devnet %s persistently", async hash => {
    const getGenesisHash = vi.fn().mockResolvedValue(hash), guard = genesisGuard({ getGenesisHash });
    await expect(guard()).rejects.toThrow("Refusing non-devnet");
    await expect(guard()).rejects.toThrow("Refusing non-devnet");
    expect(getGenesisHash).toHaveBeenCalledTimes(1);
  });
});

describe("closing", () => {
  it.each([[0n, 10n, 0n], [5n, 10n, 5n], [10n, 10n, 10n], [25n, 10n, 10n]])("caps size %s at %s", (size, cap, expected) => {
    expect(closeChunk(size, cap)).toBe(expected);
  });
  it.each([0n, -1n])("rejects unusable max fill %s", cap => {
    expect(() => closeChunk(10n, cap)).toThrow("max fill must be positive");
  });
  function setup(side: "long" | "short", sizes: bigint[]) {
    const drip = new Drip(env);
    const market = { asset0: { marketId: 1n, effectivePrice: 150_000_000n, aLong: 1n, aShort: 1n, epochLong: 0n, epochShort: 0n } } as Awaited<ReturnType<Drip["market"]>>;
    vi.spyOn(drip, "market").mockResolvedValue(market);
    const portfolio = vi.spyOn(drip, "portfolio").mockResolvedValue(null);
    for (const size of sizes) portfolio.mockResolvedValueOnce({ key: key.publicKey, legs: [{ assetIndex: 0, marketId: 1n, side, basisPosQ: size, aBasis: 1n, epochSnap: 0n }] } as Awaited<ReturnType<Drip["portfolio"]>>);
    vi.spyOn(drip, "matcher").mockResolvedValue({ state: { maxFill: 10_000_000n } } as Awaited<ReturnType<Drip["matcher"]>>);
    const trade = vi.spyOn(tx, "buildTrade").mockResolvedValue([]);
    const close = vi.spyOn(tx, "buildClose").mockResolvedValue([]);
    let count = 0;
    const send = vi.spyOn(drip, "send").mockImplementation(async build => {
      await build(); count++;
      return { signature: `sig-${count}`, explorer: `explorer-${count}` };
    });
    return { drip, portfolio, trade, close, send };
  }
  it.each(["long", "short"] as const)("closes a large %s in capped reducing fills", async side => {
    const { drip, portfolio, trade, close, send } = setup(side, [25_000_000n, 15_000_000n, 5_000_000n]);
    expect(await closePosition.run(drip, {})).toEqual({ signatures: ["sig-1", "sig-2", "sig-3"], explorer: ["explorer-1", "explorer-2", "explorer-3"], closed: 25, complete: true, remaining: 0 });
    expect(send).toHaveBeenCalledTimes(3); expect(portfolio).toHaveBeenCalledTimes(5);
    expect(trade).not.toHaveBeenCalled(); expect(close).toHaveBeenCalledTimes(3);
    for (const call of close.mock.calls) expect(call.slice(4)).toEqual([50n, 12n, 10_000_000n]);
  });
  it("keeps the common single-fill buildClose path", async () => {
    const { drip, trade, close, send } = setup("long", [5_000_000n]);
    expect(await closePosition.run(drip, {})).toEqual({ signatures: ["sig-1"], explorer: ["explorer-1"], closed: 5, complete: true, remaining: 0 });
    expect(trade).not.toHaveBeenCalled(); expect(close).toHaveBeenCalledTimes(1); expect(send).toHaveBeenCalledTimes(1);
  });
  it("refreshes the cap and keeps closing while a position remains", async () => {
    const { drip, close, send } = setup("long", [5_000_000n, 3_000_000n, 0n]);
    vi.mocked(drip.matcher).mockResolvedValueOnce({ state: { maxFill: 6_000_000n } } as Awaited<ReturnType<Drip["matcher"]>>);
    expect(await closePosition.run(drip, {})).toMatchObject({ closed: 5, complete: true, remaining: 0 });
    expect(send).toHaveBeenCalledTimes(2);
    expect(close.mock.calls.map(call => call[6])).toEqual([6_000_000n, 10_000_000n]);
  });
  it("reports the actual closed size after a post-crank shrink empties the position", async () => {
    const { drip, send } = setup("long", [25_000_000n, 0n]);
    expect(await closePosition.run(drip, {})).toEqual({ signatures: ["sig-1"], explorer: ["explorer-1"], closed: 25, complete: true, remaining: 0 });
    expect(send).toHaveBeenCalledTimes(1);
  });
  it("limits each call to ten fills", async () => {
    const { drip, send } = setup("short", Array.from({ length: 11 }, (_, i) => BigInt(110 - 10 * i) * 1_000_000n));
    expect(await closePosition.run(drip, {})).toMatchObject({ closed: 100, complete: false, remaining: 10 });
    expect(send).toHaveBeenCalledTimes(10);
  });
  it("does not send for an empty position", async () => {
    const { drip, send } = setup("long", [0n]);
    await expect(closePosition.run(drip, {})).rejects.toThrow("no open position");
    expect(send).not.toHaveBeenCalled();
  });
});

describe("backend availability", () => {
  it.each(["/leaderboard", "/faucet/claim", "/faucet/health"])("reports missing endpoint %s even for an HTML response", async path => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("<html>not found</html>", { status: 404 }));
    await expect(new Drip(env).fetchJson(path)).rejects.toThrow(`drip API endpoint ${path} not available yet`);
  });
  it("checks faucet health before any wallet funding or ATA rent", async () => {
    const drip = new Drip(env);
    const fetch = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("missing", { status: 404 }));
    const balance = vi.spyOn(drip.conn, "getBalance"), account = vi.spyOn(drip.conn, "getAccountInfo"), send = vi.spyOn(drip, "send");
    await expect(claimTestFunds.run(drip, {})).rejects.toThrow("drip API endpoint /faucet/health not available yet");
    expect(fetch).toHaveBeenCalledWith(`${drip.apiUrl}/faucet/health`, expect.not.objectContaining({ method: "POST" }));
    expect(balance).not.toHaveBeenCalled(); expect(account).not.toHaveBeenCalled(); expect(send).not.toHaveBeenCalled();
  });
  it.each([2_049_279, 2_049_280, 2_049_281])("creates the ATA only when rent and fees are affordable: %s", async balance => {
    const drip = new Drip(env), order: string[] = [];
    vi.spyOn(drip, "fetchJson").mockImplementation(async path => { order.push(path); return {} as never; });
    vi.spyOn(drip.conn, "getMinimumBalanceForRentExemption").mockResolvedValue(2_039_280);
    vi.spyOn(drip.conn, "getBalance").mockResolvedValue(balance);
    vi.spyOn(drip.conn, "getAccountInfo").mockResolvedValue(null);
    vi.spyOn(drip, "send").mockImplementation(async build => {
      order.push("create ATA"); expect(await build()).toHaveLength(1);
      return { signature: "sig", explorer: "explorer" };
    });
    await claimTestFunds.run(drip, {});
    expect(order).toEqual(balance >= 2_049_280 ? ["/faucet/health", "create ATA", "/faucet/claim"] : ["/faucet/health", "/faucet/claim"]);
    expect(drip.conn.getMinimumBalanceForRentExemption).toHaveBeenCalledWith(165);
  });
  it("lets the faucet fund a zero-SOL wallet without an airdrop or local ATA creation", async () => {
    const drip = new Drip(env);
    const fetch = vi.spyOn(drip, "fetchJson").mockResolvedValue({ sol: 0.08 });
    vi.spyOn(drip.conn, "getMinimumBalanceForRentExemption").mockResolvedValue(2_039_280);
    vi.spyOn(drip.conn, "getBalance").mockResolvedValue(0);
    vi.spyOn(drip.conn, "getAccountInfo").mockResolvedValue(null);
    const airdrop = vi.spyOn(drip.conn, "requestAirdrop"), send = vi.spyOn(drip, "send");
    expect(await claimTestFunds.run(drip, {})).toEqual({ sol: 0.08 });
    expect(fetch).toHaveBeenLastCalledWith("/faucet/claim", { wallet: drip.wallet.publicKey.toBase58() });
    expect(airdrop).not.toHaveBeenCalled(); expect(send).not.toHaveBeenCalled();
  });
  it.each([0, 2_049_279, 2_049_280])("adds the manual SOL hint only below rent plus the fee margin: %s", async balance => {
    const drip = new Drip(env);
    vi.spyOn(drip, "fetchJson").mockResolvedValueOnce({}).mockRejectedValueOnce(new Error("Claim unavailable"));
    vi.spyOn(drip.conn, "getMinimumBalanceForRentExemption").mockResolvedValue(2_039_280);
    vi.spyOn(drip.conn, "getBalance").mockResolvedValue(balance);
    vi.spyOn(drip.conn, "getAccountInfo").mockResolvedValue({} as never);
    const expected = balance < 2_049_280
      ? `Claim unavailable; get devnet SOL at https://faucet.solana.com for wallet ${drip.wallet.publicKey.toBase58()}`
      : "Claim unavailable";
    await expect(claimTestFunds.run(drip, {})).rejects.toThrow(expected);
  });
  it("does not recreate an existing ATA", async () => {
    const drip = new Drip(env);
    vi.spyOn(drip, "fetchJson").mockResolvedValue({});
    vi.spyOn(drip.conn, "getMinimumBalanceForRentExemption").mockResolvedValue(2_039_280);
    vi.spyOn(drip.conn, "getBalance").mockResolvedValue(10_000_000);
    vi.spyOn(drip.conn, "getAccountInfo").mockResolvedValue({} as never);
    const send = vi.spyOn(drip, "send");
    await claimTestFunds.run(drip, {});
    expect(send).not.toHaveBeenCalled();
  });

});

describe("deposit", () => {
  it.each([-1, 0])("requires portfolio lamports plus fees without a rent RPC: %s", async offset => {
    const drip = new Drip(env);
    vi.spyOn(drip, "tokenBalance").mockResolvedValue(1_000_000n);
    vi.spyOn(drip, "portfolio").mockResolvedValue(null);
    vi.spyOn(drip.conn, "getBalance").mockResolvedValue(PORTFOLIO_LAMPORTS + 20000 + offset);
    const rent = vi.spyOn(drip.conn, "getMinimumBalanceForRentExemption");
    const build = vi.spyOn(tx, "buildCreatePortfolio").mockRejectedValue(new Error("creation reached"));
    await expect(deposit.run(drip, { amount_usdc: 1 })).rejects.toThrow(offset < 0
      ? `Creating a portfolio costs rent (${PORTFOLIO_LAMPORTS / 1_000_000_000} SOL) plus fees; call claim_test_funds`
      : "creation reached");
    expect(build).toHaveBeenCalledTimes(offset < 0 ? 0 : 1);
    expect(rent).not.toHaveBeenCalled();
  });
});

describe("matcher", () => {
  const state = { paused: false, maxFill: 10_000_000n, inventoryCap: 100_000_000n, inventory: 0n, refPrice: 150_000_000n, refSlot: 123n };
  it("parses unaligned little-endian values and signed inventory", () => {
    const data = Buffer.alloc(352); data.write("DRIPMTC1", 64); data[270] = 1;
    const write128 = (offset: number, value: bigint) => {
      value = BigInt.asUintN(128, value);
      data.writeBigUInt64LE(value & ((1n << 64n) - 1n), offset); data.writeBigUInt64LE(value >> 64n, offset + 8);
    };
    write128(271, state.maxFill); write128(287, state.inventoryCap); write128(303, -42_000_000n);
    data.writeBigUInt64LE(state.refPrice, 329); data.writeBigUInt64LE(state.refSlot, 337);
    expect(parseMatcher(data)).toEqual({ ...state, paused: true, inventory: -42_000_000n });
    expect(() => parseMatcher(data.subarray(0, 345))).toThrow("Invalid matcher context");
  });
  it("rejects paused and oversized fills", () => {
    expect(() => checkMatcher({ ...state, paused: true }, 1n)).toThrow("paused");
    expect(() => checkMatcher(state, 10_000_001n)).toThrow("max fill");
    expect(() => checkMatcher(state, -10_000_001n)).toThrow("max fill");
  });
  it.each([1n, -1n])("enforces inventory rule in direction %s", sign => {
    expect(() => checkMatcher({ ...state, inventory: sign * 100_000_000n }, -sign)).toThrow("inventory cap");
    expect(() => checkMatcher({ ...state, inventory: sign * 110_000_000n }, sign)).not.toThrow();
    expect(() => checkMatcher({ ...state, inventory: sign * 110_000_000n }, -sign)).toThrow("inventory cap");
    expect(() => checkMatcher({ ...state, inventory: sign * 90_000_000n }, -sign * 10_000_000n)).not.toThrow();
  });
  it("refuses equal-risk sign flips above cap", () => {
    expect(() => checkMatcher({ ...state, maxFill: 300_000_000n, inventory: 110_000_000n }, 220_000_000n)).toThrow("inventory cap");
    expect(() => checkMatcher({ ...state, maxFill: 300_000_000n, inventory: -110_000_000n }, -220_000_000n)).toThrow("inventory cap");
  });
});

describe("units and redaction", () => {
  it.each([[1, 1_000_000n], [0.000001, 1n], [0.1, 100_000n], [1.2345674, 1_234_567n], [1.2345676, 1_234_568n]])("converts %s to millionths", (value, expected) => { expect(atoms(value)).toBe(expected); });
  it.each([0, -1, NaN, Infinity, -Infinity, 0.0000001, Number.MAX_SAFE_INTEGER])("rejects invalid amount %s", value => { expect(() => atoms(value)).toThrow("positive and representable"); });
  it("redacts credentials in URL paths, queries and basic auth", () => {
    expect(redactRpc("RPC https://user:password@host/key?api-key=private failed")).toBe("RPC <rpc> failed");
    expect(redactRpc("http://localhost:8899/key")).toBe("<rpc>");
    expect(safeError(new Error(`failed ${secret} https://host/private`), env)).toBe("failed <redacted> <rpc>");
    expect(safeError(new Error(secret), {})).toBe("<redacted>");
    expect(safeError(new Error("Visit https://faucet.solana.com (devnet)"))).toContain("https://faucet.solana.com");
  });
});

describe("sending", () => {
  function setup() {
    const drip = new Drip(env), conn = drip.conn;
    vi.spyOn(conn, "getGenesisHash").mockResolvedValue(DEVNET_GENESIS);
    vi.spyOn(conn, "getLatestBlockhash").mockResolvedValue({ blockhash: Keypair.generate().publicKey.toBase58(), lastValidBlockHeight: 100 });
    const send = vi.spyOn(conn, "sendRawTransaction").mockResolvedValue("test-signature");
    const confirm = vi.spyOn(conn, "confirmTransaction").mockResolvedValue({ context: { slot: 1 }, value: { err: null } });
    const build = vi.fn(async () => [SystemProgram.transfer({ fromPubkey: key.publicKey, toPubkey: Keypair.generate().publicKey, lamports: 1 })]);
    return { drip, conn, send, confirm, build };
  }
  it("rebuilds failed simulations and only sends after success", async () => {
    vi.useFakeTimers();
    const { drip, conn, send, confirm, build } = setup();
    const simulate = vi.spyOn(conn, "simulateTransaction").mockResolvedValueOnce({ context: { slot: 1 }, value: { err: "AccountInUse", logs: [] } }).mockResolvedValue({ context: { slot: 1 }, value: { err: null, logs: [] } });
    const result = drip.send(build); await vi.runAllTimersAsync();
    expect(await result).toEqual({ signature: "test-signature", explorer: "https://explorer.solana.com/tx/test-signature?cluster=devnet" });
    expect(build).toHaveBeenCalledTimes(2); expect(simulate).toHaveBeenCalledTimes(2); expect(send).toHaveBeenCalledTimes(1); expect(confirm).toHaveBeenCalledTimes(1);
  });
  it("never sends when all three simulations fail, and keeps only five log lines", async () => {
    vi.useFakeTimers();
    const { drip, conn, send, build } = setup();
    vi.spyOn(conn, "simulateTransaction").mockResolvedValue({ context: { slot: 1 }, value: { err: "AccountInUse", logs: ["discard-me", "1", "2", "3", "4", "5"] } });
    const result = drip.send(build).catch(error => error as Error); await vi.runAllTimersAsync();
    const error = await result;
    expect(error).toBeInstanceOf(Error); expect((error as Error).message).not.toContain("discard-me");
    expect(build).toHaveBeenCalledTimes(3); expect(send).not.toHaveBeenCalled();
  });
  it("does not retry an uncertain send", async () => {
    const { drip, conn, send, build } = setup();
    vi.spyOn(conn, "simulateTransaction").mockResolvedValue({ context: { slot: 1 }, value: { err: null, logs: [] } });
    send.mockRejectedValue(new Error("network timeout"));
    await expect(drip.send(build)).rejects.toThrow("network timeout");
    expect(build).toHaveBeenCalledTimes(1); expect(send).toHaveBeenCalledTimes(1);
  });
});

describe("MCP", () => {
  it("lists exactly ten tools through an in-memory client without touching chain", async () => {
    const drip = new Drip(env), genesis = vi.spyOn(drip.conn, "getGenesisHash");
    const server = createServer(drip), client = new Client({ name: "test", version: "1" });
    const [a, b] = InMemoryTransport.createLinkedPair();
    await server.connect(a); await client.connect(b);
    try {
      const { tools } = await client.listTools();
      expect(tools.map(tool => tool.name).sort()).toEqual(["perps_markets", "market_stats", "get_position", "deposit", "open_position", "close_position", "withdraw", "trade_history", "leaderboard", "claim_test_funds"].sort());
      expect(tools.map(tool => tool.name)).toEqual(registeredTools.map(tool => tool.name));
      for (const tool of tools) expect(tool.description).toContain("devnet, test tokens");
      expect(genesis).not.toHaveBeenCalled();
    } finally { await client.close(); await server.close(); }
  });
  it("refuses every tool on a non-devnet RPC", async () => {
    const drip = new Drip(env);
    vi.spyOn(drip.conn, "getGenesisHash").mockResolvedValue("mainnet");
    const account = vi.spyOn(drip.conn, "getAccountInfo"), fetch = vi.spyOn(globalThis, "fetch");
    const server = createServer(drip), client = new Client({ name: "test", version: "1" });
    const [a, b] = InMemoryTransport.createLinkedPair(); await server.connect(a); await client.connect(b);
    try {
      for (const tool of (await client.listTools()).tools) {
        const result = await client.callTool({ name: tool.name, arguments: { amount_usdc: 1, side: "long", size: 1 } });
        expect(result.isError).toBe(true); expect(JSON.stringify(result.content)).toContain("Refusing non-devnet");
      }
      expect(account).not.toHaveBeenCalled(); expect(fetch).not.toHaveBeenCalled();
    } finally { await client.close(); await server.close(); }
  });
});

describe("marketOverrides", () => {
  it("keeps the defaults when no DRIP_* market env is set", () => {
    expect(marketOverrides({})).toEqual({});
    const drip = new Drip({});
    expect(drip.cfg.market.toBase58()).toBe(new Drip({ DRIP_MARKET: "" }).cfg.market.toBase58());
  });

  it("defaults to the devnet compute market", () => {
    const cfg = new Drip({}).cfg;
    expect(cfg.market.toBase58()).toBe("c2niKk33fXp5E8a7ZYzQXdqjEXizrSBb55VEuBQiNUU");
    expect(new Drip({}).symbol).toBe("COMPUTE-PERP");
    expect(new Drip({ DRIP_SYMBOL: " SOL-PERP " }).symbol).toBe("SOL-PERP");
    expect(cfg.mint.toBase58()).toBe("4NS7ozkPTEisCTSeigw33vjjWkWZ7NechuPddhXnA5Zf");
    expect(cfg.lpPortfolio.toBase58()).toBe("6T48gQiBPQsYnyf1C5eBkwJ3G7N8Mkxt5Aau3sNrntUu");
    expect(cfg.matcherProgram?.toBase58()).toBe("8era4GFKLUu24TLr1H2Gjqt5csiYj4jhPwoxCXfEDvKE");
    expect(cfg.matcherContext?.toBase58()).toBe("4XZtvznVHA7AdYTiq9jdi8PhL9pXuJEjyaiqQwTTPYfN");
    expect(new Drip({ DRIP_MATCHER_PROGRAM: "8era4GFKLUu24TLr1H2Gjqt5csiYj4jhPwoxCXfEDvKE" }).cfg.matcherContext?.toBase58())
      .toBe("4XZtvznVHA7AdYTiq9jdi8PhL9pXuJEjyaiqQwTTPYfN");
  });

  it("points the MCP at another market from env", () => {
    const [market, mint, lp, ctx, program] = Array.from({ length: 5 }, () => Keypair.generate().publicKey.toBase58());
    const drip = new Drip({ DRIP_MARKET: market, DRIP_MINT: mint, DRIP_LP_PORTFOLIO: lp, DRIP_MATCHER_CONTEXT: ctx, DRIP_MATCHER_PROGRAM: program });
    expect(drip.cfg.market.toBase58()).toBe(market);
    expect(drip.cfg.mint.toBase58()).toBe(mint);
    expect(drip.cfg.lpPortfolio.toBase58()).toBe(lp);
    expect(drip.cfg.matcherContext?.toBase58()).toBe(ctx);
    expect(drip.cfg.matcherProgram?.toBase58()).toBe(program);
  });

  it("disables trading with DRIP_MATCHER_CONTEXT=none and rejects bad keys", () => {
    const drip = new Drip({ DRIP_MATCHER_CONTEXT: "none" });
    expect(drip.cfg.matcherContext).toBeUndefined();
    expect(drip.cfg.matcherProgram).toBeUndefined();
    expect(() => new Drip({ DRIP_MARKET: "not-a-key" })).toThrow("DRIP_MARKET is not a valid public key");
    expect(new Drip({ DRIP_MATCHER_PROGRAM: " none " }).cfg.matcherContext).toBeUndefined();
  });

  it("sets the matcher program and context together", () => {
    const [ctx, program] = Array.from({ length: 2 }, () => Keypair.generate().publicKey);
    const defaults = { matcherProgram: program, matcherContext: ctx };
    const other = Keypair.generate().publicKey.toBase58();
    expect(marketOverrides({ DRIP_MATCHER_PROGRAM: other }, defaults).matcherContext?.toBase58()).toBe(ctx.toBase58());
    expect(marketOverrides({ DRIP_MATCHER_PROGRAM: other }, defaults).matcherProgram?.toBase58()).toBe(other);
    expect(() => marketOverrides({ DRIP_MATCHER_CONTEXT: other }, {})).toThrow("must be set together");
    expect(() => marketOverrides({ DRIP_MATCHER_PROGRAM: other }, {})).toThrow("must be set together");
    expect(marketOverrides({ DRIP_MARKET: ` ${other} ` }).market?.toBase58()).toBe(other);
  });
});
