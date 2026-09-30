fn main() {
    println!("cargo:rerun-if-changed=block_utils.c");
    cc::Build::new()
        .compiler("clang")
        .file("block_utils.c")
        .flag("-fblocks")
        .compile("block_utils");
}
