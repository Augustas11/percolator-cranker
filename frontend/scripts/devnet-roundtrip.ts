// Devnet round-trip for the TS client: create portfolio, deposit, open, close,
// withdraw, built by src/chain/ exactly as the browser does (TradeCpi through
// the LP's matcher; the tester is the only signer). Needs a funded throwaway
// wallet and the matcher keys (VITE_MATCHER_PROGRAM / VITE_MATCHER_CONTEXT).
//
//   TESTER_KEYPAIR=... VITE_MATCHER_PROGRAM=... VITE_MATCHER_CONTEXT=... npm run roundtrip
import { readFileSync } from "node:fs";
import { Connection, Keypair, Transaction, sendAndConfirmTransaction } from "@solana/web3.js";
import type { PublicKey, TransactionInstruction } from "@solana/web3.js";
import { devnetConfig as cfg } from "../src/chain/config";
import { parseMarket, parsePortfolio } from "../src/chain/accounts";
import { createAtaIdempotentIx } from "../src/chain/pda";
import { buildClose, buildCreatePortfolio, buildDeposit, buildTrade, buildWithdraw, findPortfolio } from "../src/chain/tx";

const path = process.env.TESTER_KEYPAIR;
if (!path) throw new Error("set TESTER_KEYPAIR");
const tester = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(path, "utf8"))));
const conn = new Connection(process.env.RPC_URL ?? cfg.rpcUrl, "confirmed");
if ((await conn.getGenesisHash()) === "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d") throw new Error("refusing mainnet");
const sigs: Record<string, string> = {};

async function send(label: string, ixs: TransactionInstruction[], signers: Keypair[] = []) {
  sigs[label] = await sendAndConfirmTransaction(conn, new Transaction().add(...ixs), [tester, ...signers], { commitment: "confirmed" });
  console.log(label, sigs[label]);
}

async function trade(label: string, portfolio: PublicKey, sizeQ: bigint) {
  const mark = parseMarket((await conn.getAccountInfo(cfg.market))!.data).asset0.effectivePrice;
  const limit = sizeQ > 0n ? mark + mark * 50n / 10_000n : mark - mark * 50n / 10_000n; // as the UI: mark +- 0.5%
  await send(label, await buildTrade(conn, cfg, tester.publicKey, portfolio, sizeQ, limit, 12n));
}

const marketAccount = await conn.getAccountInfo(cfg.market);
if (!marketAccount) throw new Error("Market account not found");
let portfolio = await findPortfolio(conn, cfg.programId, parseMarket(marketAccount.data), tester.publicKey);
if (!portfolio) {
  const created = await buildCreatePortfolio(cfg, tester.publicKey);
  await send("create_portfolio", created.ixs, [created.portfolio]);
  portfolio = created.portfolio.publicKey;
}
await send("deposit", [createAtaIdempotentIx(tester.publicKey, tester.publicKey, cfg.mint),
  ...await buildDeposit(conn, cfg, tester.publicKey, portfolio, 10_000_000n)]);
await trade("open_long", portfolio, 100_000n); // 0.1 SOL
await send("close", await buildClose(conn, cfg, tester.publicKey, portfolio, 50n, 12n));
const pf = parsePortfolio((await conn.getAccountInfo(portfolio))!.data);
if (pf.legs.length) throw new Error("position still open after close");
await send("withdraw", await buildWithdraw(conn, cfg, tester.publicKey, portfolio, pf.capital));
console.log(JSON.stringify({ tester: tester.publicKey.toBase58(), portfolio: portfolio.toBase58(), sigs }, null, 2));
