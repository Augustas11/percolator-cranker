;; Initial memory over the 1 MiB cap.
(module (memory (export "memory") 17)
  (func (export "abi_version") (result i32) i32.const 1)
  (func (export "alloc") (param i32) (result i32) i32.const 0)
  (func (export "step") (param i32 i32) (result f64) f64.const 1))
