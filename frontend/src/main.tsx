/// <reference types="vite/client" />
import { createRoot } from "react-dom/client";
import App from "./App";
import { MockMarketClient } from "./market/mock";
import { DevnetMarketClient } from "./market/devnet";
import { devnetConfig } from "./chain/config";

const client = import.meta.env.VITE_MARKET_CLIENT === "devnet" ? new DevnetMarketClient(devnetConfig) : new MockMarketClient();
createRoot(document.getElementById("root")!).render(<App client={client} />);
