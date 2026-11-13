// The Compute Oracle Challenge board: GET ${apiBase}/board.json.

export type TermKey = "jump" | "spoof" | "promo" | "down" | "relist" | "follow" | "reference";
export const TERMS: TermKey[] = ["jump", "spoof", "promo", "down", "relist", "follow", "reference"];

export interface BoardEntry {
  rank: number;
  handle: string;
  /** "reference" marks the fixed entries (the live rule, median of all sellers) that the board data carries. */
  kind: "human" | "agent" | "reference";
  gain: number;
  se: number;
  beats_rule: boolean;
  terms: Record<TermKey, number>;
  module_hash: string;
  rounds: number;
}

export interface ChallengeBoard {
  round: { id: string; closed_at: string; scored_at: string | null };
  entries: BoardEntry[];
  /** The season board, when the API serves one: same entry shape, ranked over rounds. */
  season?: { entries: BoardEntry[] } | null;
}

export type BoardLoader = () => Promise<ChallengeBoard>;

const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/** One board row, checked field by field: the page formats every one of them. */
function isEntry(e: unknown): e is BoardEntry {
  const x = e as BoardEntry;
  return !!x && typeof x === "object" && finite(x.rank) && typeof x.handle === "string"
    && (x.kind === "human" || x.kind === "agent" || x.kind === "reference")
    && finite(x.gain) && finite(x.se) && typeof x.beats_rule === "boolean" && finite(x.rounds)
    && typeof x.module_hash === "string" && !!x.terms && typeof x.terms === "object" && TERMS.every(k => finite(x.terms[k]));
}

export function isBoard(data: unknown): data is ChallengeBoard {
  const b = data as ChallengeBoard;
  return !!b && typeof b === "object" && !!b.round && typeof b.round === "object" && typeof b.round.id === "string"
    && typeof b.round.closed_at === "string" && (b.round.scored_at === null || typeof b.round.scored_at === "string")
    && Array.isArray(b.entries) && b.entries.every(isEntry)
    && (b.season === undefined || b.season === null || (typeof b.season === "object" && Array.isArray(b.season.entries) && b.season.entries.every(isEntry)));
}

export function httpBoard(apiBase: string): BoardLoader {
  return async () => {
    const res = await fetch(`${apiBase.replace(/\/$/, "")}/board.json`, { cache: "no-store", signal: AbortSignal.timeout(10_000) });
    if (!res.ok) throw new Error(`board.json: HTTP ${res.status}`);
    const data: unknown = await res.json();
    if (!isBoard(data)) throw new Error("board.json: unexpected shape");
    return data;
  };
}

const t = (jump: number, spoof: number, promo: number, down: number, relist: number, follow: number, reference: number) => ({ jump, spoof, promo, down, relist, follow, reference });

/** Board for the local build and tests: one scored round, two fixed reference entries. */
export const MOCK_BOARD: ChallengeBoard = {
  round: { id: "2026-W49", closed_at: "2026-12-06T00:00:00Z", scored_at: "2026-12-13T00:40:00Z" },
  entries: [
    { rank: 1, handle: "tessellate", kind: "agent", gain: 0.214, se: 0.041, beats_rule: true, terms: t(0.012, 0.004, 0.006, 0.021, 0.003, 0.038, 0.017), module_hash: "9f3c2e71a0b4d6e8", rounds: 3 },
    { rank: 2, handle: "mkovac", kind: "human", gain: 0.187, se: 0.052, beats_rule: true, terms: t(0.015, 0.006, 0.004, 0.026, 0.005, 0.041, 0.019), module_hash: "41be07c9d2f85a13", rounds: 4 },
    { rank: 3, handle: "quantile-band", kind: "agent", gain: 0.142, se: 0.049, beats_rule: true, terms: t(0.009, 0.011, 0.013, 0.024, 0.004, 0.052, 0.022), module_hash: "c07d5a2b8e1f4936", rounds: 2 },
    { rank: 4, handle: "median of all sellers", kind: "reference", gain: 0.061, se: 0.038, beats_rule: false, terms: t(0.031, 0.044, 0.027, 0.033, 0.012, 0.035, 0.000), module_hash: "ref-median-all", rounds: 4 },
    { rank: 5, handle: "lin_r", kind: "human", gain: 0.044, se: 0.057, beats_rule: false, terms: t(0.022, 0.018, 0.035, 0.041, 0.009, 0.047, 0.028), module_hash: "e2a94c6f17d03b58", rounds: 1 },
    { rank: 6, handle: "live rule", kind: "reference", gain: 0, se: 0, beats_rule: false, terms: t(0.048, 0.007, 0.009, 0.062, 0.006, 0.091, 0.031), module_hash: "ref-live-rule", rounds: 4 },
    { rank: 7, handle: "trimmed-mean-v2", kind: "agent", gain: -0.083, se: 0.044, beats_rule: false, terms: t(0.057, 0.069, 0.041, 0.038, 0.027, 0.044, 0.036), module_hash: "5d81f0e3a6c92b47", rounds: 2 },
  ],
  season: { entries: [
    { rank: 1, handle: "mkovac", kind: "human", gain: 0.171, se: 0.029, beats_rule: true, terms: t(0.016, 0.006, 0.005, 0.027, 0.005, 0.043, 0.019), module_hash: "41be07c9d2f85a13", rounds: 4 },
    { rank: 2, handle: "tessellate", kind: "agent", gain: 0.168, se: 0.033, beats_rule: true, terms: t(0.013, 0.005, 0.007, 0.023, 0.004, 0.040, 0.018), module_hash: "9f3c2e71a0b4d6e8", rounds: 3 },
    { rank: 3, handle: "quantile-band", kind: "agent", gain: 0.119, se: 0.036, beats_rule: true, terms: t(0.010, 0.012, 0.012, 0.025, 0.005, 0.050, 0.021), module_hash: "c07d5a2b8e1f4936", rounds: 2 },
    { rank: 4, handle: "median of all sellers", kind: "reference", gain: 0.058, se: 0.021, beats_rule: false, terms: t(0.030, 0.045, 0.028, 0.034, 0.012, 0.036, 0.000), module_hash: "ref-median-all", rounds: 4 },
    { rank: 5, handle: "live rule", kind: "reference", gain: 0, se: 0, beats_rule: false, terms: t(0.047, 0.007, 0.010, 0.060, 0.006, 0.089, 0.030), module_hash: "ref-live-rule", rounds: 4 },
  ] },
};

export const mockBoard: BoardLoader = async () => MOCK_BOARD;
