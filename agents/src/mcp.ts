import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

export interface Mcp {
  call(name: string, args?: Record<string, unknown>): Promise<unknown>;
  close(): Promise<void>;
}

export async function connect(env: NodeJS.ProcessEnv = process.env): Promise<Mcp> {
  const [command, ...args] = (env.DRIP_MCP_CMD ?? "drip-mcp").trim().split(/\s+/);
  if (!command) throw new Error("DRIP_MCP_CMD must not be empty");
  const transport = new StdioClientTransport({
    command, args,
    env: Object.fromEntries(Object.entries(env).filter((entry): entry is [string, string] => entry[1] !== undefined)),
    stderr: "inherit",
  });
  const client = new Client({ name: "drip-starter-agent", version: "0.1.0" });
  try {
    await client.connect(transport);
  } catch (error) {
    await transport.close();
    throw error;
  }
  return {
    async call(name, args = {}) {
      const result = await client.callTool({ name, arguments: args });
      const content = result.content;
      if (!Array.isArray(content) || content.length !== 1 || content[0].type !== "text") {
        throw new Error("Expected one JSON text item from MCP");
      }
      const text = content[0].text as string;
      if (result.isError) {
        let message = text;
        try { message = JSON.parse(text).error ?? text; } catch { /* plain-text SDK error */ }
        throw new Error(message);
      }
      return JSON.parse(text);
    },
    close: () => client.close(),
  };
}
