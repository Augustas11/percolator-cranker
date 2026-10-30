import { useEffect, useState } from "react";
import App from "../App";
import type { MarketClient } from "../market/types";
import type { IndexerClient } from "../indexer/client";
import Home from "../pages/Home";
import Tools from "../pages/Tools";
import Arena from "../pages/Arena";
import Build from "../pages/Build";
import Nav from "./Nav";
import Footer from "./Footer";
import { site } from "./config";
import { Link, usePath } from "./router";
import "./site.css";

export default function Site({ client, indexer, indexerBase = null }: { client: MarketClient; indexer: IndexerClient | null; indexerBase?: string | null }) {
  const path = usePath();
  const [tradeMounted, setTradeMounted] = useState(path === site.paths.trade);
  useEffect(() => { if (path === site.paths.trade) setTradeMounted(true); }, [path]);
  const titles: Record<string, string> = { [site.paths.home]: "drip", [site.paths.tools]: "Tools · drip", [site.paths.arena]: "Arena · drip", [site.paths.build]: "Build · drip", [site.paths.trade]: "Trade · drip" };
  const title = titles[path] ?? "Page not found · drip";
  useEffect(() => { document.title = title; }, [title]);
  const page = path === site.paths.home ? <Home client={client} indexerBase={indexerBase} />
    : path === site.paths.tools ? <Tools />
    : path === site.paths.arena ? <Arena indexerBase={indexerBase} />
    : path === site.paths.build ? <Build />
    : <><h1>Page not found</h1><Link to={site.paths.home}>drip</Link></>;

  return <div className="site">
    <div className="site-spotlight" aria-hidden="true" />
    <img className="site-watermark" src="/brand/d-mark.svg" alt="" aria-hidden="true" />
    <Nav />
    {tradeMounted && <div hidden={path !== site.paths.trade}><App client={client} indexer={indexer} /></div>}
    {path !== site.paths.trade && <main className={path === site.paths.home ? "site-home" : "site-page"}>{page}</main>}
    <Footer />
  </div>;
}
