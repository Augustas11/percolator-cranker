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
//! be copied, are refused at load.

use super::rules::Rule;
use crate::OrEndpoint;
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::sync::Arc;
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
/// Fuel for one step (`alloc` and `step` together); about one unit per
/// instruction. The template's rules use 9k-33k on 23 quotes; a step that
/// burns it all takes about 1.5 ms.
pub const FUEL_PER_STEP: u64 = 1_000_000;
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
    fn new(shared: &Shared) -> Result<Self> {
        let limits = StoreLimitsBuilder::new().memory_size(MAX_MEMORY_BYTES).memories(1).table_elements(MAX_TABLE_ELEMENTS).instances(1).build();
        let mut store = Store::new(&shared.engine, limits);
        store.limiter(|l| l);
        let inst = Linker::new(&shared.engine).instantiate_and_start(&mut store, &shared.module).map_err(|e| anyhow!("instantiate: {e}"))?;
        let memory = inst.get_memory(&store, "memory").context("no exported memory \"memory\"")?;
        let globals = shared.globals.iter().map(|n| inst.get_global(&store, n).with_context(|| format!("global {n}"))).collect::<Result<_>>()?;
        let abi = inst.get_typed_func::<(), i32>(&store, "abi_version").map_err(|e| anyhow!("export abi_version() -> i32: {e}"))?;
        let alloc = inst.get_typed_func::<i32, i32>(&store, "alloc").map_err(|e| anyhow!("export alloc(i32) -> i32: {e}"))?;
        let step = inst.get_typed_func::<(i32, i32), f64>(&store, "step").map_err(|e| anyhow!("export step(i32, i32) -> f64: {e}"))?;
        store.set_fuel(FUEL_PER_STEP).map_err(|e| anyhow!("{e}"))?;
        let v = abi.call(&mut store, ()).map_err(|e| anyhow!("abi_version trapped: {e}"))?;
        if v != ABI_VERSION {
            bail!("module speaks ABI {v}; this scorer speaks {ABI_VERSION}");
        }
        Ok(Self { store, memory, globals, alloc, step })
    }

    /// A fresh instance put into `saved`'s state.
    fn restored(shared: &Shared, saved: &Saved) -> Result<Self> {
        let mut l = Self::new(shared)?;
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

    /// One call of the module: Err on a trap, out of fuel, or a pointer
    /// that does not fit the input.
    fn call(&mut self, input: &[u8]) -> Result<f64> {
        self.store.set_fuel(FUEL_PER_STEP).map_err(|e| anyhow!("{e}"))?;
        let len = i32::try_from(input.len())?;
        let ptr = self.alloc.call(&mut self.store, len)?;
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
    /// None once the instance could not be rebuilt: holds from then on.
    live: Option<Live>,
    buf: Vec<u8>,
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
        let engine = Engine::new(&cfg);
        let module = Module::new(&engine, &patched).map_err(|e| anyhow!("not a valid module: {e}"))?;
        let shared = Arc::new(Shared { engine, module, globals });
        let live = Live::new(&shared)?;
        let mut saved = Saved { memory: Vec::new(), globals: Vec::new() };
        live.save(&mut saved);
        Ok(Self { shared, saved, live: Some(live), buf: Vec::new() })
    }

    /// Fuel the last step used (tests and docs).
    pub fn fuel_used(&self) -> Option<u64> {
        self.live.as_ref().and_then(|l| l.store.get_fuel().ok()).map(|left| FUEL_PER_STEP - left)
    }
}

impl Rule for WasmRule {
    fn step(&mut self, at_ms: u64, endpoints: &[OrEndpoint]) -> Option<f64> {
        let live = self.live.as_mut()?;
        encode(at_ms, endpoints, &mut self.buf);
        match live.call(&self.buf) {
            Ok(x) => {
                live.save(&mut self.saved);
                (x.is_finite() && x > 0.0).then_some(x)
            }
            Err(_) => {
                // Back to the state before this step; if that fails, hold for good.
                self.live = Live::restored(&self.shared, &self.saved).ok();
                None
            }
        }
    }

    fn clone_box(&self) -> Box<dyn Rule> {
        let live = self.live.as_ref().and_then(|_| Live::restored(&self.shared, &self.saved).ok());
        Box::new(WasmRule { shared: self.shared.clone(), saved: self.saved.clone(), live, buf: Vec::new() })
    }
}

/// Check what wasmi's validation does not, and export every mutable global
/// so a clone can copy it. Returns the module to load and those exports.
fn prepare(wasm: &[u8]) -> Result<(Vec<u8>, Vec<String>)> {
    use wasmparser::{Operator, Parser, Payload};
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
                for (i, g) in r.into_iter().enumerate() {
                    if g.map_err(|e| anyhow!("global section: {e}"))?.ty.mutable {
                        mutable.push(i as u32);
                    }
                }
            }
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
        let body = wasm.get(i + 1 + n..i + 1 + n + size as usize).context("truncated section")?;
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
            out.extend_from_slice(&wasm[i..i + 1 + n + size as usize]);
        }
        i += 1 + n + size as usize;
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
