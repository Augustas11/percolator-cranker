import { site } from "./config";
import { Link, usePath } from "./router";

export default function Nav() {
  const path = usePath();
  return <nav className="site-nav" aria-label="Main">
    <Link className="brand" to={site.paths.home} aria-label="drip"><img src="/brand/d-mark.svg" alt="" /><span aria-hidden="true">rip</span></Link>
    <div className="nav-links">
      {(["tools", "arena", "build", "trade"] as const).map(page => <Link key={page} to={site.paths[page]} className={page === "trade" ? "button-link" : undefined} aria-current={path === site.paths[page] ? "page" : undefined}>{page[0].toUpperCase() + page.slice(1)}</Link>)}
    </div>
  </nav>;
}
