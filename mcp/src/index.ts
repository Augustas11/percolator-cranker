import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { tools } from "./tools";
import { Drip, safeError } from "./drip";

export function createServer(drip = new Drip()) {
  const server = new McpServer({ name: "drip-mcp", version: "0.1.0" });
  let pending = Promise.resolve();
  const run = (action: () => Promise<object>) => {
    const result = pending.then(async () => {
      try {
        await drip.guard();
        const output = await action();
        return { content: [{ type: "text" as const, text: JSON.stringify({ ...output, ...drip.generatedWallet }, null, 2) }] };
      } catch (error) {
        return { isError: true, content: [{ type: "text" as const, text: JSON.stringify({ error: safeError(error), ...drip.generatedWallet }, null, 2) }] };
      }
    });
    pending = result.then(() => {});
    return result;
  };
  for (const definition of tools) {
    server.registerTool(definition.name, {
      description: definition.description, ...(definition.inputSchema ? { inputSchema: definition.inputSchema } : {}),
    }, args => run(() => definition.run(drip, args)));
  }
  return server;
}

const isMain = (() => {
  try { return !!process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href; }
  catch { return false; }
})();
if (isMain) {
  try { await createServer().connect(new StdioServerTransport()); }
  catch { process.stderr.write("Unable to start drip-mcp; check configuration\n"); process.exitCode = 1; }
}
