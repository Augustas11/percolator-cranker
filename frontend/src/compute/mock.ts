import type { ComputeFeedClient, ComputeHistory, ComputeLatest, ComputeMarket } from "./client";
import { marketFixture } from "./fixture";

const HOUR = 3_600_000;

// Deterministic feed for tests and `npm run dev`. Times are relative to `now`.
export class MockComputeFeedClient implements ComputeFeedClient {
  readonly mock = true;
  constructor(private now = Date.now(), private ageMs = 3 * 60_000) {}

  async latest(): Promise<ComputeLatest> {
    const landed = this.now - this.ageMs;
    return {
      v: 1, generated_ms: this.now, mark_usd: 0.303551, landed_ms: landed, source_ms: landed - 140_000,
      sig: "4w4fnVJdCzYevzVWKzQakgnKQAbhpSmNMG4ugg", slot: 1000, market: "c2niKk33fXp5E8a7ZYzQXdqjEXizrSBb55VEuBQiNUU", oracle: "FLMpxq4EYV727uLVNw9Z14fL4ZDzTVtmvibVrLRgWRW4",
      source: "openrouter", set: "preferred", clamp: "off (n<3)", blend_in_out: 15,
      pricing_set: [
        { provider: "DeepInfra", quantization: "bf16", listed: 0.2578125, priced: 0.2578125, up: true },
        { provider: "Cerebras", quantization: "fp16", listed: 1.02125, priced: 1.02125, up: true },
      ],
      sellers: [
        { provider: "Darkbloom", quantization: "fp4", blended: 0.1265625, up: true, in_set: false, tier: false },
        { provider: "DeepInfra", quantization: "bf16", blended: 0.2578125, up: true, in_set: true, tier: true },
        { provider: "GMICloud", quantization: "bf16", blended: 0.621875, up: false, in_set: false, tier: true },
        { provider: "Cerebras", quantization: "fp16", blended: 1.02125, up: true, in_set: true, tier: true },
      ],
      flop_side: { flop_per_gn: 0.00291036, prints: 185, buyers: 3, miners: 34, gateway_share: 0.498, stale: null },
    };
  }

  async market(): Promise<ComputeMarket> {
    return marketFixture(this.now);
  }

  async history(): Promise<ComputeHistory> {
    const from = this.now - 168 * HOUR;
    const points: [number, number][] = Array.from({ length: 168 }, (_, i) => [from + i * HOUR, i < 140 ? 0.311224 : 0.303551]);
    return {
      v: 1, generated_ms: this.now, from_ms: from, to_ms: this.now, high: 0.311224, low: 0.303551, points,
      changes: [{ t_ms: from + 140 * HOUR, from: 0.311224, to: 0.303551, sig: "5Hp7eRC4i3bKqs1qMWiZuc5TiaSjsEAaSiZp2j" }],
      outages: [{ provider: "GMICloud", quantization: "bf16", from_ms: from + 139 * HOUR, to_ms: null }],
      recorder_gaps: [],
    };
  }
}
