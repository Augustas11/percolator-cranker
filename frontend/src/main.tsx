/// <reference types="vite/client" />
import { createRoot } from "react-dom/client";
import Site from "./site/Site";
import { MockMarketClient } from "./market/mock";
import { DevnetMarketClient } from "./market/devnet";
import { devnetConfig } from "./chain/config";
import { HttpIndexerClient, indexerUrl } from "./indexer/client";
import { MockIndexerClient } from "./indexer/mock";
import { HttpComputeFeedClient, computeFeedUrl } from "./compute/client";
import { MockComputeFeedClient } from "./compute/mock";
import { mockBoard } from "./challenge/board";

const client = import.meta.env.VITE_MARKET_CLIENT === "devnet" ? new DevnetMarketClient(devnetConfig) : new MockMarketClient();
const indexer = import.meta.env.VITE_MARKET_CLIENT === "devnet"
  ? indexerUrl === "none" ? null : new HttpIndexerClient(indexerUrl, { market: devnetConfig.market.toBase58(), program: devnetConfig.programId.toBase58() })
  : new MockIndexerClient();
const indexerBase = import.meta.env.VITE_MARKET_CLIENT === "devnet" ? (indexerUrl === "none" ? null : indexerUrl) : null;
const computeFeed = import.meta.env.VITE_MARKET_CLIENT === "devnet"
  ? computeFeedUrl === "none" ? null : new HttpComputeFeedClient(computeFeedUrl)
  : new MockComputeFeedClient();
createRoot(document.getElementById("root")!).render(<Site client={client} indexer={indexer} indexerBase={indexerBase} computeFeed={computeFeed}
  challengeBoard={import.meta.env.VITE_MARKET_CLIENT === "devnet" ? null : mockBoard} />);
