//! A mark rule supplied as a WebAssembly module: `flop-score --rule
//! wasm:<file>`. The ABI and limits are in docs/flop-compute/CHALLENGE_ABI.md;
//! the starter template is flop-index/challenge-template/.
//!
//! The module runs in wasmi, an interpreter, with no imports (so no clock,
//! randomness, I/O or threads), NaN canonicalisation, a fuel budget per step
//! and a memory cap. Each step the host writes the snapshot into the
//! module's memory at the pointer `alloc(len)` returns and calls
//! `step(ptr, len) -> f64`. A trap (including running out of fuel) holds and
//! puts the module back as it was before that step; a non-finite or
//! non-positive return holds and keeps the state the step left.
//!
//! The scorer clones a rule to replay attacks from one point. A clone is a
//! fresh instance given the original's memory and mutable globals. Globals a
//! module does not export are exported under `__flop_global_<i>` before it is
//! loaded so they can be read; table and segment mutations, which could not
//! be copied, are refused at load, as are mutable globals that are not
//! numbers (a function reference cannot move between instances).
//!
//! Every call into the runtime runs under `catch_unwind`. A panic there (a
//! runtime bug a module found) disqualifies the submission with the reason
//! (`WasmRule::disqualified`) instead of ending the process. This is defence
//! in depth: the load-time checks are the control.

use super::rules::Rule;
use crate::OrEndpoint;
use anyhow::{anyhow, bail, Context, Result};
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use wasmi::{CompilationMode, Config, Engine, Global, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc, Val};

/// The ABI this host speaks: the module's `abi_version()` must return it.
pub const ABI_VERSION: i32 = 1;
/// Largest module file accepted.
pub const MAX_MODULE_BYTES: usize = 1 << 20;
/// Linear memory cap (16 pages): stack, heap and state together. Growing
/// past it fails (memory.grow returns -1); a module declaring more initial
/// memory does not load.
pub const MAX_MEMORY_BYTES: usize = 1 << 20;
/// Table size cap (function pointers; tables cannot change after load).
pub const MAX_TABLE_ELEMENTS: usize = 10_000;
/// Most globals, functions, and data or element segments a module may
/// declare (each costs host time on every clone).
pub const MAX_GLOBALS: u32 = 1_000;
pub const MAX_FUNCTIONS: u32 = 10_000;
pub const MAX_SEGMENTS: u32 = 1_000;
/// Fuel for one step (`alloc` and `step` together); about one unit per
/// instruction. The template's rules use 9k-33k on 23 quotes.
pub const FUEL_PER_STEP: u64 = 1_000_000;
/// This many traps in one series disqualifies the submission.
pub const MAX_TRAPS_PER_SERIES: u32 = 100;

/// Run-time limits. The defaults are what `flop-score` uses; a round runner
/// may tighten them per submission (`WasmRule::with_limits`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub fuel_per_step: u64,
    /// Traps (including out of fuel) in one series that disqualify the
    /// submission. Counted per instance: a clone starts from the count at
    /// the point it was taken, like the rest of the module's state, so a
    /// replay behaves as the clean pass would have.
    pub max_traps_per_series: u32,
    /// Host time for one series (all its instances), checked between
    /// steps; over it disqualifies. Wall time depends on the machine, so
    /// this never holds (which would change a score): it is a runner's
    /// safety stop, off by default.
    pub max_wall_ms_per_series: Option<u64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self { fuel_per_step: FUEL_PER_STEP, max_traps_per_series: MAX_TRAPS_PER_SERIES, max_wall_ms_per_series: None }
    }
}
/// Snapshot layout: header (u64 at_ms, u32 n, u32 0), then n records.
pub const HEADER_BYTES: usize = 16;
/// Record: u64 provider hash, u64 quantization hash, f64 prompt, f64
/// completion, i32 status, u32 0. Little-endian.
pub const QUOTE_BYTES: usize = 40;

const GLOBAL_EXPORT: &str = "__flop_global_";

/// 64-bit FNV-1a of a name, as the module sees providers and quantizations.
pub fn name_hash(s: &str) -> u64 {
    super::fnv(s)
}

/// The snapshot as the module reads it.
pub fn encode(at_ms: u64, endpoints: &[OrEndpoint], out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(&at_ms.to_le_bytes());
    out.extend_from_slice(&(endpoints.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for e in endpoints {
        out.extend_from_slice(&name_hash(&e.provider).to_le_bytes());
        out.extend_from_slice(&name_hash(&e.quantization).to_le_bytes());
        out.extend_from_slice(&e.prompt.to_le_bytes());
        out.extend_from_slice(&e.completion.to_le_bytes());
        out.extend_from_slice(&(e.status.clamp(i32::MIN as i64, i32::MAX as i64) as i32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
    }
}

/// Compiled once, shared by every clone.
struct Shared {
    engine: Engine,
    module: Module,
    /// Export names of the mutable globals, in index order.
    globals: Vec<String>,
    /// Why the submission was disqualified: a runtime panic, the trap cap,
    /// the wall-clock limit, or an instance that could not be rebuilt.
    disqualified: Mutex<Option<String>>,
}

impl Shared {
    fn disqualified(&self) -> Option<String> {
        self.disqualified.lock().map(|d| d.clone()).unwrap_or_else(|p| p.into_inner().clone())
    }

    /// Record the first reason; later ones are dropped.
    fn disqualify(&self, why: String) {
        self.disqualified.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert(why);
    }
}

thread_local! {
    static GUARDED: Cell<u32> = const { Cell::new(0) };
}

/// True while this thread is inside a guarded runtime call: a panic hook
/// should leave such a panic to `catch_unwind` (flop-score's hook does).
pub fn in_guard() -> bool {
    GUARDED.with(|g| g.get() > 0)
}

/// Run `f` under `catch_unwind`; a panic disqualifies the submission.
fn guarded<T>(shared: &Shared, what: &str, f: impl FnOnce() -> T) -> Option<T> {
    GUARDED.with(|g| g.set(g.get() + 1));
    let r = catch_unwind(AssertUnwindSafe(f));
    GUARDED.with(|g| g.set(g.get() - 1));
    match r {
        Ok(v) => Some(v),
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
            shared.disqualify(format!("runtime panic in {what}: {msg}"));
            None
        }
    }
}

/// A module's state between steps: its linear memory and mutable globals.
#[derive(Clone)]
struct Saved {
    memory: Vec<u8>,
    globals: Vec<Val>,
}

struct Live {
    store: Store<StoreLimits>,
    memory: Memory,
    globals: Vec<Global>,
    alloc: TypedFunc<i32, i32>,
    step: TypedFunc<(i32, i32), f64>,
}

impl Live {
    /// A fresh instance. `check_abi` calls `abi_version` (once, at load; not
    /// on rebuilds, where a hostile one could burn fuel again).
    fn new(shared: &Shared, check_abi: bool) -> Result<Self> {
        let limits = StoreLimitsBuilder::new().memory_size(MAX_MEMORY_BYTES).memories(1).table_elements(MAX_TABLE_ELEMENTS).instances(1).build();
        let mut store = Store::new(&shared.engine, limits);
        store.limiter(|l| l);
        let inst = Linker::new(&shared.engine).instantiate_and_start(&mut store, &shared.module).map_err(|e| anyhow!("instantiate: {e}"))?;
        let memory = inst.get_memory(&store, "memory").context("no exported memory \"memory\"")?;
        let globals = shared.globals.iter().map(|n| inst.get_global(&store, n).with_context(|| format!("global {n}"))).collect::<Result<_>>()?;
        let abi = inst.get_typed_func::<(), i32>(&store, "abi_version").map_err(|e| anyhow!("export abi_version() -> i32: {e}"))?;
        let alloc = inst.get_typed_func::<i32, i32>(&store, "alloc").map_err(|e| anyhow!("export alloc(i32) -> i32: {e}"))?;
        let step = inst.get_typed_func::<(i32, i32), f64>(&store, "step").map_err(|e| anyhow!("export step(i32, i32) -> f64: {e}"))?;
        if check_abi {
            store.set_fuel(FUEL_PER_STEP).map_err(|e| anyhow!("{e}"))?;
            let v = abi.call(&mut store, ()).map_err(|e| anyhow!("abi_version trapped: {e}"))?;
            if v != ABI_VERSION {
                bail!("module speaks ABI {v}; this scorer speaks {ABI_VERSION}");
            }
        }
        Ok(Self { store, memory, globals, alloc, step })
    }

    /// A fresh instance put into `saved`'s state.
    fn restored(shared: &Shared, saved: &Saved) -> Result<Self> {
        let mut l = Self::new(shared, false)?;
        let have = l.memory.data(&l.store).len();
        if saved.memory.len() > have {
            let pages = ((saved.memory.len() - have) / 65_536) as u64;
            l.memory.grow(&mut l.store, pages).map_err(|e| anyhow!("restore memory: {e}"))?;
        }
        let mem = l.memory.data_mut(&mut l.store);
        if mem.len() != saved.memory.len() {
            bail!("restore memory: {} bytes, saved {}", mem.len(), saved.memory.len());
        }
        mem.copy_from_slice(&saved.memory);
        for (g, v) in l.globals.iter().zip(&saved.globals) {
            g.set(&mut l.store, v.clone()).map_err(|e| anyhow!("restore global: {e}"))?;
        }
        Ok(l)
    }

    fn save(&self, into: &mut Saved) {
        let mem = self.memory.data(&self.store);
        into.memory.clear();
        into.memory.extend_from_slice(mem);
        into.globals.clear();
        into.globals.extend(self.globals.iter().map(|g| g.get(&self.store)));
    }

    /// One call of the module, and the fuel it used: Err on a trap, out of
    /// fuel, or a pointer that is 0, unaligned or does not fit the input.
    fn call(&mut self, input: &[u8], fuel: u64) -> (Result<f64>, u64) {
        let r = self.call_inner(input, fuel);
        (r, fuel - self.store.get_fuel().unwrap_or(0).min(fuel))
    }

    fn call_inner(&mut self, input: &[u8], fuel: u64) -> Result<f64> {
        self.store.set_fuel(fuel).map_err(|e| anyhow!("{e}"))?;
        let len = i32::try_from(input.len())?;
        let ptr = self.alloc.call(&mut self.store, len)?;
        if ptr % 8 != 0 {
            bail!("alloc returned an unaligned pointer");
        }
        let mem = self.memory.data_mut(&mut self.store);
        let start = ptr as u32 as usize;
        let end = start.checked_add(input.len()).filter(|&e| ptr != 0 && e <= mem.len()).context("alloc returned no room")?;
        mem[start..end].copy_from_slice(input);
        Ok(self.step.call(&mut self.store, (ptr, len))?)
    }
}

/// A rule run from a WebAssembly module. Between steps its instance always
/// equals `saved`.
pub struct WasmRule {
    shared: Arc<Shared>,
    saved: Saved,
    /// None once disqualified.
    live: Option<Live>,
    buf: Vec<u8>,
    limits: Limits,
    /// Traps so far on this instance's path; cloned with its state.
    traps: u32,
    /// The loaded rule: each clone of it starts a series with a fresh wall
    /// clock; clones of a series share it.
    root: bool,
    wall_ns: Arc<AtomicU64>,
    last_fuel: Option<u64>,
}

impl WasmRule {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        Self::new(&bytes).with_context(|| format!("wasm rule {}", path.display()))
    }

    pub fn new(wasm: &[u8]) -> Result<Self> {
        if wasm.len() > MAX_MODULE_BYTES {
            bail!("module is {} bytes; the cap is {MAX_MODULE_BYTES}", wasm.len());
        }
        let (patched, globals) = prepare(wasm)?;
        let mut cfg = Config::default();
        // Eager: lazy compilation would charge fuel on first call only.
        cfg.consume_fuel(true).compilation_mode(CompilationMode::Eager).allow_start_fn(false).wasm_multi_memory(false);
        let load = || -> Result<Self> {
            let engine = Engine::new(&cfg);
            let module = Module::new(&engine, &patched).map_err(|e| anyhow!("not a valid module: {e}"))?;
            let shared = Arc::new(Shared { engine, module, globals, disqualified: Mutex::new(None) });
            let live = Live::new(&shared, true)?;
            let mut saved = Saved { memory: Vec::new(), globals: Vec::new() };
            live.save(&mut saved);
            Ok(Self { shared, saved, live: Some(live), buf: Vec::new(), limits: Limits::default(), traps: 0, root: true, wall_ns: Default::default(), last_fuel: None })
        };
        GUARDED.with(|g| g.set(g.get() + 1));
        let r = catch_unwind(AssertUnwindSafe(load));
        GUARDED.with(|g| g.set(g.get() - 1));
        r.unwrap_or_else(|_| bail!("disqualified: runtime panic while loading"))
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Fuel the last step used, a trapping one included (tests and docs).
    pub fn fuel_used(&self) -> Option<u64> {
        self.last_fuel
    }

    #[cfg(test)]
    fn guarded<T>(&self, f: impl FnOnce() -> T) -> Option<T> {
        guarded(&self.shared, "test", f)
    }
}

impl Rule for WasmRule {
    fn step(&mut self, at_ms: u64, endpoints: &[OrEndpoint]) -> Option<f64> {
        if let Some(ms) = self.limits.max_wall_ms_per_series {
            if self.wall_ns.load(Ordering::Relaxed) >= ms.saturating_mul(1_000_000) {
                self.shared.disqualify(format!("used more than {ms} ms of host time in one series (the wall-clock limit)"));
            }
        }
        if self.shared.disqualified().is_some() {
            self.live = None;
        }
        let live = self.live.as_mut()?;
        let t = self.limits.max_wall_ms_per_series.map(|_| Instant::now());
        encode(at_ms, endpoints, &mut self.buf);
        let (buf, fuel) = (&self.buf, self.limits.fuel_per_step);
        let out = guarded(&self.shared, "step", || {
            let (r, used) = live.call(buf, fuel);
            if r.is_ok() {
                live.save(&mut self.saved);
            }
            (r, used)
        });
        let mark = match out {
            None => {
                self.live = None;
                None
            }
            Some((r, used)) => {
                self.last_fuel = Some(used);
                match r {
                    Ok(x) => (x.is_finite() && x > 0.0).then_some(x),
                    Err(_) => {
                        // Back to the state before this step. Holding for
                        // good instead would let a module opt out of the
                        // attack replays, so the cap and a failed rebuild
                        // disqualify.
                        self.traps += 1;
                        let (shared, saved) = (&self.shared, &self.saved);
                        self.live = if self.traps >= self.limits.max_traps_per_series {
                            shared.disqualify(format!("trapped {} times in one series (the cap)", self.traps));
                            None
                        } else {
                            let l = guarded(shared, "rebuild", || Live::restored(shared, saved)).and_then(|r| r.map_err(|e| shared.disqualify(format!("could not rebuild after a trap: {e:#}"))).ok());
                            l
                        };
                        None
                    }
                }
            }
        };
        if let Some(t) = t {
            self.wall_ns.fetch_add(t.elapsed().as_nanos().min(u64::MAX as u128) as u64, Ordering::Relaxed);
        }
        mark
    }

    fn clone_box(&self) -> Box<dyn Rule> {
        let (shared, saved) = (&self.shared, &self.saved);
        let live = if self.live.is_some() && shared.disqualified().is_none() {
            guarded(shared, "clone", || Live::restored(shared, saved)).and_then(|r| r.map_err(|e| shared.disqualify(format!("could not clone: {e:#}"))).ok())
        } else {
            None
        };
        let wall_ns = if self.root { Arc::default() } else { self.wall_ns.clone() };
        Box::new(WasmRule { shared: shared.clone(), saved: saved.clone(), live, buf: Vec::new(), limits: self.limits, traps: self.traps, root: false, wall_ns, last_fuel: None })
    }

    fn disqualified(&self) -> Option<String> {
        self.shared.disqualified()
    }
}

/// Check what wasmi's validation does not, and export every mutable global
/// so a clone can copy it. Returns the module to load and those exports.
fn prepare(wasm: &[u8]) -> Result<(Vec<u8>, Vec<String>)> {
    use wasmparser::{Operator, Parser, Payload, ValType};
    if wasm.len() < 8 {
        bail!("not a valid module: {} bytes", wasm.len());
    }
    let mut mutable = Vec::new();
    for p in Parser::new(0).parse_all(wasm) {
        match p.map_err(|e| anyhow!("not a valid module: {e}"))? {
            Payload::ImportSection(r) if r.count() > 0 => {
                let names: Vec<String> = r.into_iter().filter_map(|i| i.ok()).map(|i| format!("{}.{}", i.module, i.name)).collect();
                bail!("module imports {}; a rule may import nothing", names.join(", "));
            }
            Payload::StartSection { .. } => bail!("module has a start function; a rule may not run code at load"),
            Payload::GlobalSection(r) => {
                if r.count() > MAX_GLOBALS {
                    bail!("module has {} globals; the cap is {MAX_GLOBALS}", r.count());
                }
                for (i, g) in r.into_iter().enumerate() {
                    let ty = g.map_err(|e| anyhow!("global section: {e}"))?.ty;
                    if ty.mutable {
                        // A clone copies mutable globals between instances;
                        // only numbers can move (a funcref from another
                        // instance made wasmi panic, review of PR 88).
                        if !matches!(ty.content_type, ValType::I32 | ValType::I64 | ValType::F32 | ValType::F64) {
                            bail!("module has a mutable {} global; a rule's mutable globals must be i32, i64, f32 or f64", ty.content_type);
                        }
                        mutable.push(i as u32);
                    }
                }
            }
            Payload::FunctionSection(r) if r.count() > MAX_FUNCTIONS => bail!("module has {} functions; the cap is {MAX_FUNCTIONS}", r.count()),
            Payload::DataSection(r) if r.count() > MAX_SEGMENTS => bail!("module has {} data segments; the cap is {MAX_SEGMENTS}", r.count()),
            Payload::ElementSection(r) if r.count() > MAX_SEGMENTS => bail!("module has {} element segments; the cap is {MAX_SEGMENTS}", r.count()),
            Payload::CodeSectionEntry(body) => {
                let mut ops = body.get_operators_reader().map_err(|e| anyhow!("code: {e}"))?;
                while !ops.eof() {
                    let op = ops.read().map_err(|e| anyhow!("code: {e}"))?;
                    if matches!(
                        op,
                        Operator::TableSet { .. }
                            | Operator::TableGrow { .. }
                            | Operator::TableFill { .. }
                            | Operator::TableCopy { .. }
                            | Operator::TableInit { .. }
                            | Operator::ElemDrop { .. }
                            | Operator::MemoryInit { .. }
                            | Operator::DataDrop { .. }
                    ) {
                        bail!("module uses {op:?}; table and segment changes are not allowed (a clone could not copy them)");
                    }
                }
            }
            _ => {}
        }
    }
    let names: Vec<String> = mutable.iter().map(|i| format!("{GLOBAL_EXPORT}{i}")).collect();
    if mutable.is_empty() {
        return Ok((wasm.to_vec(), names));
    }
    // Append the exports to the export section (id 7), which a rule always has.
    let mut out = wasm[..8].to_vec();
    let mut i = 8;
    let mut done = false;
    while i < wasm.len() {
        let id = wasm[i];
        let (size, n) = leb(&wasm[i + 1..])?;
        let start = i + 1 + n;
        let end = usize::try_from(size).ok().and_then(|s| start.checked_add(s)).context("section size overflows")?;
        let body = wasm.get(start..end).context("truncated section")?;
        if id == 7 {
            let (count, m) = leb(body)?;
            let mut nb = Vec::with_capacity(body.len() + 32 * names.len());
            put_leb(&mut nb, count + mutable.len() as u64);
            nb.extend_from_slice(&body[m..]);
            for (g, name) in mutable.iter().zip(&names) {
                put_leb(&mut nb, name.len() as u64);
                nb.extend_from_slice(name.as_bytes());
                nb.push(0x03);
                put_leb(&mut nb, *g as u64);
            }
            out.push(7);
            put_leb(&mut out, nb.len() as u64);
            out.extend_from_slice(&nb);
            done = true;
        } else {
            out.extend_from_slice(&wasm[i..end]);
        }
        i = end;
    }
    if !done {
        bail!("module exports nothing; a rule exports memory, abi_version, alloc and step");
    }
    Ok((out, names))
}

fn leb(b: &[u8]) -> Result<(u64, usize)> {
    let mut v = 0u64;
    for (i, x) in b.iter().take(10).enumerate() {
        v |= ((x & 0x7f) as u64) << (7 * i);
        if x & 0x80 == 0 {
            return Ok((v, i + 1));
        }
    }
    bail!("bad LEB128")
}

fn put_leb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

#[cfg(test)]
mod tests;
