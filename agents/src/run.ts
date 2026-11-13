import { readFileSync, realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { config, runAgent } from "./agent";
import { connect } from "./mcp";
import { houseStrategies, strategies } from "./strategies";

export function agentEnvs(names: string[], env: NodeJS.ProcessEnv): NodeJS.ProcessEnv[] {
  if (!names.length || names.some(name => !Object.hasOwn(strategies, name) && !Object.hasOwn(houseStrategies, name))) {
    throw new Error(`Choose strategies: ${[...Object.keys(strategies), ...Object.keys(houseStrategies)].join(", ")}`);
  }
  if (new Set(names).size !== names.length) throw new Error("Duplicate strategy names");
  const wallets = new Set<string>();
  return names.map(name => {
    const key = `DRIP_KEYPAIR_${name.toUpperCase()}`;
    const keypair = env[key] || (names.length === 1 ? env.DRIP_KEYPAIR : undefined);
    // Never fall back to the MCP's default wallet: you or another agent may be trading it.
    if (!keypair) throw new Error(names.length === 1 ? `Set ${key} or DRIP_KEYPAIR to this agent's own wallet file` : `Set ${key}; each agent needs its own wallet file`);
    // Compare contents, so copies and symlinks of one wallet are caught too.
    let wallet: string;
    try { wallet = readFileSync(keypair, "utf8").replace(/\s/g, ""); }
    catch { throw new Error(`Cannot read ${key} wallet file`); }
    if (wallets.has(wallet)) throw new Error("Each agent requires a separate wallet");
    wallets.add(wallet);
    return { ...env, DRIP_KEYPAIR: keypair };
  });
}

async function main() {
  const names = (process.argv[2] ?? "").split(",").filter(Boolean);
  const envs = agentEnvs(names, process.env);
  const built = names.map(name => Object.hasOwn(strategies, name) ? strategies[name] : houseStrategies[name](process.env));
  const stop = new AbortController();
  const cfg = config(process.env, stop.signal);
  let terminated = false;
  const shutdown = () => { terminated = true; stop.abort(); };
  process.once("SIGINT", shutdown);
  process.once("SIGTERM", shutdown);
  try {
    const results = await Promise.allSettled(built.map(async (strategy, i) => {
      try {
        const mcp = await connect(envs[i]);
        await runAgent(strategy, cfg, mcp);
      } catch (error) {
        stop.abort();
        throw error;
      }
    }));
    if (!terminated && results.some(result => result.status === "rejected")) throw new Error("Agent startup or shutdown failed; check MCP configuration");
  } finally {
    process.removeListener("SIGINT", shutdown);
    process.removeListener("SIGTERM", shutdown);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  main().catch(error => {
    // Our own errors name no secrets; MCP errors arrive already redacted.
    process.stdout.write(JSON.stringify({ agent: "runner", event: "error", message: error instanceof Error ? error.message : "Unable to run agents" }) + "\n");
    process.exitCode = 1;
  });
}
