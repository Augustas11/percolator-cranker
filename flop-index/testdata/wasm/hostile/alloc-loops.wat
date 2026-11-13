;; alloc never returns.
(module (memory (export "memory") 1)
  (func (export "abi_version") (result i32) i32.const 1)
  (func (export "alloc") (param i32) (result i32) (loop $l (br $l)) i32.const 0)
  (func (export "step") (param i32 i32) (result f64) f64.const 1))
