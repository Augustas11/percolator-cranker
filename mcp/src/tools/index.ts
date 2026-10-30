import type { Tool } from "./tool";
import perpsMarkets from "./perps_markets";
import marketStats from "./market_stats";
import getPosition from "./get_position";
import deposit from "./deposit";
import openPosition from "./open_position";
import closePosition from "./close_position";
import withdraw from "./withdraw";
import tradeHistory from "./trade_history";
import leaderboard from "./leaderboard";
import claimTestFunds from "./claim_test_funds";

export const tools: Tool[] = [perpsMarkets, marketStats, getPosition, deposit, openPosition, closePosition, withdraw, tradeHistory, leaderboard, claimTestFunds];
