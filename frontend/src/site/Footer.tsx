import { site } from "./config";
import { Link } from "./router";

export default function Footer() {
  const columns = [
    { title: "PRODUCT", links: [["Index", "/compute"], ["Trade", "/trade"], ["Arena", "/arena"], ["Challenge", site.challenge.enabled ? site.paths.challenge : null], ["Agents", "/agents"]] },
    { title: "BUILD", links: [["Docs", "/docs"], ["GitHub", site.repo], ["MCP", "/agents#install"]] },
    { title: "COMMUNITY", links: [["X", site.x], ["Telegram", site.telegram], ["Status", site.status]] },
  ].map(col => ({ ...col, links: col.links.filter((l): l is [string, string] => !!l[1]) })).filter(col => col.links.length > 0);
  return <footer className="site-footer"><div className="footer-inner">
    <div className="footer-about">
      <div className="brand"><img src="/brand/d-mark.svg" alt="" /><span>rip</span></div>
      <p>Drip is the market for AI compute. Live prices from every seller, and a perp to trade them. Built for perp traders and for agents on FLOP Network.</p>
      <p className="official-links">Official: <a href={site.officialSite}>drip-devnet.vercel.app</a> · <a href={site.repo}>github.com/Augustas11/drip-markets</a></p>
    </div>
    <div className="footer-columns">{columns.map(col => <div key={col.title}><div className="footer-label">{col.title}</div><ul>{col.links.map(([label, url]) => <li key={label}>{url.startsWith("/") ? <Link to={url}>{label}</Link> : <a href={url}>{label}</a>}</li>)}</ul></div>)}</div>
  </div></footer>;
}
