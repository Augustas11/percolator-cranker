// The Drip matcher context (matcher/program/src/lib.rs): the liquidity account's limits. The matcher fills at the
// oracle (mark) price exactly, with no spread; it caps each fill and the LP's net inventory.
import { i128, u16, u64, u128 } from "./bytes";
import type { Liquidity } from "../market/types";

const MAGIC = "DRIPMTC1";

export function parseMatcher(data: Uint8Array) {
  if (data.length < 352 || new TextDecoder().decode(data.subarray(64, 72)) !== MAGIC) throw new Error("Invalid matcher context");
  return {
    paused: data[270] !== 0,
    maxFill: u128(data, 271),
    inventoryCap: u128(data, 287),
    inventory: i128(data, 303),
    maxAgeSlots: u64(data, 319),
    deviationBps: u16(data, 327),
    refPrice: u64(data, 329),
    refSlot: u64(data, 337),
  };
}

/**
 * Largest size each side can open in one fill. A long buys from the LP (its inventory falls by the size), a short
 * sells to it. A fill may not push |inventory| past the cap unless it shrinks |inventory|.
 */
export function sideLimits(maxFill: number, inventory: number, cap: number) {
  const long = Math.max(0, Math.min(maxFill, Math.max(inventory + cap, inventory > 0 ? 2 * inventory : 0)));
  const short = Math.max(0, Math.min(maxFill, Math.max(cap - inventory, inventory < 0 ? -2 * inventory : 0)));
  return { long, short };
}

export function liquidityFrom(state: ReturnType<typeof parseMatcher>, slot?: number): Liquidity {
  const units = (q: bigint) => Number(q) / 1e6;
  const maxFill = units(state.maxFill), inventory = units(state.inventory), cap = units(state.inventoryCap);
  const { long, short } = sideLimits(maxFill, inventory, cap);
  // The program refuses a fill unless ref_slot != 0 and slot - ref_slot <= max age (lib.rs, tag 0).
  const refSlot = Number(state.refSlot);
  const refAgeSlots = slot === undefined ? undefined : refSlot === 0 || slot < refSlot ? null : slot - refSlot;
  return { spreadBps: 0, maxFill, maxLong: long, maxShort: short, paused: state.paused, deviationBps: state.deviationBps,
    refPrice: Number(state.refPrice) / 1e6, maxAgeSlots: Number(state.maxAgeSlots), ...(refAgeSlots === undefined ? {} : { refAgeSlots }) };
}
