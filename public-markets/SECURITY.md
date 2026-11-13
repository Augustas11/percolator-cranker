# Security

This is a **devnet** market with test tokens. Nothing here is deployed on
mainnet, and the code is not audited.

## Reporting

Report suspected vulnerabilities in this repository privately, through
GitHub private vulnerability reporting: **@SECURITY_CONTACT@**. Please
don't open a public issue. Expect an acknowledgement within a few days.

Issues in the upstream engine or wrapper belong upstream, under their own
disclosure process.

## Scope

- **In scope:** key handling in the bots, faucet, MCP server and agents;
  transaction construction; the price rule and anything that could make the
  pusher sign a mark it did not compute; the indexer's and faucet's HTTP
  surfaces.
- **Out of scope:** upstream program behaviour, devnet RPC availability, and
  the value of test tokens.
