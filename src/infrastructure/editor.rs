use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledEditor {
    pub name: &'static str,
    pub bundle_identifier: &'static str,
}

/// Editors supported by the compact IDE launcher, in fallback order.
const EDITORS: &[(&str, &[&str])] = &[
    ("Cursor", &["com.todesktop.230313mzl4w4u92"]),
    ("Visual Studio Code", &["com.microsoft.VSCode"]),
    ("VS Code Insiders", &["com.microsoft.VSCodeInsiders"]),
    (
        "Windsurf",
        &[
            "com.exafunction.windsurf",
            "com.windsurf.ide",
            "com.codeium.windsurf",
        ],
    ),
    ("Zed", &["dev.zed.Zed"]),
    ("Xcode", &["com.apple.dt.Xcode"]),
    ("Sublime Text", &["com.sublimetext.4", "com.sublimetext.3"]),
    ("VSCodium", &["com.vscodium", "com.visualstudio.code.oss"]),
];

/// Returns the supported IDEs currently indexed by macOS Launch Services.
pub fn installed_editors() -> Vec<InstalledEditor> {
    installed_editors_with(bundle_is_registered)
}

fn installed_editors_with(mut is_registered: impl FnMut(&str) -> bool) -> Vec<InstalledEditor> {
    EDITORS
        .iter()
        .filter_map(|(name, bundle_ids)| {
            bundle_ids.iter().find_map(|bundle_identifier| {
                is_registered(bundle_identifier).then_some(InstalledEditor {
                    name,
                    bundle_identifier,
                })
            })
        })
        .collect()
}

pub fn open_in_editor(path: &Path, editor: &InstalledEditor) -> Result<()> {
    if !path.exists() {
        bail!("No existe nada para abrir en {}", path.display());
    }
    let output = Command::new("/usr/bin/open")
        .arg("-b")
        .arg(editor.bundle_identifier)
        .arg(path)
        .output()?;
    if !output.status.success() {
        bail!("{} no pudo abrir {}", editor.name, path.display());
    }
    Ok(())
}

fn bundle_is_registered(bundle_identifier: &str) -> bool {
    let query = format!("kMDItemCFBundleIdentifier == '{bundle_identifier}'");
    Command::new("/usr/bin/mdfind")
        .arg(query)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

/// Read the app's native icon on the UI thread. No files or network requests are needed.
pub fn editor_icon_png(bundle_identifier: &str) -> Option<Vec<u8>> {
    unsafe extern "C" {
        fn vibra_copy_editor_icon_png(
            identifier: *const std::ffi::c_char,
            length: *mut usize,
        ) -> *mut u8;
    }
    let identifier = std::ffi::CString::new(bundle_identifier).ok()?;
    let mut length = 0;
    // SAFETY: both arguments remain valid for the call. The bridge returns an
    // owned malloc allocation of `length` bytes, or null on failure.
    let bytes = unsafe { vibra_copy_editor_icon_png(identifier.as_ptr(), &mut length) };
    if bytes.is_null() {
        return None;
    }
    // SAFETY: copy the bridge's buffer before releasing its allocation exactly once.
    let png = unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec();
    unsafe { libc::free(bytes.cast()) };
    Some(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_returns_only_installed_editors_in_display_order() {
        let mut attempted = Vec::new();
        let editors = installed_editors_with(|bundle_id| {
            attempted.push(bundle_id.to_owned());
            matches!(bundle_id, "com.microsoft.VSCode" | "dev.zed.Zed")
        });

        assert_eq!(
            editors.iter().map(|editor| editor.name).collect::<Vec<_>>(),
            ["Visual Studio Code", "Zed"]
        );
        assert!(attempted.contains(&"com.todesktop.230313mzl4w4u92".to_owned()));
    }

    #[test]
    fn discovery_uses_only_one_registered_bundle_per_editor() {
        let editors = installed_editors_with(|bundle_id| bundle_id.starts_with("com.sublimetext"));
        assert_eq!(editors.len(), 1);
        assert_eq!(editors[0].name, "Sublime Text");
        assert_eq!(editors[0].bundle_identifier, "com.sublimetext.4");
    }
}
