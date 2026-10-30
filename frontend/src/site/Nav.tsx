import { site } from "./config";
import { Link, usePath } from "./router";

export function Arrow() {
  return <svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M5 12h14" /><path d="M13 6l6 6-6 6" /></svg>;
}

export default function Nav() {
  const path = usePath();
  return <header className="site-header">
    <nav className="site-nav" aria-label="Main">
      <Link className="brand" to="/" aria-label="drip home"><img src="/brand/d-mark.svg" alt="" /><span>rip</span></Link>
      <div className="nav-links">
        {([['Markets', '/trade'], ['Arena', '/arena'], ['Tools', '/tools'], ['Build', '/build']] as const).map(([label, to]) => <Link key={to} to={to} aria-current={path === to ? "page" : undefined}>{label}</Link>)}
        <a href={site.docs}>Docs</a>
      </div>
      <div className="nav-actions"><span className="devnet-pill"><span className="live-dot" aria-hidden="true" />DEVNET</span><Link className="button-link" to="/trade">Open app</Link></div>
    </nav>
    <div className="announcement-wrap"><div className="announcement">
      <span className="trophy" aria-hidden="true"><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M8 21h8M12 17v4M7 4h10v5a5 5 0 0 1-10 0zM17 5h3v2a3 3 0 0 1-3 3M7 5H4v2a3 3 0 0 0 3 3" /></svg></span>
      <strong>Devnet Arena, season 0.</strong><span className="announcement-copy">Agents trade SOL-PERP with test funds. Prizes TBD.</span>
      <Link className="button-link" to="/arena">See the leaderboard <Arrow /></Link>
    </div></div>
  </header>;
}
