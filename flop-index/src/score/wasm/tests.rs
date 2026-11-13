use super::*;
use crate::score::data::{self, Series, Snapshot, DAY_MS};
use crate::score::rules::{blend, up, Spec};
use crate::score::{day_losses, median, splitmix};
use std::collections::BTreeMap;

const DIR: &str = env!("CARGO_MANIFEST_DIR");

fn fixture(name: &str) -> WasmRule {
    WasmRule::load(&Path::new(DIR).join("testdata/wasm").join(name)).unwrap()
}

/// A hand-written test module, testdata/wasm/hostile/<name>.wat (built to
/// .wasm next to it, so no WAT parser is needed here).
fn hostile(name: &str) -> Result<WasmRule> {
    WasmRule::load(&Path::new(DIR).join("testdata/wasm/hostile").join(format!("{name}.wasm")))
}

/// Synthetic recorder data shaped like a real recording (providers renamed,
/// prices scaled): 4 h of qwen3.8-27b across a UTC midnight
/// (2026-11-11 22:00 to 11-12 02:00), with endpoints going down.
fn real() -> Vec<Series> {
    data::load(&Path::new(DIR).join("testdata/recorder-excerpt")).unwrap().0
}

/// Each provider's cheapest up blend, by name.
fn cheapest(eps: &[OrEndpoint]) -> BTreeMap<String, f64> {
    let mut m = BTreeMap::new();
    for e in up(eps) {
        let b = blend(e);
        m.entry(e.provider.clone()).and_modify(|x: &mut f64| *x = x.min(b)).or_insert(b);
    }
    m
}

/// The template's median.rs, natively.
#[derive(Clone)]
struct NativeMedian;
impl Rule for NativeMedian {
    fn step(&mut self, _: u64, eps: &[OrEndpoint]) -> Option<f64> {
        let v: Vec<f64> = cheapest(eps).into_values().collect();
        if v.len() < 2 { None } else { median(v) }
    }
    fn clone_box(&self) -> Box<dyn Rule> {
        Box::new(self.clone())
    }
}

/// The template's hold.rs, natively: a provider's last up price counts for
/// 30 minutes after it goes missing.
#[derive(Clone, Default)]
struct NativeHold(BTreeMap<String, (f64, u64)>);
impl Rule for NativeHold {
    fn step(&mut self, at: u64, eps: &[OrEndpoint]) -> Option<f64> {
        const HOLD_MS: u64 = 30 * 60 * 1000;
        let cur = cheapest(eps);
        for (p, b) in &cur {
            self.0.insert(p.clone(), (*b, at));
        }
        self.0.retain(|_, (_, t)| *t <= at && at - *t <= HOLD_MS);
        let mut v: Vec<f64> = cur.values().copied().collect();
        v.extend(self.0.iter().filter(|(p, _)| !cur.contains_key(*p)).map(|(_, (b, _))| *b));
        if v.len() < 2 { None } else { median(v) }
    }
    fn clone_box(&self) -> Box<dyn Rule> {
        Box::new(self.clone())
    }
}

fn bits(x: Option<f64>) -> Option<u64> {
    x.map(f64::to_bits)
}

/// Step two rules over every snapshot; every mark must be equal to the bit.
fn same_marks(a: &mut dyn Rule, b: &mut dyn Rule, series: &[Series]) -> usize {
    let mut n = 0;
    for s in series {
        for sn in &s.snaps {
            assert_eq!(bits(a.step(sn.at_ms, &sn.endpoints)), bits(b.step(sn.at_ms, &sn.endpoints)), "{} at {}", s.group, sn.at_ms);
            n += 1;
        }
    }
    n
}

/// Three days of 5-minute snapshots: 5 providers on a random walk, each
/// going down now and then for 5 to 60 minutes.
fn five_minute(days: u64) -> Vec<Series> {
    let mut r = 11u64;
    let mut u = || {
        r = splitmix(r);
        (r >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut p: Vec<f64> = (0..5).map(|_| 0.2 + u()).collect();
    let mut down = [0u32; 5];
    let step = 5 * 60_000;
    let snaps = (0..days * DAY_MS / step)
        .map(|i| {
            let endpoints = (0..5)
                .map(|k| {
                    p[k] *= (0.004 * (u() - 0.5)).exp();
                    if down[k] > 0 {
                        down[k] -= 1;
                    } else if u() < 0.01 {
                        down[k] = 1 + (u() * 12.0) as u32;
                    }
                    let quantization = if k % 2 == 0 { "bf16" } else { "fp8" };
                    OrEndpoint { provider: format!("p{k}"), quantization: quantization.into(), prompt: p[k] * 1e-6, completion: 4.0 * p[k] * 1e-6, status: if down[k] > 0 { 1 } else { 0 } }
                })
                .collect();
            Snapshot { at_ms: 20_400 * DAY_MS + i * step, endpoints }
        })
        .collect();
    vec![Series { group: "test:5min".into(), snaps }]
}

#[test]
fn the_median_template_matches_its_native_twin_on_real_data() {
    let s = real();
    assert_eq!(same_marks(&mut fixture("median.wasm"), &mut NativeMedian, &s), 48);
}

#[test]
fn the_live_template_matches_the_spec_rule_on_real_data() {
    let s = real();
    assert_eq!(same_marks(&mut fixture("live.wasm"), &mut Spec, &s), 48);
    let s = five_minute(1);
    same_marks(&mut fixture("live.wasm"), &mut Spec, &s);
}

#[test]
fn a_stateful_wasm_rule_scores_like_its_native_twin_under_attack_replay() {
    // The scorer clones the rule at each day's anchor and replays five
    // scenarios from the clone: equal day losses mean the clones carried
    // the module's state exactly.
    for s in [real(), five_minute(3)] {
        let w = day_losses(&fixture("hold.wasm"), &s, 1).unwrap();
        let n = day_losses(&NativeHold::default(), &s, 1).unwrap();
        assert_eq!(w, n);
        assert!(w.len() >= 2);
    }
    // And the state matters: holding is not the plain median here.
    let s = five_minute(3);
    assert_ne!(day_losses(&fixture("hold.wasm"), &s, 1).unwrap(), day_losses(&NativeMedian, &s, 1).unwrap());
    same_marks(&mut fixture("hold.wasm"), &mut NativeHold::default(), &s);
}

#[test]
fn a_clone_carries_state_and_is_independent() {
    let s = five_minute(1);
    let snaps = &s[0].snaps;
    let mut a = fixture("hold.wasm");
    for sn in &snaps[..100] {
        a.step(sn.at_ms, &sn.endpoints);
    }
    let mut b = a.clone_box();
    // The original diverges (everyone down, later), the clone does not see it.
    let gone: Vec<OrEndpoint> = snaps[100].endpoints.iter().map(|e| OrEndpoint { status: 1, ..e.clone() }).collect();
    a.step(snaps[100].at_ms, &gone);
    let mut c = a.clone_box();
    let mut native = NativeHold::default();
    for sn in &snaps[..100] {
        native.step(sn.at_ms, &sn.endpoints);
    }
    let mut native_c = native.clone_box();
    native_c.step(snaps[100].at_ms, &gone);
    for sn in &snaps[100..200] {
        assert_eq!(bits(b.step(sn.at_ms, &sn.endpoints)), bits(native.step(sn.at_ms, &sn.endpoints)));
        assert_eq!(bits(c.step(sn.at_ms, &sn.endpoints)), bits(native_c.step(sn.at_ms, &sn.endpoints)));
    }
}

#[test]
fn two_runs_give_the_same_losses_to_the_bit() {
    let s = five_minute(2);
    let a = day_losses(&fixture("hold.wasm"), &s, 7).unwrap();
    let b = day_losses(&fixture("hold.wasm"), &s, 7).unwrap();
    let flat = |m: &BTreeMap<u64, crate::score::DayLoss>| m.values().flat_map(|l| [l.jump, l.follow, l.reference, l.attack[0], l.attack[1], l.attack[2], l.attack[3]]).map(f64::to_bits).collect::<Vec<_>>();
    assert_eq!(flat(&a), flat(&b));
}

#[test]
fn the_template_rules_use_a_small_part_of_the_fuel() {
    let s = real();
    let sn = s[0].snaps.iter().max_by_key(|sn| sn.endpoints.len()).unwrap();
    for f in ["median.wasm", "live.wasm", "hold.wasm"] {
        let mut r = fixture(f);
        assert!(r.step(sn.at_ms, &sn.endpoints).is_some());
        let used = r.fuel_used().unwrap();
        assert!(used > 0 && used < FUEL_PER_STEP / 20, "{f}: {used} fuel on {} quotes", sn.endpoints.len());
    }
}


#[test]
fn a_trap_holds_and_undoes_the_step() {
    let mut r = hostile("odd").unwrap();
    assert_eq!(r.step(1, &[]), Some(1001.0));
    assert_eq!(r.step(2, &[]), None, "trap");
    // The trapped step's writes to the global and memory are gone.
    assert_eq!(r.step(1, &[]), Some(2002.0));
}

#[test]
fn running_out_of_fuel_holds_and_cannot_hang() {
    let t = std::time::Instant::now();
    let mut r = hostile("odd").unwrap();
    assert_eq!(r.step(1, &[]), Some(1001.0));
    for _ in 0..20 {
        assert_eq!(r.step(3, &[]), None, "infinite loop");
    }
    assert_eq!(r.step(1, &[]), Some(2002.0), "usable after");
    assert!(t.elapsed() < std::time::Duration::from_secs(20), "{:?}", t.elapsed());
    // A loop in alloc too.
    let mut r = hostile("alloc-loops").unwrap();
    assert_eq!(r.step(1, &[]), None);
}

#[test]
fn the_scorer_runs_through_a_rule_that_loops_some_steps() {
    // Loops forever on every snapshot whose at_ms is a multiple of 3 x 5 min,
    // else returns 1.0: the scorer finishes and the rule holds its last mark.
    let r = hostile("loops-every-15min").unwrap();
    let l = day_losses(&r, &five_minute(1)[..], 1).unwrap();
    assert_eq!(l.len(), 1);
}

#[test]
fn a_bad_return_holds_and_keeps_the_state() {
    let mut r = hostile("odd").unwrap();
    for t in [4, 5, 6, 7] {
        assert_eq!(r.step(t, &[]), None, "at_ms {t}");
    }
    // Four completed steps (they hold, but count) and this one.
    assert_eq!(r.step(1, &[]), Some(5005.0));
}

#[test]
fn memory_cannot_grow_past_the_cap() {
    let mut r = hostile("odd").unwrap();
    assert_eq!(r.step(8, &[]), None, "1 + 16 pages is over 1 MiB");
    assert_eq!(r.step(1, &[]), Some(1001.0));
    let e = hostile("memory-17-pages");
    assert!(e.is_err());
}

#[test]
fn nan_is_canonical() {
    // 0/0 is a negative NaN on x86 and a positive one on ARM; canonicalised,
    // its sign bit is 0 everywhere.
    assert_eq!(hostile("odd").unwrap().step(9, &[]), Some(1.0));
}

#[test]
fn modules_are_refused_with_a_clear_error() {
    let err = |w: Result<WasmRule>| format!("{:#}", w.err().expect("refused"));
    let big = vec![0u8; MAX_MODULE_BYTES + 1];
    assert!(err(WasmRule::new(&big)).contains("the cap is 1048576"));
    assert!(err(hostile("imports")).contains("import nothing"));
    assert!(err(hostile("start")).contains("start function"));
    assert!(err(hostile("table-set")).contains("TableSet"));
    assert!(err(hostile("abi-2")).contains("ABI 2"));
    assert!(err(hostile("memory-only")).contains("abi_version"));
    assert!(err(hostile("memory-17-pages")).contains("instantiate"));
    assert!(err(hostile("huge-table")).contains("instantiate"));
    assert!(err(WasmRule::new(b"not wasm")).contains("not a valid module"));
}

#[test]
fn fixtures_are_built_from_the_template_as_committed() {
    // testdata/wasm/README.md lists the sha256 of each template source, each
    // test module source, and each built module; a changed source means
    // rebuilt fixtures.
    use sha2::{Digest, Sha256};
    let readme = std::fs::read_to_string(Path::new(DIR).join("testdata/wasm/README.md")).unwrap();
    let mut n = 0;
    for line in readme.lines() {
        let Some((hash, path)) = line.trim().split_once("  ") else { continue };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let bytes = std::fs::read(Path::new(DIR).join(path.trim())).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex(&Sha256::digest(&bytes)), hash, "{path} changed: rebuild the fixtures (testdata/wasm/README.md)");
        n += 1;
    }
    assert_eq!(n, 41, "every source and fixture is listed");
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn a_mutable_reference_global_is_refused() {
    // Review of PR 88: a clone copying a funcref global made wasmi panic.
    let e = format!("{:#}", hostile("mut-funcref-global").err().expect("refused"));
    assert!(e.contains("mutable funcref global"), "{e}");
}


#[test]
fn modules_over_the_load_caps_are_refused() {
    for (f, want) in [
        ("too-many-globals", "1001 globals"),
        ("too-many-functions", "10001 functions"),
        ("too-many-data-segments", "1001 data segments"),
        ("too-many-element-segments", "1001 element segments"),
    ] {
        let e = format!("{:#}", hostile(f).err().expect("refused"));
        assert!(e.contains(want), "{f}: {e}");
    }
    // A section size past the end is an error, not an overflow.
    let mut w = b"\0asm\x01\0\0\0".to_vec();
    w.extend_from_slice(&[0x07, 0xff, 0xff, 0xff, 0xff, 0x0f]);
    assert!(WasmRule::new(&w).is_err());
}



#[test]
fn fuel_used_reports_the_trapping_step() {
    let mut r = hostile("odd").unwrap();
    r.step(3, &[]);
    assert_eq!(r.fuel_used(), Some(FUEL_PER_STEP));
    r.step(2, &[]);
    let used = r.fuel_used().unwrap();
    assert!(used > 0 && used < 100, "{used}");
}

#[test]
fn an_unaligned_alloc_holds() {
    let mut r = hostile("misaligned").unwrap();
    assert_eq!(r.step(1, &[]), None);
    let mut ok = hostile("odd").unwrap();
    assert_eq!(ok.step(1, &[]), Some(1001.0));
}


#[test]
fn a_panic_in_the_runtime_disqualifies_instead_of_crashing() {
    // Defence in depth behind the load checks: whatever panics inside a
    // guarded call is caught, the rule and its clones hold, and the scorer
    // returns an error instead of losses.
    let mut r = fixture("median.wasm");
    let s = real();
    let sn = &s[0].snaps[0];
    assert!(r.step(sn.at_ms, &sn.endpoints).is_some());
    let mut c = r.clone_box();
    assert_eq!(r.guarded(|| panic!("boom")), None);
    assert!(r.disqualified().unwrap().contains("boom"));
    assert_eq!(r.step(sn.at_ms, &sn.endpoints), None);
    assert_eq!(c.step(sn.at_ms, &sn.endpoints), None, "clones share the verdict");
    assert!(c.disqualified().is_some());
    assert!(!in_guard());
    let e = format!("{:#}", day_losses(&r, &s, 1).unwrap_err());
    assert!(e.starts_with("disqualified: runtime panic"), "{e}");
}

#[test]
fn reaching_the_trap_cap_disqualifies() {
    // Re-review of PR 88: with a shared trap budget, 100 traps late in the
    // clean pass made every replay hold at the anchor, immune to attacks.
    // Two days of a module that loops every 15 minutes: 192 traps in the
    // clean pass.
    let r = hostile("loops-every-15min").unwrap();
    let e = format!("{:#}", day_losses(&r, &five_minute(2), 1).unwrap_err());
    assert_eq!(e, format!("disqualified: trapped {MAX_TRAPS_PER_SERIES} times in one series (the cap)"));
    let e = format!("{:#}", crate::score::score(&r, &Spec, &five_minute(2), 1, crate::score::Split::All).err().unwrap());
    assert!(e.starts_with("disqualified: trapped") && e.ends_with("(candidate)"), "{e}");
    // One day: 96 in the clean pass, under the cap; each replay counts from
    // its save point, so it runs.
    assert!(day_losses(&hostile("loops-every-15min").unwrap(), &five_minute(1), 1).is_ok());
}

#[test]
fn the_trap_count_is_cloned_with_the_state() {
    // A clone taken before some traps replays them exactly as the original
    // did, count included.
    let lim = Limits { max_traps_per_series: 4, ..Limits::default() };
    let run = |r: &mut dyn Rule| [1, 2, 1, 2, 1, 2, 1].map(|t| r.step(t, &[]));
    let mut a = hostile("odd").unwrap().with_limits(lim).clone_box();
    a.step(1, &[]);
    let mut before = a.clone_box();
    let first = run(a.as_mut());
    assert_eq!(first, [Some(2002.0), None, Some(3003.0), None, Some(4004.0), None, Some(5005.0)]);
    assert_eq!(run(before.as_mut()), first, "same from the save point");
    // Now a has 3 traps: a clone of it has 3 too, and its next trap is the 4th.
    let mut after = a.clone_box();
    assert_eq!(after.step(1, &[]), Some(6006.0));
    assert!(after.disqualified().is_none());
    assert_eq!(after.step(2, &[]), None);
    assert!(after.disqualified().unwrap().contains("trapped 4 times"));
}

#[test]
fn the_wall_clock_limit_disqualifies() {
    let mut r = hostile("alloc-loops").unwrap().with_limits(Limits { max_wall_ms_per_series: Some(5), max_traps_per_series: u32::MAX, ..Limits::default() });
    let t = std::time::Instant::now();
    for _ in 0..2_000 {
        assert_eq!(r.step(1, &[]), None);
    }
    assert!(t.elapsed() < std::time::Duration::from_millis(500), "{:?}", t.elapsed());
    assert!(r.disqualified().unwrap().contains("wall-clock limit"));
    let r = fixture("median.wasm").with_limits(Limits { max_wall_ms_per_series: Some(0), ..Limits::default() });
    let e = format!("{:#}", day_losses(&r, &real(), 1).unwrap_err());
    assert!(e.starts_with("disqualified: used more than 0 ms"), "{e}");
}

#[test]
fn the_costliest_known_module_is_bounded() {
    // Grows to 1 MiB, then copies memory until out of fuel every step, with
    // 1,000 globals: every step traps, so the cap disqualifies it quickly.
    let r = hostile("costly").unwrap();
    let mut c = r.clone_box();
    assert_eq!(c.step(1, &[]), Some(1.0), "grows");
    assert_eq!(c.step(2, &[]), None, "out of fuel");
    let t = std::time::Instant::now();
    assert!(day_losses(&r, &five_minute(1)[..], 1).is_err());
    assert!(t.elapsed() < std::time::Duration::from_secs(30), "{:?}", t.elapsed());
}
