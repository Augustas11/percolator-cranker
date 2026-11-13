import { useEffect, useState } from "react";
import App from "../App";
import type { MarketClient } from "../market/types";
import type { IndexerClient } from "../indexer/client";
import type { ComputeFeedClient } from "../compute/client";
import Home from "../pages/Home";
import Arena from "../pages/Arena";
import Agents from "../pages/Agents";
import Compute from "../pages/Compute";
import Docs from "../pages/Docs";
import Challenge from "../pages/Challenge";
import type { BoardLoader } from "../challenge/board";
import { applyMeta, notFoundMeta, pageMeta } from "./meta";
import Nav from "./Nav";
import Footer from "./Footer";
import PageBoundary from "./PageBoundary";
import { site } from "./config";
import { PERP } from "../market/types";
import { Link, usePath } from "./router";
import "./site.css";

export default function Site({ client, indexer, indexerBase = null, computeFeed = null, challengeBoard = null }: { client: MarketClient; indexer: IndexerClient | null; indexerBase?: string | null; computeFeed?: ComputeFeedClient | null; challengeBoard?: BoardLoader | null }) {
  const path = usePath();
  const side = path === site.paths.trade ? new URLSearchParams(window.location.search).get("side") : null;
  const requestedSide = side === "long" || side === "short" ? side : undefined;
  const [sideRequest, setSideRequest] = useState(0);
  useEffect(() => {
    const navigate = () => {
      const side = new URLSearchParams(window.location.search).get("side");
      if (window.location.pathname.replace(/\/$/, "") === site.paths.trade && (side === "long" || side === "short")) setSideRequest(n => n + 1);
    };
    window.addEventListener("popstate", navigate);
    window.addEventListener("site:navigate", navigate);
    return () => {
      window.removeEventListener("popstate", navigate);
      window.removeEventListener("site:navigate", navigate);
    };
  }, []);
  const [tradeMounted, setTradeMounted] = useState(path === site.paths.trade);
  useEffect(() => { if (path === site.paths.trade) setTradeMounted(true); }, [path]);
  const challenge = site.challenge.enabled;
  // Old paths keep working: Tools and Build are now /agents; the swarm names are the challenge (only while it is on).
  const redirect = path === site.paths.tools || path === site.paths.build ? site.paths.agents
    : challenge && (path === "/swarm" || path === "/swarm-markets") ? site.paths.challenge : null;
  useEffect(() => {
    if (!redirect) return;
    window.history.replaceState({}, "", redirect + window.location.hash);
    window.dispatchEvent(new Event("site:navigate"));
  }, [redirect]);
  const known = path !== site.paths.challenge || challenge;
  const meta = (known && pageMeta[path]) || notFoundMeta;
  useEffect(() => { applyMeta(meta); }, [meta]);
  const page = path === site.paths.home ? <Home client={client} indexer={indexer} indexerBase={indexerBase} computeFeed={computeFeed} />
    : path === site.paths.agents || path === site.paths.tools || path === site.paths.build ? <Agents feed={computeFeed} />
    : path === site.paths.arena ? <Arena indexer={indexer} />
    : path === site.paths.compute ? <Compute feed={computeFeed} />
    : path === site.paths.docs ? <Docs feed={computeFeed} />
    : challenge && path === site.paths.challenge ? <Challenge feed={computeFeed} board={challengeBoard} />
    : <div className="not-found"><h1>Page not found</h1><p>This page does not exist.</p><div className="hero-actions"><Link className="button-link" to={site.paths.compute}>Compute index</Link><Link className="button-link outline" to={site.paths.trade}>Trade {PERP.name}</Link></div></div>;

  return <div className="site">
    <div className="site-decoration" aria-hidden="true">
      <div className="site-spotlight" />
      <img className="site-watermark" src="/brand/d-mark.svg" alt="" />
    </div>
    {/* A failure in the nav or footer leaves a plain link home instead of a blank page. */}
    <PageBoundary path={path} fallback={<header className="site-header"><nav className="site-nav" aria-label="Main"><Link className="brand" to="/">Drip</Link></nav></header>}><Nav /></PageBoundary>
    {tradeMounted && <div hidden={path !== site.paths.trade}><PageBoundary path={site.paths.trade}><App client={client} indexer={indexer} computeFeed={computeFeed} requestedSide={requestedSide} sideRequest={sideRequest} /></PageBoundary></div>}
    {path !== site.paths.trade && <main className={path === site.paths.home ? "site-home" : "site-page"}><PageBoundary path={path}>{page}</PageBoundary></main>}
    <PageBoundary path={path} fallback={null}><Footer /></PageBoundary>
  </div>;
}
