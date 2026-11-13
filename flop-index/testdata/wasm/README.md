# WebAssembly rule fixtures

Built from `flop-index/challenge-template/` so the tests in
`src/score/wasm/tests.rs` need no wasm toolchain. Built with rustc 1.96.0
(rustup stable, aarch64-apple-darwin host) by

    rustup target add wasm32-unknown-unknown
    flop-index/challenge-template/build.sh
    cp flop-index/challenge-template/out/*.wasm flop-index/testdata/wasm/

`hostile/*.wat` are hand-written modules for the failure tests (trap, fuel,
bad returns, memory cap, refused modules), built next to themselves with the
`wat` crate 1.228.0 (`wat::parse_file`); `wasm-tools parse` or wabt's
`wat2wasm` give an equivalent module, not necessarily the same bytes.

The `too-many-*.wasm` modules have no committed text: each is
`(module (memory (export "memory") 1) ...)` with 1,001 globals
(`(global i32 (i32.const 0))`), 10,001 `(func)`, 1,001
`(data (i32.const 0) "x")`, or a 1-entry table, `(func $f)` and 1,001
`(elem (i32.const 0) $f)`.

The test `fixtures_are_built_from_the_template_as_committed` checks every
hash below (paths from `flop-index/`). If you change a source, rebuild,
copy, and regenerate this list in `flop-index/` with

    shasum -a 256 challenge-template/Cargo.toml challenge-template/Cargo.lock \
      challenge-template/.cargo/config.toml challenge-template/build.sh \
      challenge-template/src/lib.rs challenge-template/examples/*.rs \
      testdata/wasm/*.wasm testdata/wasm/hostile/*

    3be524ab19099964c64ca986bfda08e5c1406218b5e784a7804e5a776b402567  challenge-template/Cargo.toml
    ecfdb20c7ac9f7b075661876dee8db6ed0eb6ee062c8a433098b66b44d86f165  challenge-template/Cargo.lock
    a6d725fa93af7eba40b75d33d81837ead71b4094e2025b4a928e2767b91269ca  challenge-template/.cargo/config.toml
    4bb02a5b5f91119b11c0e2897993f9dcb7d21d24a5156a852797863c2c502ec4  challenge-template/build.sh
    a3de52e7fc4c297cc86c051fa1c15025db3fed70008ab45ceb5f857b2c3e2c87  challenge-template/src/lib.rs
    d0150671232fadf5cb2225afe9104a86b0de2e45753e8696105e70dc99d4fb3c  challenge-template/examples/hold.rs
    b61ccf7e234d1b8cd2f96968f074494e08e5e7be7099add792b7e98992b174f8  challenge-template/examples/live.rs
    4a2dce962ad4659b444f46aef64e4a2ce68e91cb474210984d7cf3a55e2a37d5  challenge-template/examples/median.rs
    08a5ed48b554a9e02f3b9f3c768b4842765d05014d527c1381f1b5c25fa254d9  testdata/wasm/hold.wasm
    9599eb277ca22c041c6edbffe9a2b4178006f0dfa82b943909109551f4a18aaa  testdata/wasm/live.wasm
    5f1adec6e6bc8ebdcdd964b21acbf91fce74524c17a327cc9dd5a6b6323a8820  testdata/wasm/median.wasm
    8d3c4f1dd4b38dbf2aa8c63201b15a66793e684d747b87df167dff06f2db1e8b  testdata/wasm/hostile/abi-2.wasm
    fba25c9680232377d9c870880d0bf0cddf37a04e9c45fdc180b0f0c02abdd150  testdata/wasm/hostile/abi-2.wat
    cf475cccc36047790813ab722ee0f8f9dff13dc0e84ed7e3e914b05be3acc317  testdata/wasm/hostile/alloc-loops.wasm
    74157c6ca79264599f35525729750092af8889c4ec5e3bf83fda8421e010add9  testdata/wasm/hostile/alloc-loops.wat
    cbc3380e0aa8e2e53ad15446d8cfcfe0769256836496ccda1b12a9b4546db149  testdata/wasm/hostile/costly.wasm
    16558018fe5f82701daae5bbba8ad84cf46dcbc879261cd8f62536e9cfd4d5e3  testdata/wasm/hostile/costly.wat
    ac2833b38702206bbdc09c88c09c4e1ad9a16eef2f3ea5bf87ba6a87249804e1  testdata/wasm/hostile/huge-table.wasm
    1d64887feb93e63c8a3e147cc51a0a53d52f3bb12575dfdd4d070955e258aaba  testdata/wasm/hostile/huge-table.wat
    77c668308bfa4207d6bfb4238568c1d60617dab6f94cb86102a3d16758c04f5f  testdata/wasm/hostile/imports.wasm
    0893dd2117b6511e6f0b2a0eb240d75f1229bda1a583f08fab40dc77895ad16c  testdata/wasm/hostile/imports.wat
    fe7fbd7da8baabf057e8f378446bb75ca02e3619d024e8ee04aa79e4dfa2aeb4  testdata/wasm/hostile/loops-every-15min.wasm
    da869bf8932938d46c61a76f07ae3749ead1bb895af74105fd2adcbf5fbd60b9  testdata/wasm/hostile/loops-every-15min.wat
    1d3f06793e9e24b02fb523917b85ac5091f2438eee65b05c970467db84782d33  testdata/wasm/hostile/memory-17-pages.wasm
    a42ecf3794003c7b3d5d1e893c4250415946be0c63d3e56aa9f7e1a59577ed58  testdata/wasm/hostile/memory-17-pages.wat
    86b83c292b6362ce9b0e8b468f00c88186366ca4292be39a7acb5808d9f13d39  testdata/wasm/hostile/memory-only.wasm
    17e35a9c03984aacd7527b3fd29248909d45533575609b1414d25e594b16f52e  testdata/wasm/hostile/memory-only.wat
    bb8719faf0e5b1dc9ee6538ef22aa239f5ff9b17ce54dc9ea3093209e8515428  testdata/wasm/hostile/misaligned.wasm
    7f73daf2d6662eb6e3aaf9865634607323d8bc2876656c136e725cd7f5d51518  testdata/wasm/hostile/misaligned.wat
    55e7d0f1548bfcad491869133400e8fb0a557b3a8c7fdfa99646ae43ebdca6cc  testdata/wasm/hostile/mut-funcref-global.wasm
    c2b262020aef4a63dac855f9414e29f5d2485e8173e49c0fb61c5fed33111b9a  testdata/wasm/hostile/mut-funcref-global.wat
    0eaace6a3ae259adc1b530963bb609a12728b91201e996ecb33cacc2c13b5bed  testdata/wasm/hostile/odd.wasm
    0716875c0606429a80d4e6c1f142fcfd22d5a632ea6319a0f59d1d99ec78f290  testdata/wasm/hostile/odd.wat
    426dff64ca3167bae7acd0407f9b218153ba7085a2bc0d8416c94ff9cf4574e3  testdata/wasm/hostile/start.wasm
    076800afbb9a24d94a5f8b65d600d674fcecbb5a450d54dc5ebe7bc611cacbf0  testdata/wasm/hostile/start.wat
    7d095a357dae6a3a96bd716aeeb88fe0252a2a2980285d528ae0b34f2fffd36a  testdata/wasm/hostile/table-set.wasm
    1cd9160ea0326cd56dd2d066c74658476716e9a87929d8690f92881144e54f03  testdata/wasm/hostile/table-set.wat
    1c53e93c28f776a18f563d5d13d21a47bb791609d2258506ab77f1e68bfb183e  testdata/wasm/hostile/too-many-data-segments.wasm
    43cb0d40c958d347fce16a5801cb8608ea85b45ddfa9ca0ab3b31faaea46292c  testdata/wasm/hostile/too-many-element-segments.wasm
    a398cff60fed71dc7acf376e07b1c8c62a895f1c25ce522475e005ad16e50520  testdata/wasm/hostile/too-many-functions.wasm
    9dfc23292901904b32da35e5d3121f114b1b3b4b0b6793eca3bb21758cd12f89  testdata/wasm/hostile/too-many-globals.wasm
