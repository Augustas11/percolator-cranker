// After `vite build`: one static HTML shell per route with that route's title, description and og:url, so link
// previews (which do not run JavaScript) show the route's own tags. vercel.json rewrites each route to its shell.
// Gated routes (the challenge, behind a config flag) keep the home shell.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

interface Meta { title: string; description: string; gated?: boolean }
interface Routes { origin: string; home: Meta; routes: Record<string, Meta> }

const escape = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** The built index.html with one route's tags. Throws if a tag the shell must carry is missing. */
export function shell(html: string, meta: Meta, url: string) {
  const swaps: [RegExp, string][] = [
    [/<title>[^<]*<\/title>/, `<title>${escape(meta.title)}</title>`],
    [/<meta name="description" content="[^"]*" \/>/, `<meta name="description" content="${escape(meta.description)}" />`],
    [/<meta property="og:title" content="[^"]*" \/>/, `<meta property="og:title" content="${escape(meta.title)}" />`],
    [/<meta property="og:description" content="[^"]*" \/>/, `<meta property="og:description" content="${escape(meta.description)}" />`],
    [/<meta property="og:url" content="[^"]*" \/>/, `<meta property="og:url" content="${escape(url)}" />`],
    [/<meta name="twitter:title" content="[^"]*" \/>/, `<meta name="twitter:title" content="${escape(meta.title)}" />`],
    [/<meta name="twitter:description" content="[^"]*" \/>/, `<meta name="twitter:description" content="${escape(meta.description)}" />`],
  ];
  return swaps.reduce((out, [re, tag]) => {
    if (!re.test(out)) throw new Error(`route shell: index.html has no tag matching ${re}`);
    return out.replace(re, tag);
  }, html);
}

/** File name for a route's shell: /compute → compute.html. */
export const shellFile = (path: string) => `${path.replace(/^\//, "").replace(/\//g, "-")}.html`;

export function shells(html: string, routes: Routes) {
  return Object.entries(routes.routes).filter(([, m]) => !m.gated).map(([path, m]) => ({ file: shellFile(path), html: shell(html, m, routes.origin + path) }));
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const dist = new URL("../dist/", import.meta.url);
  const routes = JSON.parse(readFileSync(new URL("../src/site/routes.json", import.meta.url), "utf8")) as Routes;
  for (const s of shells(readFileSync(new URL("index.html", dist), "utf8"), routes)) writeFileSync(new URL(s.file, dist), s.html);
  console.log(`route shells: ${Object.values(routes.routes).filter(m => !m.gated).length} written`);
}
