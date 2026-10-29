// Devnet round-trip for the TS client: create portfolio, deposit, open, close,
// withdraw, built by src/chain/ exactly as the browser does (TradeCpi through
// the LP's matcher; the tester is the only signer). Needs a funded throwaway
// wallet and the matcher keys (VITE_MATCHER_PROGRAM / VITE_MATCHER_CONTEXT).
// Everything is simulated first. Reuse a saved portfolio unless
// ROUNDTRIP_ALLOW_CREATE=1 explicitly allows creating one.
//
//   TESTER_KEYPAIR=... VITE_MATCHER_PROGRAM=... VITE_MATCHER_CONTEXT=... npm run roundtrip
import { readFileSync } from "node:fs";
import { ComputeBudgetProgram, Connection, Keypair, Transaction, VersionedTransaction, sendAndConfirmTransaction } from "@solana/web3.js";
import type { PublicKey, SimulatedTransactionResponse, TransactionInstruction } from "@solana/web3.js";
import { devnetConfig as cfg } from "../src/chain/config";
import { parseMarket, parsePortfolio } from "../src/chain/accounts";
import { ata } from "../src/chain/pda";
import { buildClose, buildCreatePortfolio, buildDeposit, buildTrade, buildWithdraw, findPortfolio } from "../src/chain/tx";

const path = process.env.TESTER_KEYPAIR;
if (!path) throw new Error("set TESTER_KEYPAIR");
const tester = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(path, "utf8"))));
const conn = new Connection(process.env.RPC_URL ?? cfg.rpcUrl, "confirmed");
if ((await conn.getGenesisHash()) === "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d") throw new Error("refusing mainnet");
const sigs: Record<string, string> = {};

function checkSimulation(label: string, result: SimulatedTransactionResponse) {
  if (result.err) {
    console.error(label, result.err);
    console.error((result.logs ?? []).slice(-15).join("\n"));
    process.exit(1);
  }
  console.log(label, "simulation OK");
}

async function send(label: string, ixs: TransactionInstruction[], signers: Keypair[] = []) {
  const tx = new Transaction({ feePayer: tester.publicKey, ...(await conn.getLatestBlockhash()) }).add(...ixs);
  const simulation = await conn.simulateTransaction(new VersionedTransaction(tx.compileMessage()), { sigVerify: false });
  checkSimulation(label, simulation.value);
  sigs[label] = await sendAndConfirmTransaction(conn, tx, [tester, ...signers], { commitment: "confirmed" });
  console.log(label, sigs[label]);
}

async function tradeIxs(portfolio: PublicKey, sizeQ: bigint) {
  const mark = parseMarket((await conn.getAccountInfo(cfg.market))!.data).asset0.effectivePrice;
  const limit = sizeQ > 0n ? mark + mark * 50n / 10_000n : mark - mark * 50n / 10_000n; // as the UI: mark +- 0.5%
  return buildTrade(conn, cfg, tester.publicKey, portfolio, sizeQ, limit, 12n);
}

const marketAccount = await conn.getAccountInfo(cfg.market);
if (!marketAccount) throw new Error("Market account not found");
let portfolio = await findPortfolio(conn, cfg.programId, parseMarket(marketAccount.data), tester.publicKey);
let created: Awaited<ReturnType<typeof buildCreatePortfolio>> | undefined;
if (!portfolio) {
  if (process.env.ROUNDTRIP_ALLOW_CREATE !== "1") {
    throw new Error("no saved portfolio for this wallet on this market; set ROUNDTRIP_ALLOW_CREATE=1 to create one");
  }
  created = await buildCreatePortfolio(cfg, tester.publicKey);
  portfolio = created.portfolio.publicKey;
}

function check(label: string, ok: boolean, detail: string) {
  console.log(`${label}: ${ok ? "OK" : "FAIL"} (${detail})`);
  if (!ok) process.exit(1);
}

const sol = await conn.getBalance(tester.publicKey);
check("wallet SOL >= 0.02", sol >= 20_000_000, `${sol} lamports`);
const capital = created ? 0n : parsePortfolio((await conn.getAccountInfo(portfolio))!.data).capital;
const depositNeeded = capital < 10_000_000n ? 10_000_000n - capital : 0n;
if (depositNeeded > 0n) {
  const walletAta = ata(tester.publicKey, cfg.mint);
  const balance = await conn.getAccountInfo(walletAta)
    ? BigInt((await conn.getTokenAccountBalance(walletAta)).value.amount) : 0n;
  check("wallet USDC >= deposit needed", balance >= depositNeeded, `${balance} atoms available, ${depositNeeded} needed`);
} else {
  console.log("wallet USDC / deposit: skipped (capital >= 10 USDC)");
}
const freshMarket = parseMarket((await conn.getAccountInfo(cfg.market))!.data);
const slot = BigInt(await conn.getSlot());
// asset0.slotLast is the asset's funding accrual slot; group currentSlot is shared across assets.
const age = slot - freshMarket.asset0.slotLast;
check("asset0 funding accrual within 32 slots", age >= 0n && age <= 32n, `${age} slots old`);

const withoutBudget = (ixs: TransactionInstruction[]) => ixs.filter(ix => !ix.programId.equals(ComputeBudgetProgram.programId));
// Dry-run deposit + open in one tx (covers matcher ref freshness and capability).
// A brand-new portfolio can't be built against yet; its sends are still simulated one by one.
if (created) console.log("preflight deposit/open_long: skipped (new portfolio)");
else {
  const depositIxs = depositNeeded > 0n ? await buildDeposit(conn, cfg, tester.publicKey, portfolio, depositNeeded) : [];
  const dryRun = new Transaction({ feePayer: tester.publicKey, ...(await conn.getLatestBlockhash()) })
    .add(...withoutBudget(depositIxs), ...await tradeIxs(portfolio, 100_000n));
  checkSimulation("preflight deposit/open_long", (await conn.simulateTransaction(
    new VersionedTransaction(dryRun.compileMessage()), { sigVerify: false })).value);
}

if (created) await send("create_portfolio", created.ixs, [created.portfolio]);
if (depositNeeded > 0n) await send("deposit", await buildDeposit(conn, cfg, tester.publicKey, portfolio, depositNeeded));
await send("open_long", await tradeIxs(portfolio, 100_000n));
await send("close", await buildClose(conn, cfg, tester.publicKey, portfolio, 50n, 12n));
const pf = parsePortfolio((await conn.getAccountInfo(portfolio))!.data);
if (pf.legs.length) throw new Error("position still open after close");
await send("withdraw", await buildWithdraw(conn, cfg, tester.publicKey, portfolio, pf.capital));
console.log(JSON.stringify({ tester: tester.publicKey.toBase58(), portfolio: portfolio.toBase58(), sigs }, null, 2));
