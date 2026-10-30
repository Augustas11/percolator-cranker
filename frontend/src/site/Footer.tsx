import { site } from "./config";

export default function Footer() {
  return <footer className="site-footer">
    <p>Official links: <a href={site.officialSite}>drip-devnet.vercel.app</a> · <a href={site.repo}>github.com/Augustas11/drip</a></p>
    <p>Devnet only. Test tokens have no value. Software provided as-is.</p>
  </footer>;
}
