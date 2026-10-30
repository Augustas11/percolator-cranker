import { PublicKey } from "@solana/web3.js";
import { i128, u8, u16, u32, u64, u128 } from "./bytes";
import { layout as l } from "./layout";

// percolator-prog src/v16_program.rs:748-749, PortfolioMatcherConfigV16.
export const POSITION_EPOCH_BITS = 49;
export const POSITION_EPOCH_MAX = (1n << BigInt(POSITION_EPOCH_BITS)) - 1n;

function checkHeader(data: Uint8Array, kind: number, minimum: number): void {
  if (data.length < l.HEADER_LEN) throw new Error("Account too short for v16 header");
  if (u64(data, 0) !== 0x5045_5243_5631_3600n) throw new Error("Invalid v16 account magic");
  if (u16(data, 8) !== 16) throw new Error("Invalid v16 account version");
  if (u8(data, 10) !== kind) throw new Error(`Invalid account kind: expected ${kind}`);
  if (data.length < minimum) throw new Error(`Account too short: expected at least ${minimum} bytes`);
}

export function parseWrapperConfig(data: Uint8Array) {
  checkHeader(data, 1, l.MARKET_GROUP_OFF);
  const base = l.HEADER_LEN;
  return {
    marketauth: new PublicKey(data.subarray(base + l["cfg.marketauth"], base + l["cfg.marketauth"] + 32)),
    collateralMint: new PublicKey(data.subarray(base + l["cfg.collateral_mint"], base + l["cfg.collateral_mint"] + 32)),
    tradeFeeBaseBps: u64(data, base + l["cfg.trade_fee_base_bps"]),
    oracleMode: u8(data, base + l["cfg.oracle_mode"]),
  };
}

export function parseMarket(data: Uint8Array) {
  checkHeader(data, 1, l.ASSET0_SLOT_OFF + l.ENGINE_ASSET_SLOT_LEN + l.ASSET_ORACLE_WRAPPER_LEN);
  const group = l.MARKET_GROUP_OFF;
  const config = group + l["group.config"];
  // v16_program.rs:1740-1745: oracle wrapper precedes the engine asset slot.
  const asset = l.ASSET0_SLOT_OFF + l.ASSET_ORACLE_WRAPPER_LEN + l["slot.asset"];
  return {
    config: parseWrapperConfig(data),
    marketGroupId: data.subarray(group + l["group.market_group_id"], group + l["group.market_group_id"] + 32),
    maintenanceMarginBps: u64(data, config + l["config.maintenance_margin_bps"]),
    initialMarginBps: u64(data, config + l["config.initial_margin_bps"]),
    vault: u128(data, group + l["group.vault"]),
    insurance: u128(data, group + l["group.insurance"]),
    currentSlot: u64(data, group + l["group.current_slot"]),
    mode: u8(data, group + l["group.mode"]),
    asset0: {
      marketId: u64(data, asset + l["asset.market_id"]),
      effectivePrice: u64(data, asset + l["asset.effective_price"]),
      slotLast: u64(data, asset + l["asset.slot_last"]),
      aLong: u128(data, asset + l["asset.a_long"]),
      aShort: u128(data, asset + l["asset.a_short"]),
      epochLong: u64(data, asset + l["asset.epoch_long"]),
      epochShort: u64(data, asset + l["asset.epoch_short"]),
      lifecycle: u8(data, asset + l["asset.lifecycle"]),
    },
  };
}

export function parsePortfolio(data: Uint8Array) {
  checkHeader(data, 2, l.PORTFOLIO_ACCOUNT_LEN);
  const base = l.HEADER_LEN;
  const legs: { assetIndex: number; marketId: bigint; side: "long" | "short"; basisPosQ: bigint; aBasis: bigint; epochSnap: bigint }[] = [];
  for (let i = 0; i < l.LEG_COUNT; i++) {
    const leg = base + l["pf.legs"] + i * l.LEG_LEN;
    if (u8(data, leg + l["leg.active"]) === 0) continue;
    const side = u8(data, leg + l["leg.side"]);
    if (side !== 0 && side !== 1) throw new Error(`Invalid leg side: ${side}`);
    legs.push({
      assetIndex: u32(data, leg + l["leg.asset_index"]),
      marketId: u64(data, leg + l["leg.market_id"]),
      side: side === 0 ? "long" : "short",
      basisPosQ: i128(data, leg + l["leg.basis_pos_q"]),
      aBasis: u128(data, leg + l["leg.a_basis"]),
      epochSnap: u64(data, leg + l["leg.epoch_snap"]),
    });
  }
  return {
    marketGroupId: data.subarray(base + l["pf.provenance_header"], base + l["pf.provenance_header"] + 32),
    owner: new PublicKey(data.subarray(base + l["pf.owner"], base + l["pf.owner"] + 32)),
    portfolioId: u64(data, l.PORTFOLIO_ID_OFF),
    sequence: u64(data, l.PORTFOLIO_MATCHER_SEQUENCE_OFF),
    positionEpoch: (u64(data, l.PORTFOLIO_MATCHER_CONTROL_OFF) >> 1n) & POSITION_EPOCH_MAX,
    capital: u128(data, base + l["pf.capital"]),
    pnl: i128(data, base + l["pf.pnl"]),
    legs,
  };
}

export function effectiveAbsQ(market: ReturnType<typeof parseMarket>, leg: ReturnType<typeof parsePortfolio>["legs"][number]): bigint {
  const asset = market.asset0;
  if (leg.epochSnap !== (leg.side === "long" ? asset.epochLong : asset.epochShort)) return 0n;
  const raw = leg.basisPosQ < 0n ? -leg.basisPosQ : leg.basisPosQ;
  const aSide = leg.side === "long" ? asset.aLong : asset.aShort;
  return (raw * aSide + leg.aBasis - 1n) / leg.aBasis;
}
