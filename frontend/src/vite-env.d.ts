/// <reference types="vite/client" />
interface ImportMetaEnv {
  readonly VITE_INDEXER_URL?: string;
  readonly VITE_COMPUTE_FEED_URL?: string;
  readonly VITE_MARKET_CLIENT?: string;
  readonly VITE_RPC_URL?: string;
  readonly VITE_PROGRAM_ID?: string;
  readonly VITE_MARKET?: string;
  readonly VITE_MINT?: string;
  readonly VITE_LP_PORTFOLIO?: string;
  readonly VITE_MATCHER_PROGRAM?: string;
  readonly VITE_MATCHER_CONTEXT?: string;
}
