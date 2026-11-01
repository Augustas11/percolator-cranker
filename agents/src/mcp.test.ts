import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  connect: vi.fn(), close: vi.fn(), transportClose: vi.fn(), callTool: vi.fn(), transport: vi.fn(),
}));
vi.mock("@modelcontextprotocol/sdk/client/index.js", () => ({
  Client: class {
    connect = mocks.connect;
    close = mocks.close;
    callTool = mocks.callTool;
  },
}));
vi.mock("@modelcontextprotocol/sdk/client/stdio.js", () => ({
  StdioClientTransport: class {
    constructor(options: unknown) { mocks.transport(options); }
    close = mocks.transportClose;
  },
}));
import { connect } from "./mcp";

beforeEach(() => vi.resetAllMocks());

describe("MCP client", () => {
  it("spawns the published server and forwards the supplied child environment", async () => {
    const mcp = await connect({ DRIP_KEYPAIR: "wallet-path", DRIP_RPC_URL: "rpc-url", PATH: "/bin", ABSENT: undefined });
    expect(mocks.transport).toHaveBeenCalledWith({
      command: "drip-mcp", args: [], stderr: "inherit",
      env: { DRIP_KEYPAIR: "wallet-path", DRIP_RPC_URL: "rpc-url", PATH: "/bin" },
    });
    mocks.callTool.mockResolvedValue({ content: [{ type: "text", text: '{"ok":true}' }] });
    expect(await mcp.call("deposit", { amount_usdc: 500 })).toEqual({ ok: true });
    expect(mocks.callTool).toHaveBeenCalledWith({ name: "deposit", arguments: { amount_usdc: 500 } });
    await mcp.close();
    expect(mocks.close).toHaveBeenCalledOnce();
  });
  it("splits an override on whitespace and supplies empty tool arguments", async () => {
    const mcp = await connect({ DRIP_MCP_CMD: " node   /tmp/server.js " });
    expect(mocks.transport.mock.calls[0][0]).toMatchObject({ command: "node", args: ["/tmp/server.js"] });
    mocks.callTool.mockResolvedValue({ content: [{ type: "text", text: "{}" }] });
    await mcp.call("close_position");
    expect(mocks.callTool).toHaveBeenCalledWith({ name: "close_position", arguments: {} });
  });
  it("throws the JSON error when the tool reports isError", async () => {
    const mcp = await connect({});
    mocks.callTool.mockResolvedValue({ isError: true, content: [{ type: "text", text: '{"error":"Insufficient funds"}' }] });
    await expect(mcp.call("deposit")).rejects.toThrow("Insufficient funds");
  });
  it("keeps a plain-text isError message", async () => {
    const mcp = await connect({});
    mocks.callTool.mockResolvedValue({ isError: true, content: [{ type: "text", text: "MCP error -32602: bad args" }] });
    await expect(mcp.call("deposit")).rejects.toThrow("MCP error -32602: bad args");
  });
  it("rejects malformed responses", async () => {
    const mcp = await connect({});
    mocks.callTool.mockResolvedValueOnce({ content: [] });
    await expect(mcp.call("get_position")).rejects.toThrow("JSON text");
    mocks.callTool.mockResolvedValueOnce({ content: [{ type: "text", text: "invalid" }] });
    await expect(mcp.call("get_position")).rejects.toThrow();
  });
  it("closes a failed connection", async () => {
    mocks.connect.mockRejectedValue(new Error("connection failed"));
    await expect(connect({})).rejects.toThrow("connection failed");
    expect(mocks.transportClose).toHaveBeenCalledOnce();
    await expect(connect({ DRIP_MCP_CMD: " " })).rejects.toThrow("must not be empty");
  });
});
