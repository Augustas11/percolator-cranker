import { site } from "./config";
import routes from "./routes.json";

export interface PageMeta { title: string; description: string }

const HOME: PageMeta = routes.home;
const routeMeta = routes.routes as Record<string, PageMeta & { gated?: boolean }>;
const pick = ({ title, description }: PageMeta): PageMeta => ({ title, description });

/**
 * Title and description per route, from routes.json, which the build also turns into one static HTML shell per route
 * (scripts/route-shells.ts) so link previews show the route's own tags. Unknown paths get the 404 entry.
 */
export const pageMeta: Record<string, PageMeta> = {
  [site.paths.home]: HOME,
  ...Object.fromEntries(Object.entries(routeMeta).map(([path, m]) => [path, pick(m)])),
  ...Object.fromEntries(Object.entries(routes.aliases as Record<string, string>).map(([path, to]) => [path, pick(routeMeta[to])])),
};
export const notFoundMeta: PageMeta = { title: "Page not found · Drip", description: HOME.description };

function setMeta(attr: "name" | "property", key: string, content: string) {
  let el = document.head.querySelector<HTMLMetaElement>(`meta[${attr}="${key}"]`);
  if (!el) {
    el = document.createElement("meta");
    el.setAttribute(attr, key);
    document.head.appendChild(el);
  }
  el.content = content;
}

/** Write the page's title, description, Open Graph and Twitter tags. */
export function applyMeta({ title, description }: PageMeta, path = window.location.pathname) {
  document.title = title;
  setMeta("property", "og:url", routes.origin + (path === "/" ? "/" : path.replace(/\/$/, "")));
  setMeta("name", "description", description);
  setMeta("property", "og:title", title);
  setMeta("property", "og:description", description);
  setMeta("name", "twitter:title", title);
  setMeta("name", "twitter:description", description);
}
