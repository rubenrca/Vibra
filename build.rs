use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=native/sparkle_bridge.m");
    println!("cargo:rerun-if-changed=native/sparkle_bridge_stub.c");
    println!("cargo:rerun-if-changed=native/sparkle_bridge.h");
    println!("cargo:rerun-if-changed=native/notification_bridge.m");
    println!("cargo:rerun-if-changed=native/notification_bridge.h");
    println!("cargo:rerun-if-changed=native/window_bridge.m");
    println!("cargo:rerun-if-changed=native/window_bridge.h");
    println!("cargo:rerun-if-changed=native/process_inspect.c");
    println!("cargo:rerun-if-changed=native/process_inspect.h");
    println!("cargo:rerun-if-env-changed=VIBRA_SPARKLE_FRAMEWORK");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    cc::Build::new()
        .file(manifest_dir.join("native/notification_bridge.m"))
        .include(manifest_dir.join("native"))
        .flag("-fobjc-arc")
        .flag("-fobjc-exceptions")
        .compile("vibra_notification_bridge");
    cc::Build::new()
        .file(manifest_dir.join("native/window_bridge.m"))
        .include(manifest_dir.join("native"))
        .flag("-fobjc-arc")
        .compile("vibra_window_bridge");
    cc::Build::new()
        .file(manifest_dir.join("native/process_inspect.c"))
        .include(manifest_dir.join("native"))
        .compile("vibra_process_inspect");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=UserNotifications");
    if let Some(framework_dir) = find_sparkle_framework(&manifest_dir) {
        let parent = framework_dir
            .parent()
            .expect("Sparkle.framework must live inside a Frameworks directory")
            .to_path_buf();
        println!("cargo:rustc-cfg=vibra_has_sparkle");
        println!(
            "cargo:warning=linking Sparkle from {}",
            framework_dir.display()
        );

        cc::Build::new()
            .file(manifest_dir.join("native/sparkle_bridge.m"))
            .include(manifest_dir.join("native"))
            .flag("-fobjc-arc")
            .flag(format!("-F{}", parent.display()))
            .compile("vibra_sparkle_bridge");

        println!("cargo:rustc-link-search=framework={}", parent.display());
        println!("cargo:rustc-link-lib=framework=Sparkle");
        println!("cargo:rustc-link-lib=framework=Foundation");
        // Packaged app layout.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
        // Local `cargo run` against the same framework directory.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", parent.display());
    } else {
        println!(
            "cargo:warning=Sparkle.framework not found; building stub updater (set VIBRA_SPARKLE_FRAMEWORK or run package once)"
        );
        cc::Build::new()
            .file(manifest_dir.join("native/sparkle_bridge_stub.c"))
            .include(manifest_dir.join("native"))
            .compile("vibra_sparkle_bridge");
    }
}

fn find_sparkle_framework(manifest_dir: &Path) -> Option<PathBuf> {
    if let Ok(path) = env::var("VIBRA_SPARKLE_FRAMEWORK") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return Some(path);
        }
    }

    let candidates = [
        manifest_dir.join(
            ".build/artifacts/sparkle/Sparkle/Sparkle.xcframework/macos-arm64_x86_64/Sparkle.framework",
        ),
        manifest_dir.join(
            ".build/checkouts/Sparkle/Sparkle.xcframework/macos-arm64_x86_64/Sparkle.framework",
        ),
        manifest_dir.join("third_party/Sparkle.framework"),
        manifest_dir.join("dist/Vibra.app/Contents/Frameworks/Sparkle.framework"),
    ];
    candidates.into_iter().find(|path| path.is_dir())
}
