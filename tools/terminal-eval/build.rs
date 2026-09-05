use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=GHOSTTY_SOURCE");
    println!("cargo:rerun-if-env-changed=GHOSTTY_LIB_DIR");
    println!("cargo:rerun-if-changed=ghostty_shim.c");
    let source =
        env::var("GHOSTTY_SOURCE").expect("set GHOSTTY_SOURCE to the pinned Ghostty checkout");
    let lib = env::var("GHOSTTY_LIB_DIR").unwrap_or_else(|_| format!("{source}/zig-out/lib"));
    cc::Build::new()
        .file("ghostty_shim.c")
        .include(format!("{source}/include"))
        .flag_if_supported("-std=c11")
        .compile("ghostty_eval_shim");
    // Explicit archive path: Darwin's linker otherwise prefers the sibling dylib.
    println!("cargo:rustc-link-arg={lib}/libghostty-vt.a");
}
