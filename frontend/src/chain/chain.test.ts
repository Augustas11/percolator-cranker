// @vitest-environment node
import { readFileSync } from "node:fs";
import { PublicKey, SystemInstruction } from "@solana/web3.js";
import { describe, expect, it } from "vitest";
import instructions from "./fixtures/instructions.json";
import accounts from "./fixtures/accounts.json";
import pdas from "./fixtures/pdas.json";
import { vaultAuthority, vault, matcherDelegate } from "./pda";
import { effectiveAbsQ, parseMarket, parsePortfolio, parseWrapperConfig, POSITION_EPOCH_MAX } from "./accounts";
import { Writer, u8, u16, u32, u64, u128, i128 } from "./bytes";
import * as ix from "./instructions";
import { layout } from "./layout";

const marketBytes = readFileSync(new URL("./fixtures/market.bin", import.meta.url));
const encoders = {
  InitPortfolio: ix.encodeInitPortfolio,
  Deposit: ix.encodeDeposit,
  Withdraw: ix.encodeWithdraw,
  PermissionlessCrank: ix.encodePermissionlessCrank,
  TradeNoCpi: ix.encodeTradeNoCpi,
  TradeCpi: ix.encodeTradeCpi,
  ClosePortfolio: ix.encodeClosePortfolio,
  CloseResolved: ix.encodeCloseResolved,
};

it("matches Rust vault and matcher PDAs", () => {
  const program = new PublicKey(pdas.program), market = new PublicKey(pdas.market);
  expect(vaultAuthority(program, market).toBase58()).toBe(pdas.vaultAuthority);
  expect(vault(program, market, new PublicKey(pdas.mint)).toBase58()).toBe(pdas.vault);
  const m = pdas.matcherDelegate;
  expect(matcherDelegate(program, market, new PublicKey(m.makerAccount), new PublicKey(m.makerOwner),
    new PublicKey(m.matcherProgram), new PublicKey(m.matcherContext)).toBase58()).toBe(m.delegate);
});

// Normalize only at the assertion boundary to compare with the Rust JSON output.
function jsonValue(value: unknown): unknown {
  if (value instanceof PublicKey) return Buffer.from(value.toBytes()).toString("hex");
  if (value instanceof Uint8Array) return Buffer.from(value).toString("hex");
  if (typeof value === "bigint") return value.toString();
  if (Array.isArray(value)) return value.map(jsonValue);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, v]) => [key, jsonValue(v)]));
  }
  return value;
}

describe("Rust instruction fixtures", () => {
  for (const name of Object.keys(encoders) as (keyof typeof encoders)[]) {
    it(name, () => {
      const fixture = instructions[name];
      const args = JSON.parse(JSON.stringify(fixture.args), (_key, value: unknown) =>
        typeof value === "string" ? BigInt(value) : value,
      );
      expect(Buffer.from(encoders[name](args)).toString("hex")).toBe(fixture.hex);
    });
  }
});

describe("Rust account fixtures", () => {
  it("parses the market and wrapper config field by field", () => {
    const { config, asset0, ...market } = parseMarket(marketBytes);
    const { lifecycle, ...asset } = asset0;
    expect(jsonValue({ ...config, ...market, asset0: asset })).toEqual(accounts.market);
    expect(jsonValue(parseWrapperConfig(marketBytes))).toEqual(jsonValue(config));
    expect(lifecycle).toBe(marketBytes[layout.ASSET0_SLOT_OFF + layout.ASSET_ORACLE_WRAPPER_LEN + layout["slot.asset"] + layout["asset.lifecycle"]]);
  });

  for (const name of ["portfolio", "lp_portfolio"] as const) {
    it(`parses ${name} including active legs`, () => {
      const data = readFileSync(new URL(`./fixtures/${name}.bin`, import.meta.url));
      const parsed = parsePortfolio(data);
      expect(jsonValue({ ...parsed, legs: parsed.legs.map(leg => ({ ...leg, side: leg.side === "long" ? 0 : 1 })) })).toEqual(accounts[name]);
    });
  }

  it("rejects mismatched account kinds", () => {
    expect(() => parsePortfolio(marketBytes)).toThrow(/kind/);
    const portfolio = readFileSync(new URL("./fixtures/portfolio.bin", import.meta.url));
    expect(() => parseMarket(portfolio)).toThrow(/kind/);
    expect(() => parseWrapperConfig(portfolio)).toThrow(/kind/);
  });

  it("rejects invalid magic, version and truncated accounts", () => {
    const badMagic = Uint8Array.from(marketBytes);
    badMagic[0] ^= 1;
    expect(() => parseMarket(badMagic)).toThrow(/magic/);
    const badVersion = Uint8Array.from(marketBytes);
    badVersion[8] = 15;
    expect(() => parseMarket(badVersion)).toThrow(/version/);
    expect(() => parseMarket(marketBytes.subarray(0, 10))).toThrow(/short/);
    expect(() => parseMarket(marketBytes.subarray(0, 100))).toThrow(/short/);
    expect(() => parseWrapperConfig(marketBytes.subarray(0, 100))).toThrow(/short/);
    const portfolio = readFileSync(new URL("./fixtures/portfolio.bin", import.meta.url));
    expect(() => parsePortfolio(portfolio.subarray(0, -1))).toThrow(/short/);
  });

  it("masks matcher flags and fee caps out of positionEpoch", () => {
    const data = Uint8Array.from(readFileSync(new URL("./fixtures/portfolio.bin", import.meta.url)));
    new DataView(data.buffer).setBigUint64(layout.PORTFOLIO_MATCHER_CONTROL_OFF, (0x3fffn << 50n) | (POSITION_EPOCH_MAX << 1n) | 1n, true);
    expect(parsePortfolio(data).positionEpoch).toBe(POSITION_EPOCH_MAX);
  });
});

describe("integer bytes", () => {
  it("round trips bounds through unaligned sliced buffers", () => {
    const encoded = new Writer().u8(255).u16(65535).u32(0xffffffff).u64((1n << 64n) - 1n)
      .u128((1n << 128n) - 1n).i128(-(1n << 127n)).i128((1n << 127n) - 1n).finish();
    const backing = new Uint8Array(encoded.length + 5);
    backing.set(encoded, 3);
    const data = backing.subarray(3, 3 + encoded.length);
    expect(u8(data, 0)).toBe(255);
    expect(u16(data, 1)).toBe(65535);
    expect(u32(data, 3)).toBe(0xffffffff);
    expect(u64(data, 7)).toBe((1n << 64n) - 1n);
    expect(u128(data, 15)).toBe((1n << 128n) - 1n);
    expect(i128(data, 31)).toBe(-(1n << 127n));
    expect(i128(data, 47)).toBe((1n << 127n) - 1n);
  });

  it("rejects values outside each integer range", () => {
    for (const [method, bits] of [["u8", 8], ["u16", 16], ["u32", 32], ["u64", 64], ["u128", 128]] as const) {
      expect(() => new Writer()[method](-1)).toThrow(RangeError);
      expect(() => new Writer()[method](1n << BigInt(bits))).toThrow(RangeError);
    }
    expect(() => new Writer().i128(-(1n << 127n) - 1n)).toThrow(RangeError);
    expect(() => new Writer().i128(1n << 127n)).toThrow(RangeError);
    for (const value of [1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
      expect(() => new Writer().u64(value)).toThrow(RangeError);
    }
    expect(() => u64(new Uint8Array(7), 0)).toThrow(RangeError);
    expect(() => ix.encodePermissionlessCrank({ nowSlot: 0n, observations: Array.from({ length: 256 }, () => ({ assetIndex: 0, oracleAccounts: 0 })) })).toThrow(RangeError);
  });
});

describe("instruction account metas", () => {
  const [program, owner, market, portfolio, ownerAta, vault, authority, ownerB, portfolioB, oracle] =
    Array.from({ length: 10 }, (_, i) => new PublicKey(new Uint8Array(32).fill(i + 1)));
  const funds = { portfolioId: 7n, expectedSequence: 2n, amount: 100n };
  const close = { portfolioId: 7n, expectedSequence: 2n, positionEpoch: 3n };
  const crank = { nowSlot: 1n, observations: [] };

  it("keeps the tester as the only TradeCpi signer and bounds readonly tail accounts", () => {
    const args = { accountAPortfolioId: 1n, accountAPositionEpoch: 2n, accountBPortfolioId: 3n,
      accountBPositionEpoch: 4n, accountBMatcherSequence: 5n, assetIndex: 0, marketId: 1n,
      sizeQ: -1n, feeBps: 12n, limitPrice: 100n, backingFeeCapBps: 0 };
    const build = (tail: { pubkey: PublicKey; isWritable?: boolean }[] = []) =>
      ix.tradeCpiIx(program, owner, market, portfolio, portfolioB, ownerB, ownerAta, authority, args, tail);
    expect(build([{ pubkey: oracle }, { pubkey: vault, isWritable: true }]).keys).toEqual([
      [owner, true, true], [market, false, true], [portfolio, false, true], [portfolioB, false, true],
      [ownerB, false, false], [ownerAta, false, true], [authority, false, false],
      [oracle, false, false], [vault, false, true],
    ].map(([pubkey, isSigner, isWritable]) => ({ pubkey, isSigner, isWritable })));
    expect(build(Array.from({ length: 32 }, () => ({ pubkey: oracle }))).keys).toHaveLength(39);
    expect(() => build(Array.from({ length: 33 }, () => ({ pubkey: oracle })))).toThrow(/32/);
  });

  it("preserves account order and signer/writable flags for all seven builders", () => {
    const trade = { accountAPortfolioId: 1n, accountAPositionEpoch: 1n, accountBPortfolioId: 2n,
      accountBPositionEpoch: 1n, assetIndex: 0, marketId: 1n, sizeQ: 1n, execPrice: 1n, feeBps: 0n, backingFeeCapBps: 0 };
    const token = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
    const cases: [ReturnType<typeof ix.initPortfolioIx>, [PublicKey, boolean, boolean][]][] = [
      [ix.initPortfolioIx(program, owner, market, portfolio), [[owner, true, true], [market, false, true], [portfolio, false, true]]],
      [ix.depositIx(program, owner, market, portfolio, ownerAta, vault, funds), [[owner, true, true], [market, false, true], [portfolio, false, true], [ownerAta, false, true], [vault, false, true], [token, false, false]]],
      [ix.withdrawIx(program, owner, market, portfolio, ownerAta, vault, authority, funds), [[owner, true, true], [market, false, true], [portfolio, false, true], [ownerAta, false, true], [vault, false, true], [authority, false, false], [token, false, false]]],
      [ix.crankIx(program, owner, market, portfolio, crank, [oracle]), [[owner, true, true], [market, false, true], [portfolio, false, true], [oracle, false, false]]],
      [ix.tradeNoCpiIx(program, owner, ownerB, market, portfolio, portfolioB, trade), [[owner, true, true], [ownerB, true, true], [market, false, true], [portfolio, false, true], [portfolioB, false, true]]],
      [ix.closePortfolioIx(program, owner, market, portfolio, close), [[owner, true, true], [market, false, true], [portfolio, false, true]]],
      [ix.closeResolvedIx(program, owner, market, portfolio, ownerAta, vault, authority, false, { feeRatePerSlot: 0n }), [[owner, false, false], [market, false, true], [portfolio, false, true], [ownerAta, false, true], [vault, false, true], [authority, false, false], [token, false, false]]],
    ];
    for (const [instruction, keys] of cases) {
      expect(instruction.programId).toEqual(program);
      expect(instruction.keys).toEqual(keys.map(([pubkey, isSigner, isWritable]) => ({ pubkey, isSigner, isWritable })));
    }
    expect(ix.crankIx(program, owner, market, portfolio, crank).keys).toHaveLength(3);
    expect(ix.closeResolvedIx(program, owner, market, portfolio, ownerAta, vault, authority, true, { feeRatePerSlot: 0n }).keys[0]).toEqual({ pubkey: owner, isSigner: true, isWritable: false });
  });
});

it.each(["long", "short"] as const)("rounds effective %s size up and ignores reset epochs", side => {
  const market = parseMarket(marketBytes);
  market.asset0.aLong = market.asset0.aShort = 5n;
  market.asset0.epochLong = 7n;
  market.asset0.epochShort = 8n;
  const leg = { assetIndex: 0, marketId: 1n, side, basisPosQ: side === "long" ? 3n : -3n,
    aBasis: 10n, epochSnap: side === "long" ? 7n : 8n };
  expect(effectiveAbsQ(market, leg)).toBe(2n);
  expect(effectiveAbsQ(market, { ...leg, epochSnap: 6n })).toBe(0n);
});

it("funds portfolio rent from the standard two-year minimum", () => {
  const key = new PublicKey(new Uint8Array(32).fill(1));
  const [create] = ix.createPortfolioIxs(key, key, key, key);
  const args = SystemInstruction.decodeCreateAccount(create);
  expect(args.space).toBe(layout.PORTFOLIO_ACCOUNT_LEN);
  expect(args.lamports).toBe((layout.PORTFOLIO_ACCOUNT_LEN + 128) * 6960);
});
