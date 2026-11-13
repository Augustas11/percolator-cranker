;; Loops forever when at_ms is a multiple of 15 minutes, else returns 1.0.
(module (memory (export "memory") 1)
  (func (export "abi_version") (result i32) i32.const 1)
  (func (export "alloc") (param i32) (result i32) i32.const 64)
  (func (export "step") (param $p i32) (param i32) (result f64)
    (if (i64.eqz (i64.rem_u (i64.load (local.get $p)) (i64.const 900000))) (then (loop $l (br $l))))
    f64.const 1))
