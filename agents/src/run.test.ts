import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { agentEnvs } from "./run";

describe("wallet selection", () => {
  it("rejects unknown, duplicate and unconfigured agents before connecting", () => {
    expect(() => agentEnvs([], {})).toThrow("Choose strategies");
    expect(() => agentEnvs(["toString"], {})).toThrow("Choose strategies");
    expect(() => agentEnvs(["trend", "trend"], {})).toThrow("Duplicate");
    expect(() => agentEnvs(["trend"], {})).toThrow("own wallet file");
    expect(() => agentEnvs(["hedger", "basis"], {})).toThrow("DRIP_KEYPAIR_HEDGER");
    expect(() => agentEnvs(["trend", "meanrev"], { DRIP_KEYPAIR: "shared" })).toThrow("DRIP_KEYPAIR_TREND");
  });
  it("passes separate wallets and rejects the same wallet via symlink or copy", () => {
    const dir = mkdtempSync(join(tmpdir(), "drip-agents-"));
    try {
      const first = join(dir, "first.json"), second = join(dir, "second.json"), alias = join(dir, "alias.json"), copy = join(dir, "copy.json");
      writeFileSync(first, "[1]");
      writeFileSync(second, "[2]");
      writeFileSync(copy, "[1]\n");
      symlinkSync(first, alias);
      expect(agentEnvs(["trend"], { DRIP_KEYPAIR: first })[0].DRIP_KEYPAIR).toBe(first);
      expect(agentEnvs(["trend"], { DRIP_KEYPAIR: second, DRIP_KEYPAIR_TREND: first })[0].DRIP_KEYPAIR).toBe(first);
      const env = { DRIP_KEYPAIR_TREND: first, DRIP_KEYPAIR_MEANREV: second, DRIP_RPC_URL: "test-rpc" };
      expect(agentEnvs(["trend", "meanrev"], env).map(value => value.DRIP_KEYPAIR)).toEqual([first, second]);
      expect(agentEnvs(["trend", "meanrev"], env)[0].DRIP_RPC_URL).toBe("test-rpc");
      expect(() => agentEnvs(["trend", "meanrev"], { ...env, DRIP_KEYPAIR_MEANREV: alias })).toThrow("separate wallet");
      expect(() => agentEnvs(["trend", "meanrev"], { ...env, DRIP_KEYPAIR_MEANREV: copy })).toThrow("separate wallet");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
