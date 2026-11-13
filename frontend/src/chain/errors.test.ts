import { expect, test } from "vitest";
import { ChainError, plainChainError, toChainError } from "./errors";

const programs = { engine: "Engine1111", matcher: "Matcher111" };

test.each([
  ["WalletSignTransactionError: User rejected the request.", [], "You declined the transaction in your wallet."],
  ["Transaction simulation failed: Attempt to debit an account but found no record of a prior credit.", [], "Your wallet does not have enough devnet SOL to pay the network fee."],
  ["failed to send transaction: Transaction simulation failed: Error processing Instruction 0: insufficient lamports 100, need 5000", [], "Your wallet does not have enough devnet SOL to pay the network fee."],
  ["Transaction simulation failed: Blockhash not found", [], "The transaction expired before it landed. Try again."],
  ["TransactionExpiredBlockheightExceededError: Signature abc has expired: block height exceeded.", [], "The transaction expired before it landed. Try again."],
  ["Transaction simulation failed: Error processing Instruction 1: custom program error: 0x9", ["Program Matcher111 invoke [2]", "Program Matcher111 failed: custom program error: 0x9"], "The liquidity account's reference price is too old, so it refused the fill. Try again in a minute."],
  ["Transaction simulation failed: Error processing Instruction 1: custom program error: 0xb", ["Program Matcher111 failed: custom program error: 0xb"], "The order is larger than the liquidity account fills at once. Split it into smaller trades."],
  ["Transaction simulation failed: Error processing Instruction 1: custom program error: 0x1b", ["Program Engine1111 failed: custom program error: 0x1b"], "The market price is stale right now, so the trade was refused. Try again in a minute."],
  ["Transaction simulation failed: Error processing Instruction 1: custom program error: 0x5", ["Program log: margin check failed: undercollateralized", "Program Engine1111 failed: custom program error: 0x5"], "Not enough free collateral for this size. Deposit more test USDC or trade smaller."],
  ["Transaction simulation failed: Error processing Instruction 1: custom program error: 0x5", ["Program Engine1111 failed: custom program error: 0x5"], "The network rejected the transaction before sending it."],
  ["Transaction simulation failed: Error processing Instruction 0: custom program error: 0x1", ["Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA invoke [1]", "Program log: Error: insufficient funds", "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA failed: custom program error: 0x1"], "You do not have that much test USDC in your wallet."],
  ["WalletSignTransactionError: Transaction cancelled", [], "You declined the transaction in your wallet."],
  ["{\"code\":4001,\"message\":\"User denied\"}", [], "You declined the transaction in your wallet."],
] as [string, string[], string][])("plain sentence for %s", (raw, logs, expected) => {
  expect(plainChainError(raw, logs, programs)).toBe(expected);
});

test("a matcher code from another program is not read as the matcher's", () => {
  expect(plainChainError("custom program error: 0x9", ["Program Other1111 failed: custom program error: 0x9"], programs)).toBeNull();
});

test("toChainError keeps the raw text and logs as details, with a fallback sentence", () => {
  const err = toChainError(new Error("something odd"), ["Program log: one", "Program log: two"], programs);
  expect(err).toBeInstanceOf(ChainError);
  expect(err.message).toBe("The transaction failed.");
  expect(err.details).toBe("something odd\nProgram log: one\nProgram log: two");
});

test("wording that is not a wallet rejection or a blockhash expiry is not read as one", () => {
  expect(plainChainError("Program log: order declined by risk check", [], programs)).toBeNull();
  expect(plainChainError("Program log: the season has expired", [], programs)).toBeNull();
});
