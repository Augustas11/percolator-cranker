import { useEffect } from "react";
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
  const titles: Record<string, string> = { [site.paths.home]: "drip", [site.paths.tools]: "Tools · drip", [site.paths.arena]: "Arena · drip", [site.paths.build]: "Build · drip", [site.paths.trade]: "Trade · drip" };
  const title = titles[path] ?? "Page not found · drip";
  useEffect(() => { document.title = title; }, [title]);
  const page = path === site.paths.home ? <Home indexer={indexer} />
    : path === site.paths.tools ? <Tools />
    : path === site.paths.arena ? <Arena indexerBase={indexerBase} />
    : path === site.paths.build ? <Build />
    : <><h1>Page not found</h1><Link to={site.paths.home}>drip</Link></>;

  return <div className="site">
    <Nav />
    {path === site.paths.trade ? <App client={client} indexer={indexer} /> : <main className="site-page">{page}</main>}
    <Footer />
  </div>;
}
