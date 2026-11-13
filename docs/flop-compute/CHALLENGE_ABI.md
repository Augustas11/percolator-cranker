# Mark rules as WebAssembly

`flop-score` ([SCORER.md](SCORER.md)) can score a rule supplied as a
WebAssembly module instead of one compiled into it: pass
`--rule wasm:<file.wasm>` (or `--base wasm:<file.wasm>`). Everything else
(data pins, losses, attacks, the gate) is unchanged.

## Three commands

    rustup target add wasm32-unknown-unknown            # once
    flop-index/challenge-template/build.sh              # -> challenge-template/out/*.wasm
    cargo run --release --manifest-path flop-index/Cargo.toml --bin flop-score -- \
      --data <pin dir> --rule wasm:flop-index/challenge-template/out/median.wasm --split tune

To write your own rule, copy one of `challenge-template/examples/*.rs`
(`median`, `live`, `hold`), add it to the template's `Cargo.toml` as an
`[[example]]` with `crate-type = ["cdylib"]`, and run `build.sh`. A rule is

    fn step(state: &mut State, at_ms: u64, quotes: &[Quote]) -> Option<f64>
    export_rule!(State = INITIAL, step);

returning USD per 1M benchmark tokens, or `None` to hold. Any language that
emits a module with the exports below works.

## Limits

| What | Limit |
|---|---|
| Module file | 1 MiB (1,048,576 bytes) |
| Linear memory | 1 MiB (16 pages) for stack, heap and state together. `memory.grow` past it returns -1; a module whose initial memory is larger does not load. State is promised up to 64 KiB; the template asserts it. |
| Fuel | 1,000,000 per step, about one unit per instruction, shared by `alloc` and `step`. On 23 quotes the template's rules use 9k (`live`), 14k (`median`) and 33k (`hold`). A step that burns it all takes about 1.5 ms. |
| Tables | 10,000 entries. |
| Imports | None. No WASI, clock, randomness, I/O or threads. |
| Start function | Not allowed. |
| Instructions | Not allowed: `table.set/grow/fill/copy/init`, `elem.drop`, `memory.init`, `data.drop` (a clone could not copy their effect). No SIMD, no threads, one memory, 32-bit memory. |
| Floats | NaN results are canonicalised, so a module computes the same bits on every machine. |

The runtime is wasmi 2, an interpreter, with eager compilation (so fuel
does not depend on whether a function was compiled yet).

## Exports

| Export | Type | Does |
|---|---|---|
| `memory` | memory | The module's only memory. |
| `abi_version` | `() -> i32` | Returns 1. Any other value: the module does not load. |
| `alloc` | `(len: i32) -> i32` | Returns a pointer to `len` writable bytes, 8-aligned. 0 means no room: that step holds. |
| `step` | `(ptr: i32, len: i32) -> f64` | Reads the snapshot at `ptr` and returns the mark in USD per 1M benchmark tokens. |

Each step the host calls `alloc(len)`, writes the snapshot there, and calls
`step(ptr, len)`.

## Snapshot layout

Little-endian. A 16-byte header, then `n` records of 40 bytes:

| Offset | Type | Field |
|---|---|---|
| 0 | u64 | `at_ms`, Unix milliseconds |
| 8 | u32 | `n`, number of records |
| 12 | u32 | reserved, 0 |
| 16 + 40i + 0 | u64 | provider: 64-bit FNV-1a of the UTF-8 name |
| +8 | u64 | quantization: 64-bit FNV-1a of the name (`bf16`, `fp8`, `unknown`, ...) |
| +16 | f64 | prompt price, USD per token, as listed |
| +24 | f64 | completion price, USD per token, as listed |
| +32 | i32 | status, 0 = up (OpenRouter's) |
| +36 | u32 | reserved, 0 |

`len` is always `16 + 40n`. Every listed endpoint is sent, down ones too, in
listing order. Prices are passed as listed, unchecked; the template's
`Quote::is_up` applies the scorer's filter (up, finite, non-negative, not
both zero). FNV-1a: offset basis `0xcbf29ce484222325`, prime `0x100000001b3`;
the template's `name("bf16")` computes it at compile time. The same name
always hashes the same, across steps and series.

## What causes a hold

A hold means no new mark this step; the scorer keeps the rule's last mark.

- `step` returns NaN, infinity, 0 or a negative number. The module's state
  is kept: the step happened.
- The module traps: `unreachable`, out-of-bounds access, division by zero,
  stack overflow, running out of fuel (an infinite loop ends here), a
  failed allocation that aborts. The module is put back exactly as it was
  before the step (memory and globals), and the next step runs normally.
- `alloc` returns 0 or a pointer whose `len` bytes do not fit in memory.
- If the module cannot be rebuilt after a trap (not expected), it holds for
  the rest of the series.

The scorer never panics or hangs on a module: every call is bounded by fuel.

## How clones work

The scorer clones a rule at each day's first snapshot to replay attack
scenarios from the same state. A clone is a fresh instance of the same
compiled module given the original's whole memory and every mutable global
(globals the module does not export are exported as `__flop_global_<i>`
before loading so the host can read them). The test
`a_stateful_wasm_rule_scores_like_its_native_twin_under_attack_replay`
checks that the stateful template rule scores to the bit like the same rule
written natively.

## Cost

Measured 2026-11-13 on an M-series Mac, two days of recorder data (576
snapshots, ~23 endpoints each): the built-in `spec` takes 5.4 µs a step,
`live.wasm` 7.4 µs and `hold.wasm` 23 µs; a clone (once per scenario per
day) about 15-20 µs. `live.wasm` scores exactly as `spec` (gain 0, se 0).
wasmi added about 10 s to a clean release build of `flop-index`.

## Not covered yet

The 30-minute uptime OpenRouter lists is not in the snapshot: the scorer's
data does not carry it. A later ABI version can add it in the reserved
fields.
