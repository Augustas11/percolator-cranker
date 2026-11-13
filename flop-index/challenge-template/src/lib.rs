//! Write a FLOP mark rule as a WebAssembly module
//! (docs/flop-compute/CHALLENGE_ABI.md).
//!
//! A rule is a state type and a function:
//!
//! ```ignore
//! fn step(state: &mut State, at_ms: u64, quotes: &[Quote]) -> Option<f64>
//! ```
//!
//! `export_rule!(State = INITIAL, step);` turns it into the module's exports.
//! Return the mark in USD per 1M benchmark tokens, or None to hold. State
//! lives in module memory between steps; there is no allocator, so keep it
//! in fixed-size arrays (at most 64 KiB).

#![no_std]

/// The ABI this wrapper speaks. The scorer refuses any other.
pub const ABI_VERSION: i32 = 1;
/// The most quotes one snapshot can carry into this wrapper; a larger one holds.
pub const MAX_QUOTES: usize = 1024;
/// The promised state size.
pub const MAX_STATE_BYTES: usize = 64 * 1024;
/// The SPEC's input:output token blend.
pub const BLEND: f64 = 15.0;

const HEADER: usize = 16;
const QUOTE: usize = 40;

/// One endpoint as listed at this step. Names are 64-bit FNV-1a hashes of
/// the UTF-8 name (`name("DeepInfra")`, `name("bf16")`), stable across steps.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Quote {
    pub provider: u64,
    pub quantization: u64,
    /// USD per input token, as listed.
    pub prompt: f64,
    /// USD per output token, as listed.
    pub completion: f64,
    /// OpenRouter's endpoint status: 0 is up.
    pub status: i32,
    _reserved: u32,
}

const _: () = assert!(core::mem::size_of::<Quote>() == QUOTE);

impl Quote {
    /// Up, with a usable price (the scorer's and the pusher's filter).
    pub fn is_up(&self) -> bool {
        self.status == 0 && self.prompt.is_finite() && self.completion.is_finite() && self.prompt >= 0.0 && self.completion >= 0.0 && self.prompt + self.completion > 0.0
    }
    /// USD per 1M tokens at the 15:1 blend.
    pub fn blend(&self) -> f64 {
        (BLEND * self.prompt + self.completion) / (BLEND + 1.0) * 1e6
    }
}

/// The 64-bit FNV-1a hash the host uses for provider and quantization names.
pub const fn name(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < b.len() {
        h = (h ^ b[i] as u64).wrapping_mul(0x100_0000_01b3);
        i += 1;
    }
    h
}

/// Median (mean of the middle two for an even count). Sorts `v`.
pub fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Round a non-negative value to the nearest integer, halves away from zero
/// (`f64::round` is not in `core`).
pub fn round(x: f64) -> f64 {
    if !(x >= 0.0 && x < 4_503_599_627_370_496.0) {
        return x;
    }
    let t = x as u64 as f64;
    if x - t >= 0.5 { t + 1.0 } else { t }
}

#[repr(C, align(8))]
struct Buf([u8; HEADER + QUOTE * MAX_QUOTES]);
static mut BUF: Buf = Buf([0; HEADER + QUOTE * MAX_QUOTES]);

#[doc(hidden)]
pub fn __alloc(len: i32) -> i32 {
    if len < HEADER as i32 || len as usize > HEADER + QUOTE * MAX_QUOTES {
        return 0;
    }
    core::ptr::addr_of_mut!(BUF) as usize as i32
}

/// The snapshot the host wrote at `ptr`: (at_ms, quotes).
#[doc(hidden)]
pub fn __input(ptr: i32, len: i32) -> Option<(u64, &'static [Quote])> {
    let base = core::ptr::addr_of!(BUF) as usize;
    if ptr as usize != base || len < HEADER as i32 || len as usize > HEADER + QUOTE * MAX_QUOTES {
        return None;
    }
    // SAFETY: BUF is 8-aligned, `len` bytes of it were just written by the
    // host, and nothing else touches it during a step.
    unsafe {
        let p = base as *const u8;
        let at_ms = u64::from_le_bytes(*(p as *const [u8; 8]));
        let n = u32::from_le_bytes(*(p.add(8) as *const [u8; 4])) as usize;
        if HEADER + QUOTE * n != len as usize {
            return None;
        }
        Some((at_ms, core::slice::from_raw_parts(p.add(HEADER) as *const Quote, n)))
    }
}

/// `export_rule!(State = INITIAL, step);` exports `abi_version`, `alloc` and
/// `step` for a `fn step(&mut State, u64, &[Quote]) -> Option<f64>`.
#[macro_export]
macro_rules! export_rule {
    ($state:ty = $init:expr, $step:path) => {
        const _: () = assert!(core::mem::size_of::<$state>() <= $crate::MAX_STATE_BYTES, "state over 64 KiB");
        static mut __FLOP_STATE: $state = $init;

        #[export_name = "abi_version"]
        pub extern "C" fn __flop_abi_version() -> i32 {
            $crate::ABI_VERSION
        }

        #[export_name = "alloc"]
        pub extern "C" fn __flop_alloc(len: i32) -> i32 {
            $crate::__alloc(len)
        }

        #[export_name = "step"]
        pub extern "C" fn __flop_step(ptr: i32, len: i32) -> f64 {
            let Some((at_ms, quotes)) = $crate::__input(ptr, len) else { return 0.0 };
            // SAFETY: single-threaded; the only reference to the state.
            let state = unsafe { &mut *core::ptr::addr_of_mut!(__FLOP_STATE) };
            match $step(state, at_ms, quotes) {
                Some(p) if p.is_finite() && p > 0.0 => p,
                _ => 0.0,
            }
        }
    };
}

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
