(module (memory (export "memory") 1) (table 2 funcref) (func $f) (elem declare func $f)
  (func (export "abi_version") (result i32) (table.set (i32.const 0) (ref.func $f)) i32.const 1)
  (func (export "alloc") (param i32) (result i32) i32.const 0)
  (func (export "step") (param i32 i32) (result f64) f64.const 1))
