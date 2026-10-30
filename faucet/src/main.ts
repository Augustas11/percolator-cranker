import { readFile, rename, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { Connection, Keypair, PublicKey, SystemProgram, Transaction, VersionedTransaction } from "@solana/web3.js";
import { createAtaIdempotentIx, TOKEN_PROGRAM_ID } from "../../frontend/src/chain/pda";
import { ClaimError, DEFAULT_MINT, DEVNET_GENESIS, Faucet, checkGetBody, checkTokenAccount, mintTo, needsSol, parseState, parseWallet, readBody, type State } from "./faucet";

function integer(name: string, fallback: number, max = Number.MAX_SAFE_INTEGER): number {
  const value = process.env[name] ?? String(fallback);
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(Number(value)) || Number(value) < 1 || Number(value) > max) {
    throw new Error(`Invalid ${name}`);
  }
  return Number(value);
}

const rpcUrl = process.env.FAUCET_RPC_URL ?? "https://api.devnet.solana.com";

async function main() {
  let connection: Connection;
  try {
    if (!["http:", "https:"].includes(new URL(rpcUrl).protocol)) throw new Error();
    connection = new Connection(rpcUrl, {
      commitment: "confirmed",
      disableRetryOnRateLimit: true,
      fetch: (input, init) => {
        const timeout = AbortSignal.timeout(10_000);
        return fetch(input, { ...init, signal: init?.signal ? AbortSignal.any([init.signal, timeout]) : timeout });
      },
    });
  } catch { throw new Error("Invalid FAUCET_RPC_URL"); }
  const keypairPath = process.env.FAUCET_KEYPAIR;
  if (!keypairPath) throw new Error("FAUCET_KEYPAIR is required");
  let payer: Keypair;
  try {
    const bytes: unknown = JSON.parse(await readFile(keypairPath, "utf8"));
    if (!Array.isArray(bytes) || bytes.length !== 64 || bytes.some(b => !Number.isInteger(b) || b < 0 || b > 255)) throw new Error();
    payer = Keypair.fromSecretKey(Uint8Array.from(bytes));
  } catch { throw new Error("Cannot read valid FAUCET_KEYPAIR secret-key array"); }
  let mint: PublicKey;
  try { mint = new PublicKey(process.env.FAUCET_MINT ?? DEFAULT_MINT); }
  catch { throw new Error("Invalid FAUCET_MINT"); }
  const dailyCap = integer("FAUCET_DAILY_CAP", 200);
  const amountUsdc = integer("FAUCET_AMOUNT_USDC", 1000, 100000);
  const minLamports = integer("FAUCET_MIN_PAYER_LAMPORTS", 3_000_000_000);
  const solLamports = integer("FAUCET_SOL_LAMPORTS", 80_000_000);
  const solDailyCap = integer("FAUCET_SOL_DAILY_CAP", 12);
  const bind = process.env.FAUCET_BIND ?? "127.0.0.1:8092";
  const match = /^(\[[\da-fA-F:]+\]|[^\s:/]+):(\d+)$/.exec(bind);
  if (!match || Number(match[2]) < 1 || Number(match[2]) > 65535) throw new Error("Invalid FAUCET_BIND (expected host:port)");
  const statePath = process.env.FAUCET_STATE ?? "faucet-state.json";
  if (!statePath.trim()) throw new Error("Invalid FAUCET_STATE");
  let state: State = { wallets: {}, day: new Date().toISOString().slice(0, 10), count: 0, sol_count: 0 };
  try { state = parseState(JSON.parse(await readFile(statePath, "utf8"))); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw new Error("Cannot read valid FAUCET_STATE");
  }
  let genesis: string;
  let mintAccount;
  let payerLamports;
  try {
    genesis = await connection.getGenesisHash();
    mintAccount = await connection.getAccountInfo(mint);
    payerLamports = await connection.getBalance(payer.publicKey);
  } catch { throw new Error("Startup RPC check failed (<rpc>)"); }
  if (genesis !== DEVNET_GENESIS) throw new Error("RPC genesis is not devnet");
  if (!mintAccount || !mintAccount.owner.equals(TOKEN_PROGRAM_ID) || mintAccount.data.length !== 82 ||
      mintAccount.data.readUInt32LE(0) !== 1 || !mintAccount.data.subarray(4, 36).equals(payer.publicKey.toBuffer()) ||
      mintAccount.data[44] !== 6 || mintAccount.data[45] !== 1) {
    throw new Error("Mint must be initialized classic SPL Token with 6 decimals and payer as mint authority");
  }
  const faucet = new Faucet(state, {
    mint, dailyCap, amountUsdc, solDailyCap, solLamports, minPayerLamports: minLamports, payerLamports, now: () => Math.floor(Date.now() / 1000),
    save: async value => {
      await writeFile(`${statePath}.tmp`, JSON.stringify(value), { mode: 0o600 });
      await rename(`${statePath}.tmp`, statePath);
    },
    send: async (wallet, destination, allowSol) => {
      let submitted = false;
      let dripLamports = 0;
      try {
        const payerBalance = await connection.getBalance(payer.publicKey);
        faucet.observePayer(payerBalance);
        const walletBalance = await connection.getBalance(wallet);
        dripLamports = needsSol(walletBalance, payerBalance, solLamports, minLamports, allowSol) ? solLamports : 0;
        const account = await connection.getAccountInfo(destination);
        const instructions = [];
        if (dripLamports && !account) instructions.push(createAtaIdempotentIx(payer.publicKey, wallet, mint));
        else checkTokenAccount(account, wallet, mint);
        if (dripLamports) instructions.push(SystemProgram.transfer({ fromPubkey: payer.publicKey, toPubkey: wallet, lamports: dripLamports }));
        instructions.push(mintTo(mint, destination, payer.publicKey, amountUsdc));
        const block = await connection.getLatestBlockhash();
        const tx = new VersionedTransaction(new Transaction({ feePayer: payer.publicKey, ...block })
          .add(...instructions).compileMessage());
        tx.sign([payer]);
        const simulation = await connection.simulateTransaction(tx, { sigVerify: true, commitment: "confirmed" });
        if (simulation.value.err) throw new ClaimError(503, "Mint transaction simulation failed");
        const raw = tx.serialize();
        submitted = true;
        const signature = await connection.sendRawTransaction(raw, { skipPreflight: false, maxRetries: 3 });
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), 60_000);
        try {
          const confirmation = await connection.confirmTransaction({ signature, ...block, abortSignal: controller.signal }, "confirmed");
          if (confirmation.value.err) throw new ClaimError(503, "Mint transaction was not confirmed");
        } finally { clearTimeout(timer); }
        try { faucet.observePayer(await connection.getBalance(payer.publicKey)); }
        catch { faucet.observePayer(payerBalance - dripLamports); }
        return { signature, solLamports: dripLamports };
      } catch (error) {
        if (submitted) throw new ClaimError(503, "Mint outcome unknown", undefined, true, dripLamports);
        if (error instanceof ClaimError) throw error;
        throw new ClaimError(503, "RPC unavailable or transaction not confirmed");
      }
    },
  });
  const server = createServer(async (req, res) => {
    res.setHeader("Content-Type", "application/json");
    let isClaim = false;
    let walletLog = "-";
    try {
      checkGetBody(req.method, req.headers);
      const url = new URL(req.url ?? "/", "http://localhost");
      isClaim = url.pathname === "/claim";
      if (!isClaim && url.pathname !== "/health") throw new ClaimError(404, "Not found");
      if (url.pathname === "/health") {
        if (req.method !== "GET") throw new ClaimError(405, "Method not allowed");
        res.end(JSON.stringify(faucet.health()));
        return;
      }
      if (req.method !== "GET" && req.method !== "POST") throw new ClaimError(405, "Method not allowed");
      let body: unknown;
      if (req.method === "POST") {
        body = await readBody(req.iterator({ destroyOnReturn: false }));
        if (req.headers["content-type"]?.split(";")[0].trim().toLowerCase() !== "application/json") {
          throw new ClaimError(400, "Expected application/json");
        }
        if (url.search) throw new ClaimError(400, "Unexpected query fields");
      } else {
        if ([...url.searchParams.keys()].length !== 1 || !url.searchParams.has("wallet")) throw new ClaimError(400, "Expected only a wallet field");
        body = { wallet: url.searchParams.get("wallet") };
      }
      const wallet = parseWallet(body);
      walletLog = wallet.toBase58();
      const result = await faucet.claim(wallet);
      console.error(`${new Date().toISOString()} ${walletLog} ${result.signature}`);
      res.end(JSON.stringify(result));
    } catch (error) {
      const failure = error instanceof ClaimError ? error : new ClaimError(503, "Faucet unavailable");
      if (isClaim) console.error(`${new Date().toISOString()} ${walletLog} error: ${failure.message}`);
      res.statusCode = failure.status;
      if (failure.status === 413) { res.setHeader("Connection", "close"); req.resume(); }
      res.end(JSON.stringify({ error: failure.message, ...(failure.nextClaimAfter === undefined ? {} : { next_claim_after: failure.nextClaimAfter }) }));
    }
  });
  server.requestTimeout = 10_000;
  server.headersTimeout = 10_000;
  server.maxHeadersCount = 20;
  server.on("error", () => { console.error("Faucet HTTP server failed"); process.exit(1); });
  server.listen(Number(match[2]), match[1].replace(/^\[|\]$/g, ""), () => {
    console.error(`Faucet payer: ${payer.publicKey.toBase58()}`);
  });
}

main().catch(error => {
  console.error((error instanceof Error ? error.message : "Faucet startup failed").split(rpcUrl).join("<rpc>"));
  process.exitCode = 1;
});
