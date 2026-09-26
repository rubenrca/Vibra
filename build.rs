use std::env;
use std::path::{Path, PathBuf};

fn main() {
    build_ghostty();
    println!("cargo:rerun-if-changed=native/sparkle_bridge.m");
    println!("cargo:rerun-if-changed=native/sparkle_bridge_stub.c");
    println!("cargo:rerun-if-changed=native/sparkle_bridge.h");
    println!("cargo:rerun-if-changed=native/notification_bridge.m");
    println!("cargo:rerun-if-changed=native/notification_bridge.h");
    println!("cargo:rerun-if-changed=native/window_bridge.m");
    println!("cargo:rerun-if-changed=native/window_bridge.h");
    println!("cargo:rerun-if-changed=native/editor_bridge.m");
    println!("cargo:rerun-if-changed=native/editor_bridge.h");
    println!("cargo:rerun-if-changed=native/usage_bridge.m");
    println!("cargo:rerun-if-env-changed=VIBRA_SPARKLE_FRAMEWORK");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let sparkle_framework = find_sparkle_framework(&manifest_dir);
    // Cargo otherwise keeps a previously built stub bridge after Sparkle is
    // fetched. Watch only the selected framework and higher-priority paths;
    // watching dist/Vibra.app on every build would rebuild native code after
    // each packaging run.
    watch_sparkle_paths(&manifest_dir, sparkle_framework.as_deref());
    compile_objc(
        &manifest_dir,
        "native/notification_bridge.m",
        "vibra_notification_bridge",
        &["-fobjc-exceptions"],
    );
    compile_objc(
        &manifest_dir,
        "native/window_bridge.m",
        "vibra_window_bridge",
        &[],
    );
    compile_objc(
        &manifest_dir,
        "native/editor_bridge.m",
        "vibra_editor_bridge",
        &["-fobjc-exceptions"],
    );
    println!("cargo:rustc-link-lib=framework=Foundation");
    compile_objc(
        &manifest_dir,
        "native/usage_bridge.m",
        "vibra_usage_bridge",
        &[],
    );
    println!("cargo:rustc-link-lib=framework=Security");
    println!("cargo:rustc-link-lib=framework=LocalAuthentication");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=UserNotifications");
    if let Some(framework_dir) = sparkle_framework {
        let parent = framework_dir
            .parent()
            .expect("Sparkle.framework must live inside a Frameworks directory")
            .to_path_buf();
        println!(
            "cargo:warning=linking Sparkle from {}",
            framework_dir.display()
        );

        let mut sparkle = objc_build(&manifest_dir, "native/sparkle_bridge.m");
        sparkle.flag(format!("-F{}", parent.display()));
        sparkle.compile("vibra_sparkle_bridge");

        println!("cargo:rustc-link-search=framework={}", parent.display());
        println!("cargo:rustc-link-lib=framework=Sparkle");
        // Packaged app layout.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
        // Local `cargo run` against the same framework directory.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", parent.display());
    } else {
        println!(concat!(
            "cargo:warning=Sparkle.framework not found; building stub updater ",
            "(set VIBRA_SPARKLE_FRAMEWORK or run package once)"
        ));
        cc::Build::new()
            .file(manifest_dir.join("native/sparkle_bridge_stub.c"))
            .include(manifest_dir.join("native"))
            .compile("vibra_sparkle_bridge");
    }
}

fn objc_build(manifest_dir: &Path, file: &str) -> cc::Build {
    let mut build = cc::Build::new();
    build
        .file(manifest_dir.join(file))
        .include(manifest_dir.join("native"))
        .flag("-fobjc-arc");
    build
}

fn compile_objc(manifest_dir: &Path, file: &str, name: &str, extra_flags: &[&str]) {
    let mut build = objc_build(manifest_dir, file);
    for flag in extra_flags {
        build.flag(flag);
    }
    build.compile(name);
}

fn build_ghostty() {
    assert_eq!(
        env::var("CARGO_CFG_TARGET_OS").unwrap(),
        "macos",
        "Ghostty backend currently supports macOS only"
    );
    println!("cargo:rerun-if-env-changed=GHOSTTY_SOURCE");
    println!("cargo:rerun-if-env-changed=GHOSTTY_LIB_DIR");
    println!("cargo:rerun-if-changed=native/ghostty_bridge.c");
    println!("cargo:rerun-if-changed=native/ghostty_bridge.h");
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let source = env::var_os("GHOSTTY_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".build/ghostty/source"));
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let lib = env::var_os("GHOSTTY_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(format!(".build/ghostty/{arch}/lib")));
    let archive = lib.join("libghostty-vt.a");
    assert!(
        archive.is_file(),
        "Run Scripts/fetch_ghostty.sh first (see docs/ghostty.md); missing {}",
        archive.display()
    );
    println!("cargo:rerun-if-changed={}", archive.display());
    println!(
        "cargo:rerun-if-changed={}",
        source.join("include/ghostty").display()
    );
    cc::Build::new()
        .file("native/ghostty_bridge.c")
        .include(source.join("include"))
        .include("native")
        .flag("-std=c11")
        .warnings_into_errors(true)
        .compile("vibra_ghostty_bridge");
    println!("cargo:rustc-link-arg={}", archive.display());
}

fn find_sparkle_framework(manifest_dir: &Path) -> Option<PathBuf> {
    if let Ok(path) = env::var("VIBRA_SPARKLE_FRAMEWORK") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return Some(path);
        }
    }

    sparkle_candidates(manifest_dir)
        .into_iter()
        .find(|path| path.is_dir())
}

fn sparkle_candidates(manifest_dir: &Path) -> [PathBuf; 5] {
    [
        manifest_dir.join("third_party/sparkle-2.9.4/Sparkle.framework"),
        // Legacy Swift Package Manager layouts, still checked for local caches.
        manifest_dir.join(
            ".build/artifacts/sparkle/Sparkle/Sparkle.xcframework/macos-arm64_x86_64/Sparkle.framework",
        ),
        manifest_dir.join(
            ".build/checkouts/Sparkle/Sparkle.xcframework/macos-arm64_x86_64/Sparkle.framework",
        ),
        manifest_dir.join("third_party/Sparkle.framework"),
        manifest_dir.join("dist/Vibra.app/Contents/Frameworks/Sparkle.framework"),
    ]
}

fn watch_sparkle_paths(manifest_dir: &Path, selected: Option<&Path>) {
    if let Some(path) = env::var_os("VIBRA_SPARKLE_FRAMEWORK") {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        if selected == Some(path.as_path()) {
            return;
        }
    }
    for candidate in sparkle_candidates(manifest_dir) {
        println!("cargo:rerun-if-changed={}", candidate.display());
        if selected == Some(candidate.as_path()) {
            break;
        }
    }
}
