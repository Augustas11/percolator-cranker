;; A mutable funcref global set in step (review of PR 88): copying it into a
;; clone made wasmi panic. Refused at load.
(module (memory (export "memory") 1)
  (global $g (mut funcref) (ref.null func))
  (func $f) (elem declare func $f)
  (func (export "abi_version") (result i32) i32.const 1)
  (func (export "alloc") (param i32) (result i32) i32.const 1024)
  (func (export "step") (param i32 i32) (result f64) (global.set $g (ref.func $f)) f64.const 1))
