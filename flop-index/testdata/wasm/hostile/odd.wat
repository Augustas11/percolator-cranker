;; step's behaviour by at_ms: 2 trap, 3 loop forever, 4 NaN, 5 -1, 6 inf,
;; 7 0, 8 grow memory past the cap (and trap when refused), 9 the sign of a
;; computed NaN plus 1. Every step first bumps a hidden (unexported) global
;; and a counter in memory; otherwise it returns global + 1000 x counter.
(module
  (memory (export "memory") 1)
  (global $n (mut i32) (i32.const 0))
  (func (export "abi_version") (result i32) i32.const 1)
  (func (export "alloc") (param i32) (result i32) i32.const 1024)
  (func (export "step") (param $p i32) (param $l i32) (result f64)
    (local $t i64)
    (local.set $t (i64.load (local.get $p)))
    (global.set $n (i32.add (global.get $n) (i32.const 1)))
    (i32.store (i32.const 0) (i32.add (i32.load (i32.const 0)) (i32.const 1)))
    (if (i64.eq (local.get $t) (i64.const 2)) (then unreachable))
    (if (i64.eq (local.get $t) (i64.const 3)) (then (loop $l (br $l))))
    (if (i64.eq (local.get $t) (i64.const 4)) (then (return (f64.const nan))))
    (if (i64.eq (local.get $t) (i64.const 5)) (then (return (f64.const -1))))
    (if (i64.eq (local.get $t) (i64.const 6)) (then (return (f64.const inf))))
    (if (i64.eq (local.get $t) (i64.const 7)) (then (return (f64.const 0))))
    (if (i64.eq (local.get $t) (i64.const 8))
      (then (if (i32.eq (memory.grow (i32.const 16)) (i32.const -1)) (then unreachable))))
    (if (i64.eq (local.get $t) (i64.const 9))
      (then (return (f64.convert_i64_u (i64.add (i64.const 1)
        (i64.shr_u (i64.reinterpret_f64 (f64.div (f64.const 0) (f64.const 0))) (i64.const 63)))))))
    (f64.convert_i32_u (i32.add (global.get $n) (i32.mul (i32.const 1000) (i32.load (i32.const 0))))))
)
