import { mkdtempSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Keypair, SystemProgram } from "@solana/web3.js";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { atoms, checkMatcher, DEVNET_GENESIS, Drip, genesisGuard, loadKeypair, parseMatcher, redactRpc, safeError } from "./drip";
import { createServer } from "./index";
import { tools as registeredTools } from "./tools";

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
        const result = await client.callTool({ name: tool.name, arguments: { amount_usdc: 1, side: "long", size_sol: 1 } });
        expect(result.isError).toBe(true); expect(JSON.stringify(result.content)).toContain("Refusing non-devnet");
      }
      expect(account).not.toHaveBeenCalled(); expect(fetch).not.toHaveBeenCalled();
    } finally { await client.close(); await server.close(); }
  });
});
