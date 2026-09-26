//! Explorer toolbar: create files and folders, collapse the tree, refresh.

use std::path::{Component, Path, PathBuf};

use gpui::{AnyElement, Context, MouseButton, SharedString, div, prelude::*, px, svg};

use crate::ports::files::FileEntryKind;
use crate::ui::theme::{colors, surface_tint};

use super::{RenamePrompt, RenamePromptKind, WorkspaceView, sidebar_tooltip};

/// Creates `name` inside `directory`, never outside `root` and never over an
/// existing entry. Returns the new path.
pub(super) fn create_project_entry(
    root: &Path,
    directory: &Path,
    name: &str,
    folder: bool,
) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Escribe un nombre.".to_owned());
    }
    // Nested names (`src/lib.rs`) are allowed; escaping the project is not.
    let relative = Path::new(name);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("El nombre no puede salir de la carpeta del proyecto.".to_owned());
    }
    if !directory.starts_with(root) {
        return Err("La carpeta está fuera del proyecto.".to_owned());
    }
    let path = directory.join(relative);
    if path.symlink_metadata().is_ok() {
        return Err(format!("Ya existe {}.", path.display()));
    }
    let result = if folder {
        std::fs::create_dir_all(&path)
    } else {
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map(|_| ())
            })
    };
    result.map_err(|error| format!("No se pudo crear {}: {error}", path.display()))?;
    Ok(path)
}

impl WorkspaceView {
    /// Folder that new entries go into: the selected folder, the selected
    /// file's folder, or the project root.
    fn new_entry_directory(&self) -> PathBuf {
        let root = self.project_root();
        let selected = self
            .selected_file_path
            .as_ref()
            .filter(|path| path.starts_with(&root));
        let Some(selected) = selected else {
            return root;
        };
        let is_directory = self
            .project_files
            .iter()
            .find(|row| &row.entry.path == selected)
            .is_some_and(|row| row.entry.kind == FileEntryKind::Directory);
        if is_directory {
            selected.clone()
        } else {
            selected
                .parent()
                .map(Path::to_path_buf)
                .filter(|parent| parent.starts_with(&root))
                .unwrap_or(root)
        }
    }

    fn begin_new_entry(&mut self, folder: bool, cx: &mut Context<Self>) {
        let directory = self.new_entry_directory();
        self.context_menu = None;
        self.palette_mode = None;
        self.settings_open = false;
        self.rename_prompt = Some(RenamePrompt {
            kind: if folder {
                RenamePromptKind::NewFolder { directory }
            } else {
                RenamePromptKind::NewFile { directory }
            },
            value: String::new(),
        });
        cx.notify();
    }

    /// Called from the name prompt; keeps the prompt open on errors.
    pub(super) fn confirm_new_entry(
        &mut self,
        directory: PathBuf,
        name: &str,
        folder: bool,
        cx: &mut Context<Self>,
    ) {
        match create_project_entry(&self.project_root(), &directory, name, folder) {
            Ok(path) => {
                self.rename_prompt = None;
                self.persistence_error = None;
                let mut parent = path.parent();
                while let Some(directory) = parent {
                    self.expanded_directories.insert(directory.to_path_buf());
                    if directory == self.project_root() {
                        break;
                    }
                    parent = directory.parent();
                }
                if folder {
                    self.expanded_directories.insert(path.clone());
                }
                self.selected_file_path = Some(path);
                self.refresh_project_files(cx);
            }
            Err(error) => self.persistence_error = Some(error.into()),
        }
        cx.notify();
    }

    fn collapse_file_tree(&mut self, cx: &mut Context<Self>) {
        let root = self.project_root();
        self.expanded_directories.retain(|path| *path == root);
        self.refresh_project_files(cx);
    }

    pub(super) fn explorer_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let name = self
            .snapshot
            .selected_project()
            .map(|project| project.name.to_uppercase())
            .unwrap_or_default();
        let action = |id: &'static str, icon: &'static str, label: &'static str| {
            div()
                .id(SharedString::from(id))
                .size(px(24.0))
                .flex_none()
                .rounded(px(5.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(colors().subtle)
                .hover(|button| {
                    button
                        .bg(surface_tint(colors().hover, colors().panel))
                        .text_color(colors().foreground)
                })
                .tooltip(move |_, cx| sidebar_tooltip(label, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(svg().path(icon).size(px(14.0)))
        };
        div()
            .h(px(30.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.0))
            .pl_3()
            .pr_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors().muted)
                    .child(name),
            )
            .child(
                action(
                    "explorer-new-file",
                    "chrome-icons/file-plus.svg",
                    "Nuevo archivo",
                )
                .on_click(cx.listener(|this, _, _, cx| this.begin_new_entry(false, cx))),
            )
            .child(
                action(
                    "explorer-new-folder",
                    "chrome-icons/folder-plus.svg",
                    "Nueva carpeta",
                )
                .on_click(cx.listener(|this, _, _, cx| this.begin_new_entry(true, cx))),
            )
            .child(
                action(
                    "explorer-collapse",
                    "chrome-icons/collapse-all.svg",
                    "Colapsar carpetas",
                )
                .on_click(cx.listener(|this, _, _, cx| this.collapse_file_tree(cx))),
            )
            .child(
                action("explorer-refresh", "chrome-icons/refresh.svg", "Actualizar")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_project_files(cx))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_created_inside_the_project_only() {
        let root = std::env::temp_dir().join(format!("vibra-explorer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = create_project_entry(&root, &root, "src/lib.rs", false).unwrap();
        assert!(file.is_file());
        let folder = create_project_entry(&root, &root.join("src"), "nested", true).unwrap();
        assert!(folder.is_dir());
        assert!(create_project_entry(&root, &root, "src/lib.rs", false).is_err());
        assert!(create_project_entry(&root, &root, "../escape", false).is_err());
        assert!(create_project_entry(&root, &root, "/tmp/abs", true).is_err());
        assert!(create_project_entry(&root, &root, "  ", true).is_err());
        assert!(create_project_entry(&root, Path::new("/tmp"), "x", true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
