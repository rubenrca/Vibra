# Blocks ABI compatibility patch

This directory vendors the published `block` 0.1.6 crate, licensed MIT by
Steven Sheldon. The public API and block layouts are unchanged.

The private type of `_NSConcreteStackBlock` is an inhabited, opaque C struct
instead of an empty Rust enum. Its address still supplies the block's `isa`,
as specified by the [Clang Blocks ABI](https://clang.llvm.org/docs/Block-ABI-Apple.html).
This removes Rust's future incompatibility warning about uninhabited extern
statics without dereferencing or interpreting the runtime's class storage.
Foreign declarations explicitly spell out their existing C ABI to avoid the
`missing_abi` warnings emitted by current Rust versions.

Native test fixtures come from `SSheldon/rust-block`, revision
`642ea4a4a5853a21b55b05c34832a5f1bb1af61c`. Their build uses `cc` instead of
the retired `gcc` crate. `Scripts/verify.sh` runs the original native invocation
and copy tests, plus capture lifetime regressions, on macOS. The small standalone
lockfile pins this test harness; the root lockfile pins the app's dependencies.

Remove this override when a compatible upstream dependency replaces it.
