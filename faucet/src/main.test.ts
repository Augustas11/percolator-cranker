import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Keypair, PublicKey } from "@solana/web3.js";
import { TOKEN_PROGRAM_ID } from "../../frontend/src/chain/pda";
import { DEFAULT_MINT, DEVNET_GENESIS, WINDOW } from "./faucet";

const mocks = vi.hoisted(() => ({
  rpc: {
    getGenesisHash: vi.fn(), getAccountInfo: vi.fn(), getBalance: vi.fn(),
    getLatestBlockhash: vi.fn(), simulateTransaction: vi.fn(),
    sendRawTransaction: vi.fn(), confirmTransaction: vi.fn(),
  },
  connection: vi.fn(), readFile: vi.fn(), writeFile: vi.fn(), rename: vi.fn(),
  handler: undefined as any,
}));
vi.mock("@solana/web3.js", async importOriginal => ({
  ...await importOriginal<typeof import("@solana/web3.js")>(),
  Connection: class { constructor(...args: unknown[]) { mocks.connection(...args); return mocks.rpc; } },
}));
vi.mock("node:fs/promises", () => ({ readFile: mocks.readFile, writeFile: mocks.writeFile, rename: mocks.rename }));
vi.mock("node:http", () => ({
  createServer: (handler: unknown) => {
    mocks.handler = handler;
    return { on: vi.fn(), listen: vi.fn() };
  },
}));

const payer = Keypair.generate();
const wallet = Keypair.generate().publicKey;
const other = Keypair.generate().publicKey;
let log: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  vi.resetModules();
  vi.resetAllMocks();
  mocks.handler = undefined;
  vi.stubEnv("FAUCET_RPC_URL", "https://secret.example/private-token");
  vi.stubEnv("FAUCET_KEYPAIR", "test-keypair");
  vi.stubEnv("FAUCET_DAILY_CAP", "1");
  mocks.readFile.mockImplementation(async (path: string) => {
    if (path === "test-keypair") return JSON.stringify([...payer.secretKey]);
    throw Object.assign(new Error("missing"), { code: "ENOENT" });
  });
  mocks.rpc.getGenesisHash.mockResolvedValue(DEVNET_GENESIS);
  const data = Buffer.alloc(82);
  data.writeUInt32LE(1, 0);
  payer.publicKey.toBuffer().copy(data, 4);
  data[44] = 6;
  data[45] = 1;
  mocks.rpc.getAccountInfo.mockResolvedValueOnce({ owner: TOKEN_PROGRAM_ID, data });
  const token = Buffer.alloc(165);
  new PublicKey(DEFAULT_MINT).toBuffer().copy(token, 0);
  wallet.toBuffer().copy(token, 32);
  token[108] = 1;
  mocks.rpc.getAccountInfo.mockResolvedValue({ owner: TOKEN_PROGRAM_ID, data: token });
  mocks.rpc.getBalance.mockResolvedValue(4_000_000_000);
  mocks.rpc.getLatestBlockhash.mockResolvedValue({ blockhash: Keypair.generate().publicKey.toBase58(), lastValidBlockHeight: 100 });
  mocks.rpc.simulateTransaction.mockResolvedValue({ value: { err: null } });
  mocks.rpc.sendRawTransaction.mockResolvedValue("signature");
  mocks.rpc.confirmTransaction.mockResolvedValue({ value: { err: null } });
  log = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  process.exitCode = 0;
});

async function start() {
  await import("./main");
  await vi.waitFor(() => expect(mocks.handler).toBeDefined());
}
async function claim(key = wallet, headers = {}) {
  const res = { statusCode: 200, setHeader: vi.fn(), end: vi.fn() };
  await mocks.handler({ method: "GET", url: `/claim?wallet=${key.toBase58()}`, headers, resume: vi.fn() }, res);
  return { status: res.statusCode, ...JSON.parse(res.end.mock.calls[0][0]) };
}

it("releases pre-submission RPC failures without exposing the URL", async () => {
  await start();
  mocks.rpc.getBalance.mockRejectedValueOnce(new Error(process.env.FAUCET_RPC_URL));
  expect(await claim()).toMatchObject({ status: 503, error: "RPC unavailable or transaction not confirmed" });
  expect(await claim()).toMatchObject({ status: 200 });
  expect(JSON.stringify(log.mock.calls)).not.toContain(process.env.FAUCET_RPC_URL);
});

it.each(["send", "confirm", "confirmation error"])("preserves ambiguous outcomes after %s failure", async failure => {
  await start();
  if (failure === "send") mocks.rpc.sendRawTransaction.mockRejectedValueOnce(new Error("network"));
  else if (failure === "confirm") mocks.rpc.confirmTransaction.mockRejectedValueOnce(new Error("network"));
  else mocks.rpc.confirmTransaction.mockResolvedValueOnce({ value: { err: "failed" } });
  const result = await claim();
  expect(result).toMatchObject({ status: 503, next_claim_after: expect.any(Number) });
  expect(result.next_claim_after).toBeGreaterThanOrEqual(Math.floor(Date.now() / 1000) + WINDOW - 1);
  expect(await claim()).toMatchObject({ status: 429, next_claim_after: result.next_claim_after });
  expect(await claim(other)).toMatchObject({ status: 429, error: "Global daily cap reached" });
  expect(mocks.rpc.sendRawTransaction).toHaveBeenCalledTimes(1);
});

it("times out confirmation at 60 seconds and keeps the reservation", async () => {
  await start();
  vi.useFakeTimers();
  mocks.rpc.confirmTransaction.mockReturnValueOnce(new Promise(() => {}));
  const result = claim();
  await vi.advanceTimersByTimeAsync(59_999);
  expect(mocks.rpc.confirmTransaction).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(await result).toMatchObject({ status: 503, next_claim_after: expect.any(Number) });
  expect(await claim()).toMatchObject({ status: 429 });
});

it("rejects GET bodies before starting a claim", async () => {
  await start();
  expect(await claim(wallet, { "content-length": "1" })).toMatchObject({ status: 413 });
  expect(await claim(wallet, { "transfer-encoding": "chunked" })).toMatchObject({ status: 413 });
  expect(mocks.rpc.getBalance).not.toHaveBeenCalled();
});

it("sanitizes Connection construction failures at startup", async () => {
  mocks.connection.mockImplementation(() => { throw new Error(process.env.FAUCET_RPC_URL); });
  await import("./main");
  await vi.waitFor(() => expect(process.exitCode).toBe(1));
  expect(mocks.handler).toBeUndefined();
  expect(JSON.stringify(log.mock.calls)).not.toContain(process.env.FAUCET_RPC_URL);
});
