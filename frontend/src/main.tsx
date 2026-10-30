/// <reference types="vite/client" />
import { createRoot } from "react-dom/client";
import Site from "./site/Site";
import { MockMarketClient } from "./market/mock";
import { DevnetMarketClient } from "./market/devnet";
import { devnetConfig } from "./chain/config";
import { HttpIndexerClient, indexerUrl } from "./indexer/client";
import { MockIndexerClient } from "./indexer/mock";

const client = import.meta.env.VITE_MARKET_CLIENT === "devnet" ? new DevnetMarketClient(devnetConfig) : new MockMarketClient();
const indexer = import.meta.env.VITE_MARKET_CLIENT === "devnet"
  ? indexerUrl === "none" ? null : new HttpIndexerClient(indexerUrl, { market: devnetConfig.market.toBase58(), program: devnetConfig.programId.toBase58() })
  : new MockIndexerClient();
const indexerBase = import.meta.env.VITE_MARKET_CLIENT === "devnet" ? (indexerUrl === "none" ? null : indexerUrl) : null;
createRoot(document.getElementById("root")!).render(<Site client={client} indexer={indexer} indexerBase={indexerBase} />);
