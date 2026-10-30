import { afterEach, expect, test, vi } from "vitest";
import { HttpIndexerClient } from "./client";
import { MockIndexerClient } from "./mock";

afterEach(() => { vi.unstubAllGlobals(); });

test.each(["market", "program"] as const)("rejects a mismatched %s before returning indexed data and retries", async field => {
  const stats = await new MockIndexerClient().stats();
  const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ ...stats, [field]: "another" }) });
  vi.stubGlobal("fetch", fetch);
  const client = new HttpIndexerClient("http://indexer", stats);
  const error = `indexer serves another market (${field === "market" ? "another" : stats.market})`;
  await expect(client.stats()).rejects.toThrow(error);
  await expect(client.positions("owner")).rejects.toThrow(error);
  await expect(client.fills("portfolio")).rejects.toThrow(error);
  await expect(client.leaderboard()).rejects.toThrow(error);
  expect(fetch.mock.calls.map(([url]) => url)).toEqual(Array(4).fill("http://indexer/stats"));
  const positions = { owner: "owner", as_of_slot: null, portfolios: [] };
  fetch.mockResolvedValueOnce({ ok: true, json: async () => stats })
    .mockResolvedValueOnce({ ok: true, json: async () => positions });
  await expect(client.positions("owner")).resolves.toEqual(positions);
});

test("invalidates a cached check when later stats serve another market", async () => {
  const mock = new MockIndexerClient();
  const stats = await mock.stats();
  const positions = await mock.positions("owner");
  const fetch = vi.fn(async (url: string) => ({
    ok: true, json: async () => url.endsWith("/stats") ? stats : positions,
  }));
  vi.stubGlobal("fetch", fetch);
  const client = new HttpIndexerClient("http://indexer", stats);
  await expect(client.stats()).resolves.toEqual(stats);
  await expect(client.positions("owner")).resolves.toEqual(positions);
  fetch.mockResolvedValue({ ok: true, json: async () => ({ ...stats, market: "another" }) });
  await expect(client.stats()).rejects.toThrow("indexer serves another market (another)");
  await expect(client.positions("owner")).rejects.toThrow("indexer serves another market (another)");
});

test("accepts matching stats and shares one cached check for positions and fills", async () => {
  const mock = new MockIndexerClient();
  const stats = await mock.stats();
  const positions = await mock.positions("owner");
  const fills = await mock.fills("portfolio");
  const leaderboard = await mock.leaderboard();
  const fetch = vi.fn(async (url: string) => ({
    ok: true, json: async () => url.endsWith("/stats") ? stats : url.includes("/positions?") ? positions : url.endsWith("/leaderboard") ? leaderboard : fills,
  }));
  vi.stubGlobal("fetch", fetch);
  const client = new HttpIndexerClient("http://indexer", stats);
  await expect(client.stats()).resolves.toEqual(stats);
  fetch.mockClear();
  await expect(Promise.all([client.positions("owner"), client.fills("portfolio"), client.leaderboard()])).resolves.toEqual([positions, fills, leaderboard]);
  await expect(client.positions("owner")).resolves.toEqual(positions);
  expect(fetch.mock.calls.filter(([url]) => url.endsWith("/stats"))).toHaveLength(1);
});
