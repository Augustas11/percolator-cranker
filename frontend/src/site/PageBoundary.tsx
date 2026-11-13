import { Component, type ReactNode } from "react";

/**
 * Catches a render error in one page so the nav, the footer and the other pages keep working.
 * Resets when the path changes (`path`).
 */
export default class PageBoundary extends Component<{ children: ReactNode; path: string; fallback?: ReactNode }, { failed: boolean; path: string }> {
  state = { failed: false, path: this.props.path };

  static getDerivedStateFromError() { return { failed: true }; }

  static getDerivedStateFromProps(props: { path: string }, state: { failed: boolean; path: string }) {
    return props.path !== state.path ? { failed: false, path: props.path } : null;
  }

  componentDidCatch(error: unknown) { console.error("page failed", error); }

  render() {
    if (!this.state.failed) return this.props.children;
    if (this.props.fallback !== undefined) return this.props.fallback;
    return <div className="not-found" role="alert"><h1>This page could not be shown</h1><p>Some data it reads came back malformed. Reload to try again.</p></div>;
  }
}
