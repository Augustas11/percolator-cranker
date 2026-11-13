import { Component, type ReactNode } from "react";

/**
 * Wraps one feed-driven section so a malformed field shows a short note in that
 * section instead of blanking the page. Resets when the feed sends new data (`data`).
 */
export default class FeedBoundary extends Component<{ children: ReactNode; data?: unknown; name?: string }, { failed: boolean; data?: unknown }> {
  state: { failed: boolean; data?: unknown } = { failed: false, data: this.props.data };

  static getDerivedStateFromError() { return { failed: true }; }

  static getDerivedStateFromProps(props: { data?: unknown }, state: { failed: boolean; data?: unknown }) {
    return props.data !== state.data ? { failed: false, data: props.data } : null;
  }

  componentDidCatch(error: unknown) { console.error("compute feed section failed", error); }

  render() {
    if (!this.state.failed) return this.props.children;
    return <div className="cx-broken" role="status">{this.props.name ? `${this.props.name}: ` : ""}this part of the feed could not be shown. It retries on the next update.</div>;
  }
}
