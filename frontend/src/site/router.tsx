import { useSyncExternalStore } from "react";
import type { AnchorHTMLAttributes } from "react";

const navigationEvent = "site:navigate";
const subscribe = (notify: () => void) => {
  window.addEventListener("popstate", notify);
  window.addEventListener(navigationEvent, notify);
  return () => {
    window.removeEventListener("popstate", notify);
    window.removeEventListener(navigationEvent, notify);
  };
};
const getPath = () => window.location.pathname.replace(/\/$/, "") || "/";

export function usePath() {
  return useSyncExternalStore(subscribe, getPath);
}

export function Link({ to, onClick, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string }) {
  return <a {...props} href={to} onClick={event => {
    onClick?.(event);
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.currentTarget.hasAttribute("target") || event.currentTarget.hasAttribute("download")) return;
    event.preventDefault();
    if (to !== getPath() + window.location.search) {
      window.history.pushState({}, "", to);
      window.dispatchEvent(new Event(navigationEvent));
    }
    window.scrollTo(0, 0);
  }} />;
}
