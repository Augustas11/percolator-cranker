// The test-USDC faucet (faucet/src): POST {base}/claim {wallet}, GET {base}/health.
// A wallet with under 0.08 SOL also gets a SOL drip and the faucet creates its token account; a wallet that already
// has SOL gets neither, so it must create its own token account before claiming.

export interface FaucetHealth {
  claims_today: number;
  daily_cap: number;
  amount_usdc: number;
  sol_paused?: boolean;
}

export interface FaucetClaim { amount_usdc: number; signature: string; sol: number; next_claim_after: number }

/** The faucet's own threshold for the SOL drip (and the token account it creates with it). */
export const DRIP_BELOW_SOL = 0.08;

const hours = (seconds: number) => {
  const m = Math.max(1, Math.ceil(seconds / 60));
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`;
};

/** The faucet's refusal in plain words. `now` is unix seconds. */
export function faucetMessage(status: number, error: string | undefined, nextClaimAfter: number | undefined, now: number) {
  const e = error ?? "";
  if (status === 429 && /already claimed/i.test(e)) {
    return `This wallet already got test USDC in the last 24 hours.${nextClaimAfter !== undefined && nextClaimAfter > now ? ` Next claim in ${hours(nextClaimAfter - now)}.` : ""}`;
  }
  if (status === 429 && /daily cap/i.test(e)) return "The faucet has paid out all of today's claims. Try again tomorrow.";
  if (status === 429) return "Too many requests to the faucet. Try again in a minute.";
  if (status === 400 && /token account/i.test(e)) return "Create your test USDC account first, then claim.";
  if (status === 400) return "The faucet did not accept this wallet address.";
  if (status === 503 && /outcome unknown/i.test(e)) return "The faucet sent a transaction but could not confirm it. Check your balance before claiming again.";
  if (status >= 500) return "The faucet could not send right now. Try again in a minute.";
  return `The faucet refused the claim (${status}).`;
}

export class FaucetError extends Error {}

export async function claimFaucet(base: string, wallet: string, now = () => Math.floor(Date.now() / 1000)): Promise<FaucetClaim> {
  let res: Response;
  try {
    res = await fetch(`${base.replace(/\/$/, "")}/claim`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ wallet }), signal: AbortSignal.timeout(60_000) });
  } catch { throw new FaucetError("The faucet is unreachable right now. Try again in a minute."); }
  const body = await res.json().catch(() => ({})) as { error?: string; next_claim_after?: number } & Partial<FaucetClaim>;
  if (!res.ok) throw new FaucetError(faucetMessage(res.status, body.error, typeof body.next_claim_after === "number" ? body.next_claim_after : undefined, now()));
  return body as FaucetClaim;
}

export async function faucetHealth(base: string): Promise<FaucetHealth | null> {
  try {
    const res = await fetch(`${base.replace(/\/$/, "")}/health`, { signal: AbortSignal.timeout(10_000) });
    if (!res.ok) return null;
    const h = await res.json() as FaucetHealth;
    return Number.isFinite(h.amount_usdc) && Number.isFinite(h.daily_cap) && Number.isFinite(h.claims_today) ? h : null;
  } catch { return null; }
}
