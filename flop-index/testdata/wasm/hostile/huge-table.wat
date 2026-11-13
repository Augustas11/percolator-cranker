;; A 100M-entry table: would cost the host gigabytes if it were created.
(module (memory (export "memory") 1) (table 100000000 funcref)
  (func (export "abi_version") (result i32) i32.const 1)
  (func (export "alloc") (param i32) (result i32) i32.const 0)
  (func (export "step") (param i32 i32) (result f64) f64.const 1))
