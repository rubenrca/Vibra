use std::path::PathBuf;

use directories::{BaseDirs, ProjectDirs};

const APP_SUPPORT_DIRECTORY: &str = "Vibra";

pub fn application_support_directory() -> Option<PathBuf> {
    Some(BaseDirs::new()?.data_dir().join(APP_SUPPORT_DIRECTORY))
}

pub fn gpui_preview_support_directory() -> Option<PathBuf> {
    ProjectDirs::from("dev", "rubenrca", "VibraGPUI")
        .map(|directories| directories.data_dir().to_path_buf())
}
