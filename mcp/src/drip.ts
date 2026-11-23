import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { Connection, Keypair, PublicKey, Transaction, VersionedTransaction } from "@solana/web3.js";
import type { TransactionInstruction } from "@solana/web3.js";
import { devnetConfig } from "../../frontend/src/chain/config";
import { parseMarket, parsePortfolio } from "../../frontend/src/chain/accounts";
import { i128, u64, u128 } from "../../frontend/src/chain/bytes";
import { ata } from "../../frontend/src/chain/pda";
import { findPortfolio } from "../../frontend/src/chain/tx";

export const DEVNET_GENESIS = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";
export const units = (value: bigint) => Number(value) / 1e6;
const abs = (value: bigint) => value < 0n ? -value : value;

export function atoms(value: number, name = "amount") {
  const rounded = Math.round(value * 1e6);
  if (!Number.isFinite(value) || value <= 0 || !Number.isSafeInteger(rounded) || rounded <= 0) {
    throw new Error(`${name} must be positive and representable in millionths`);
  }
  return BigInt(rounded);
}

export function redactRpc(value: string) {
  return value.replace(/https?:\/\/[^\s<>"']+/gi, url => url === "https://faucet.solana.com" ? url : "<rpc>");
}

export function safeError(error: unknown, env: NodeJS.ProcessEnv = process.env) {
  let message = error instanceof Error ? error.message : "Tool failed";
  for (const secret of [env.DRIP_SECRET_KEY, env.DRIP_KEYPAIR]) {
    if (secret) message = message.split(secret).join("<redacted>");
  }
  return redactRpc(message).replace(/\[\s*\d+(?:\s*,\s*\d+){31,}\s*\]/g, "<redacted>").slice(0, 2000);
}

export function loadKeypair(env: NodeJS.ProcessEnv = process.env, home = homedir()) {
  const decode = (json: string) => {
    try {
      const data: unknown = JSON.parse(json);
      if (!Array.isArray(data) || data.length !== 64 || !data.every(n => Number.isInteger(n) && n >= 0 && n <= 255)) throw new Error();
      return Keypair.fromSecretKey(Uint8Array.from(data));
    } catch { throw new Error("Invalid agent keypair: expected a Solana JSON secret-key array"); }
  };
  const read = (path: string) => {
    let json: string;
    try { json = readFileSync(path, "utf8"); }
    catch { throw new Error("Cannot read agent keypair file"); }
    return decode(json);
  };
  if (env.DRIP_KEYPAIR) return { keypair: read(env.DRIP_KEYPAIR), generated: false };
  if (env.DRIP_SECRET_KEY) return { keypair: decode(env.DRIP_SECRET_KEY), generated: false };
  const path = join(home, ".config/drip/devnet-agent.json");
  try { return { keypair: decode(readFileSync(path, "utf8")), generated: false }; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw new Error("Cannot load default agent keypair");
  }
  const keypair = Keypair.generate();
  try {
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    writeFileSync(path, JSON.stringify(Array.from(keypair.secretKey)), { flag: "wx", mode: 0o600 });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "EEXIST") return { keypair: read(path), generated: false };
    throw new Error("Cannot save default agent keypair");
  }
  return { keypair, generated: true };
}

export function genesisGuard(conn: Pick<Connection, "getGenesisHash">) {
  let checked: Promise<void> | undefined;
  return () => checked ??= conn.getGenesisHash().then(hash => {
    if (hash !== DEVNET_GENESIS) throw new Error("Refusing non-devnet RPC: devnet, test tokens only");
  }, error => {
    checked = undefined;
    throw error;
  });
}

export function parseMatcher(data: Uint8Array) {
  if (data.length < 352 || Buffer.from(data.subarray(64, 72)).toString() !== "DRIPMTC1") throw new Error("Invalid matcher context");
  return {
    paused: data[270] !== 0, maxFill: u128(data, 271), inventoryCap: u128(data, 287),
    inventory: i128(data, 303), refPrice: u64(data, 329), refSlot: u64(data, 337),
  };
}

export function checkMatcher(state: ReturnType<typeof parseMatcher>, signedSize: bigint) {
  if (state.paused) throw new Error("Matcher is paused");
  if (abs(signedSize) > state.maxFill) throw new Error(`Size exceeds matcher max fill of ${units(state.maxFill)} SOL`);
  const next = state.inventory - signedSize;
  if (abs(next) > state.inventoryCap && abs(next) >= abs(state.inventory)) throw new Error("Trade would exceed the LP inventory cap without reducing risk");
}

/// The built-in market is the devnet compute perp (FLOP oracle). The frontend
/// config still defaults to the retired SOL-PERP market, so override it here.
export const computeMarket = {
  market: new PublicKey("c2niKk33fXp5E8a7ZYzQXdqjEXizrSBb55VEuBQiNUU"),
  mint: new PublicKey("4NS7ozkPTEisCTSeigw33vjjWkWZ7NechuPddhXnA5Zf"),
  lpPortfolio: new PublicKey("6T48gQiBPQsYnyf1C5eBkwJ3G7N8Mkxt5Aau3sNrntUu"),
  matcherProgram: new PublicKey("8era4GFKLUu24TLr1H2Gjqt5csiYj4jhPwoxCXfEDvKE"),
  matcherContext: new PublicKey("4XZtvznVHA7AdYTiq9jdi8PhL9pXuJEjyaiqQwTTPYfN"),
};
export const marketDefaults = { ...devnetConfig, ...computeMarket };

/// Market addresses from DRIP_* env, over the frontend defaults. The defaults are
/// baked at build time (import.meta.env is empty under node), so a market relaunch
/// would otherwise need a code change. A relaunch sets the full set (market, mint,
/// LP portfolio, matcher pair). DRIP_MATCHER_PROGRAM / DRIP_MATCHER_CONTEXT = none
/// disables trading; the matcher program and context are set together or not at all.
export function marketOverrides(env: NodeJS.ProcessEnv, defaults: Partial<typeof devnetConfig> = marketDefaults) {
  const raw = (name: string) => env[name]?.trim() || undefined;
  const key = (name: string) => {
    const value = raw(name);
    if (!value) return undefined;
    try { return new PublicKey(value); } catch { throw new Error(`${name} is not a valid public key`); }
  };
  const out: Partial<typeof devnetConfig> = {};
  const programId = key("DRIP_PROGRAM_ID"), market = key("DRIP_MARKET");
  const mint = key("DRIP_MINT"), lpPortfolio = key("DRIP_LP_PORTFOLIO");
  if (programId) out.programId = programId;
  if (market) out.market = market;
  if (mint) out.mint = mint;
  if (lpPortfolio) out.lpPortfolio = lpPortfolio;
  const ctx = raw("DRIP_MATCHER_CONTEXT"), program = raw("DRIP_MATCHER_PROGRAM");
  if (ctx === "none" || program === "none") {
    out.matcherProgram = undefined;
    out.matcherContext = undefined;
  } else if (ctx || program) {
    const matcherContext = ctx ? key("DRIP_MATCHER_CONTEXT") : defaults.matcherContext;
    const matcherProgram = program ? key("DRIP_MATCHER_PROGRAM") : defaults.matcherProgram;
    if (!matcherContext || !matcherProgram) {
      throw new Error("DRIP_MATCHER_PROGRAM and DRIP_MATCHER_CONTEXT must be set together");
    }
    out.matcherContext = matcherContext;
    out.matcherProgram = matcherProgram;
  }
  return out;
}

export class Drip {
  readonly cfg;
  readonly conn: Connection;
  readonly apiUrl: string;
  readonly symbol: string;
  readonly guard;
  private agent?: ReturnType<typeof loadKeypair>;

  constructor(private env: NodeJS.ProcessEnv = process.env) {
    this.cfg = { ...marketDefaults, ...marketOverrides(env), rpcUrl: env.DRIP_RPC_URL || "https://api.devnet.solana.com" };
    this.symbol = env.DRIP_SYMBOL?.trim() || "COMPUTE-PERP";
    this.apiUrl = (env.DRIP_API_URL || "https://drip-devnet.vercel.app").replace(/\/$/, "");
    this.conn = new Connection(this.cfg.rpcUrl, "confirmed");
    this.guard = genesisGuard(this.conn);
  }

  get wallet() { return (this.agent ??= loadKeypair(this.env)).keypair; }
  get generatedWallet() { return this.agent?.generated ? { generated_wallet: this.wallet.publicKey.toBase58(), keypair_saved: "~/.config/drip/devnet-agent.json" } : {}; }

  async account(key: PublicKey) {
    const account = await this.conn.getAccountInfo(key);
    if (!account) throw new Error("Required chain account not found");
    return account;
  }
  async market() { return parseMarket((await this.account(this.cfg.market)).data); }
  async portfolio() {
    const key = await findPortfolio(this.conn, this.cfg.programId, await this.market(), this.wallet.publicKey);
    return key ? { key, ...parsePortfolio((await this.account(key)).data) } : null;
  }
  async requirePortfolio() {
    const pf = await this.portfolio();
    if (!pf) throw new Error("No portfolio; deposit test USDC first (portfolio rent is paid from your SOL)");
    return pf;
  }
  async tokenBalance() {
    const key = ata(this.wallet.publicKey, this.cfg.mint);
    return await this.conn.getAccountInfo(key) ? BigInt((await this.conn.getTokenAccountBalance(key)).value.amount) : 0n;
  }
  async matcher() {
    if (!this.cfg.matcherContext) throw new Error("Matcher context is not configured");
    const state = parseMatcher((await this.account(this.cfg.matcherContext)).data);
    return { state, output: {
      paused: state.paused, max_fill: units(state.maxFill), max_fill_atoms: state.maxFill.toString(),
      lp_inventory: units(state.inventory), inventory_atoms: state.inventory.toString(),
      inventory_cap: units(state.inventoryCap), inventory_cap_atoms: state.inventoryCap.toString(),
      ref_price: units(state.refPrice), ref_age_slots: (BigInt(await this.conn.getSlot()) - state.refSlot).toString(),
    } };
  }

  async send(build: () => Promise<TransactionInstruction[]>, signers: Keypair[] = []) {
    await this.guard();
    let tx: Transaction | undefined;
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        const ixs = await build();
        const candidate = new Transaction({ feePayer: this.wallet.publicKey, ...await this.conn.getLatestBlockhash() }).add(...ixs);
        const result = await this.conn.simulateTransaction(new VersionedTransaction(candidate.compileMessage()), { sigVerify: false });
        if (result.value.err) throw new Error(`Simulation failed: ${JSON.stringify(result.value.err)}\n${(result.value.logs ?? []).slice(-5).join("\n")}`);
        tx = candidate;
        break;
      } catch (error) {
        if (attempt === 2) throw error;
        await new Promise(resolve => setTimeout(resolve, 2000));
      }
    }
    if (!tx) throw new Error("Simulation failed");
    tx.sign(this.wallet, ...signers);
    const signature = await this.conn.sendRawTransaction(tx.serialize());
    const result = await this.conn.confirmTransaction({ signature, blockhash: tx.recentBlockhash!, lastValidBlockHeight: tx.lastValidBlockHeight! }, "confirmed");
    if (result.value.err) throw new Error(`Transaction ${signature} failed: ${JSON.stringify(result.value.err)}`);
    return { signature, explorer: `https://explorer.solana.com/tx/${signature}?cluster=devnet` };
  }

  async fetchJson<T>(path: string, body?: unknown): Promise<T> {
    const response = await fetch(this.apiUrl + path, {
      signal: AbortSignal.timeout(10_000), ...(body === undefined ? {} : {
        method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
      }),
    });
    if (response.status === 404) throw new Error(`drip API endpoint ${path} not available yet`);
    const json = await response.json();
    if (!response.ok) throw new Error(typeof json?.error === "string" ? json.error : `API returned ${response.status}`);
    return json as T;
  }

}
