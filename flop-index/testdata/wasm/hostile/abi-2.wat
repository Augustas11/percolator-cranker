(module (memory (export "memory") 1)
  (func (export "abi_version") (result i32) i32.const 2)
  (func (export "alloc") (param i32) (result i32) i32.const 0)
  (func (export "step") (param i32 i32) (result f64) f64.const 1))
