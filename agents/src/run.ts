import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { config, runAgent } from "./agent";
import { connect } from "./mcp";
import { strategies } from "./strategies";

export function agentEnvs(names: string[], env: NodeJS.ProcessEnv): NodeJS.ProcessEnv[] {
  if (!names.length || names.some(name => !Object.hasOwn(strategies, name))) {
    throw new Error(`Choose strategies: ${Object.keys(strategies).join(", ")}`);
  }
  if (new Set(names).size !== names.length) throw new Error("Duplicate strategy names");
  const wallets = new Set<string>();
  return names.map(name => {
    const keypair = env[`DRIP_KEYPAIR_${name.toUpperCase()}`] || (names.length === 1 ? env.DRIP_KEYPAIR : undefined);
    if (names.length > 1) {
      if (!keypair) throw new Error("Multiple strategies require a DRIP_KEYPAIR_<NAME> for each agent");
      // Resolve symlinks so two names cannot accidentally use the same wallet file.
      const path = realpathSync(keypair);
      if (wallets.has(path)) throw new Error("Each agent requires a separate wallet file");
      wallets.add(path);
    }
    return { ...env, ...(keypair ? { DRIP_KEYPAIR: keypair } : {}) };
  });
}

async function main() {
  const names = (process.argv[2] ?? "").split(",").filter(Boolean);
  const envs = agentEnvs(names, process.env);
  const stop = new AbortController();
  const cfg = config(process.env, stop.signal);
  let terminated = false;
  const shutdown = () => { terminated = true; stop.abort(); };
  process.once("SIGINT", shutdown);
  process.once("SIGTERM", shutdown);
  try {
    const results = await Promise.allSettled(names.map(async (name, i) => {
      try {
        const mcp = await connect(envs[i]);
        await runAgent(strategies[name], cfg, mcp);
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
  main().catch(() => {
    process.stdout.write(JSON.stringify({ agent: "runner", event: "error", message: "Unable to run agents; check strategy names, wallet files and MCP configuration" }) + "\n");
    process.exitCode = 1;
  });
}
