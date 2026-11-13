// Number and time formatting shared by /compute and the home page.

/** USD per 1M tokens: more decimals for cheaper prices, so $0.04 and $3.18 both read cleanly. */
export function usd(n: number | null | undefined) {
  if (n == null || !Number.isFinite(n)) return "—";
  const digits = n >= 1 ? 2 : n >= 0.1 ? 3 : 4;
  return "$" + n.toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits });
}
/** Like usd, but keeps 3 significant digits under one cent (FLOP prices, dollars per bn G_n). */
export const usdSmall = (n: number | null | undefined) => n == null || !Number.isFinite(n) ? "—" : n > 0 && n < 0.01 ? "$" + plain(Number(n.toPrecision(3))) : usd(n);
/** A small number written out in full, never in exponent form ($0.00000128, not $1.28e-6). */
const plain = (n: number) => /e/.test(String(n)) ? n.toFixed(Math.min(20, 2 - Math.floor(Math.log10(n)))) : String(n);
/** The perp mark, always 4 decimals as written on chain. */
export const usd4 = (n: number | null | undefined) => n == null || !Number.isFinite(n) ? "—"
  : n.toLocaleString("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 4, maximumFractionDigits: 4 });

export function pct(n: number | null | undefined, digits = 1) {
  if (n == null || !Number.isFinite(n)) return "—";
  const v = Number(n.toFixed(digits));
  return (v > 0 ? "+" : v < 0 ? "−" : "") + Math.abs(v).toFixed(digits) + "%";
}
export const direction = (n: number | null | undefined) => n == null || !Number.isFinite(n) || Number(n.toFixed(1)) === 0 ? "flat" : n > 0 ? "up" : "down";

/** Dearest over cheapest seller, e.g. 21x. */
export function times(min: number, max: number) {
  if (!(min > 0) || !Number.isFinite(max)) return "—";
  const r = max / min;
  return (r >= 10 ? Math.round(r).toString() : r.toFixed(1)) + "×";
}

export const when = (ms: number) => new Date(ms).toLocaleString("en-GB", { timeZone: "UTC", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }) + " UTC";
export const day = (ms: number) => new Date(ms).toLocaleDateString("en-GB", { timeZone: "UTC", day: "numeric", month: "short", year: "numeric" });
export const clock = (ms: number) => new Date(ms).toLocaleTimeString("en-GB", { timeZone: "UTC", hour: "2-digit", minute: "2-digit" });
export function ago(ms: number, now: number) {
  const min = Math.max(0, Math.round((now - ms) / 60_000));
  return min < 1 ? "just now" : min < 120 ? `${min} min ago` : min < 48 * 60 ? `${Math.round(min / 60)} h ago` : `${Math.round(min / 1440)} d ago`;
}
export const count = (n: number) => n.toLocaleString("en-US");
export const short = (sig: string) => sig.length > 12 ? `${sig.slice(0, 6)}…${sig.slice(-6)}` : sig;
export const txUrl = (sig: string) => `https://explorer.solana.com/tx/${encodeURIComponent(sig)}?cluster=devnet`;
export const addressUrl = (id: string) => `https://explorer.solana.com/address/${encodeURIComponent(id)}?cluster=devnet`;
