import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { agentEnvs } from "./run";

describe("wallet selection", () => {
  it("allows a single agent to use the default wallet or an override", () => {
    expect(agentEnvs(["trend"], {})[0].DRIP_KEYPAIR).toBeUndefined();
    expect(agentEnvs(["trend"], { DRIP_KEYPAIR: "default" })[0].DRIP_KEYPAIR).toBe("default");
    expect(agentEnvs(["trend"], { DRIP_KEYPAIR: "default", DRIP_KEYPAIR_TREND: "trend" })[0].DRIP_KEYPAIR).toBe("trend");
  });
  it("rejects unknown, duplicate and unconfigured agents before connecting", () => {
    expect(() => agentEnvs([], {})).toThrow("Choose strategies");
    expect(() => agentEnvs(["toString"], {})).toThrow("Choose strategies");
    expect(() => agentEnvs(["trend", "trend"], {})).toThrow("Duplicate");
    expect(() => agentEnvs(["trend", "meanrev"], { DRIP_KEYPAIR: "shared" })).toThrow("DRIP_KEYPAIR_<NAME>");
  });
  it("passes separate wallets and rejects identical paths including symlinks", () => {
    const dir = mkdtempSync(join(tmpdir(), "drip-agents-"));
    try {
      const first = join(dir, "first.json"), second = join(dir, "second.json"), alias = join(dir, "alias.json");
      writeFileSync(first, "[]");
      writeFileSync(second, "[]");
      symlinkSync(first, alias);
      const env = { DRIP_KEYPAIR_TREND: first, DRIP_KEYPAIR_MEANREV: second, DRIP_RPC_URL: "test-rpc" };
      expect(agentEnvs(["trend", "meanrev"], env).map(value => value.DRIP_KEYPAIR)).toEqual([first, second]);
      expect(agentEnvs(["trend", "meanrev"], env)[0].DRIP_RPC_URL).toBe("test-rpc");
      expect(() => agentEnvs(["trend", "meanrev"], { ...env, DRIP_KEYPAIR_MEANREV: alias })).toThrow("separate wallet");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
