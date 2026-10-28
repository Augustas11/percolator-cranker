/// <reference types="vite/client" />
import { createRoot } from "react-dom/client";
import App from "./App";
import { MockMarketClient } from "./market/mock";

createRoot(document.getElementById("root")!).render(<App client={new MockMarketClient()} />);
