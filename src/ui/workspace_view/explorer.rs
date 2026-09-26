//! Explorer loading, selection, tree rendering, and file/folder creation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, Context, MouseButton, MouseDownEvent, SharedString, div, prelude::*, px, svg,
    uniform_list,
};

use crate::ports::files::FileEntryKind;
use crate::ports::git::GitFileStatus;
use crate::ui::theme::{colors, surface_tint};

use super::files::{
    aggregate_dir_statuses, collect_project_files, file_tree_icon, file_tree_icon_color,
    git_status_color, git_status_trailing, relative_repo_path,
};
use super::{
    ProjectFileRow, RenamePrompt, RenamePromptKind, RightSidebarMode, WorkspaceView,
    sidebar_tooltip,
};

impl WorkspaceView {
    /// Refresh the Explorer from the stable project root.
    pub(super) fn refresh_project_files(&mut self, cx: &mut Context<Self>) {
        self.files_request_id = self.files_request_id.wrapping_add(1);
        let root = self.has_project_context().then(|| self.project_root());
        if self.project_files_root != root {
            self.project_files_root.clone_from(&root);
            self.project_files = Arc::new(Vec::new());
            self.selected_file_path = None;
            self.file_error = None;
        }
        let Some(root) = root else {
            self._files_task = None;
            self.files_watch = None;
            return;
        };
        self.expanded_directories.insert(root.clone());
        if self
            .selected_file_path
            .as_ref()
            .is_some_and(|path| !path.starts_with(&root))
        {
            self.selected_file_path = None;
        }
        let request_id = self.files_request_id;
        let expanded = self.expanded_directories.clone();
        let show_hidden = self.settings.show_hidden_files;
        let port = self.file_port.clone();
        self.sync_files_watcher(cx);
        let task = cx.background_spawn(async move {
            let mut rows = Vec::new();
            let result = collect_project_files(
                port.as_ref(),
                &root,
                &root,
                0,
                &expanded,
                show_hidden,
                &mut rows,
            );
            (result, rows)
        });
        self._files_task = Some(cx.spawn(async move |this, cx| {
            let (result, rows) = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.files_request_id {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.project_files = Arc::new(rows);
                        this.file_error = None;
                        if this
                            .selected_file_path
                            .as_ref()
                            .is_some_and(|path| !path.exists())
                        {
                            this.selected_file_path = None;
                        }
                    }
                    Err(error) => {
                        this.project_files = Arc::new(rows);
                        this.file_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn toggle_directory(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.expanded_directories.remove(path) {
            self.expanded_directories.insert(path.to_path_buf());
        }
        self.selected_file_path = Some(path.to_path_buf());
        self.refresh_project_files(cx);
        cx.notify();
    }

    pub(super) fn select_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selected_file_path = Some(path);
        cx.notify();
    }

    pub(super) fn files_sidebar_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let rows = Arc::clone(&self.project_files);
        let selected_path = self.selected_file_path.clone();
        let file_error = self.file_error.clone();
        let project_root = self.project_root();
        let (git_root, git_statuses) = self.diff_view.read(cx).status_index();
        let status_root = git_root.unwrap_or_else(|| project_root.clone());
        let dir_statuses: HashMap<String, _> = aggregate_dir_statuses(&git_statuses)
            .into_iter()
            .map(|(path, status)| (path.to_owned(), status))
            .collect();

        div()
            .id("project-files-content")
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.explorer_toolbar(cx))
            // File tree
            .child(
                div()
                    .id("project-file-tree")
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .px_1()
                    .pt_1()
                    .pb_2()
                    .child(
                        uniform_list(
                            "project-file-rows",
                            rows.len(),
                            cx.processor(
                                move |this, range: std::ops::Range<usize>, _window, cx| {
                                    range
                                        .map(|index| {
                                            let row = &rows[index];
                                            let path = row.entry.path.clone();
                                            let selected = selected_path.as_ref() == Some(&path);
                                            let is_directory =
                                                row.entry.kind == FileEntryKind::Directory;
                                            let rel = relative_repo_path(&path, &status_root);
                                            let status = if is_directory {
                                                rel.as_deref()
                                                    .and_then(|rel| dir_statuses.get(rel).copied())
                                            } else {
                                                rel.as_ref()
                                                    .and_then(|rel| git_statuses.get(rel).copied())
                                            };
                                            this.project_file_row(row, selected, rel, status, cx)
                                        })
                                        .collect()
                                },
                            ),
                        )
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full(),
                    ),
            )
            .when_some(file_error, |panel, error| {
                panel.child(
                    div()
                        .mx_2()
                        .mb_1()
                        .p_2()
                        .rounded(px(5.0))
                        .bg(colors().diff_deleted_bg)
                        .text_size(px(9.0))
                        .text_color(colors().danger)
                        .child(error),
                )
            })
            .into_any_element()
    }

    fn project_file_row(
        &self,
        row: &ProjectFileRow,
        selected: bool,
        rel_for_click: Option<String>,
        status: Option<GitFileStatus>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = row.entry.path.clone();
        let is_directory = row.entry.kind == FileEntryKind::Directory;
        let icon_color = if is_directory {
            status.map(git_status_color).unwrap_or(colors().folder)
        } else {
            file_tree_icon_color(row.entry.kind, &row.entry.name)
        };
        let name_color = status.map(git_status_color).unwrap_or(if is_directory {
            colors().foreground
        } else {
            colors().muted
        });
        let depth = row.depth;
        let expanded = row.expanded;
        div()
            .id(SharedString::from(format!(
                "file-row-{}",
                path.to_string_lossy()
            )))
            .h(px(26.0))
            .w_full()
            .flex()
            .items_center()
            .pr_2()
            .mb(px(1.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .bg(if selected {
                surface_tint(colors().elevated, colors().panel)
            } else {
                gpui::rgba(0x00000000)
            })
            .hover(|item| item.bg(surface_tint(colors().hover, colors().panel)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if is_directory {
                        if event.click_count == 1 {
                            this.toggle_directory(&path, cx);
                        }
                        return;
                    }
                    this.select_file_path(path.clone(), cx);
                    if let Some(rel) = rel_for_click.as_ref() {
                        // Selecting any changed file peeks it in Git; documents
                        // never replace the terminal surface.
                        let selected = this
                            .diff_view
                            .update(cx, |diff, cx| diff.select_path_if_changed(rel, cx));
                        if selected {
                            this.right_sidebar_mode = RightSidebarMode::Diff;
                            this.set_right_sidebar_visible(true, true, cx);
                            this.diff_view.read(cx).focus_review(window);
                        }
                    }
                }),
            )
            // Indent + soft guide for nested rows.
            .child(
                div()
                    .w(px(6.0 + depth as f32 * 12.0))
                    .h_full()
                    .flex_none()
                    .relative()
                    .when(depth > 0, |indent| {
                        indent.child(
                            div()
                                .absolute()
                                .left(px(6.0 + (depth as f32 - 1.0) * 12.0 + 5.0))
                                .top_0()
                                .bottom_0()
                                .w(px(1.0))
                                .bg(colors().indent_guide),
                        )
                    }),
            )
            // Expand chevron (folders) or spacer (files).
            .child(
                div()
                    .w(px(12.0))
                    .h(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(is_directory, |slot| {
                        slot.child(
                            gpui::svg()
                                .path(if expanded {
                                    "chrome-icons/chevron-down.svg"
                                } else {
                                    "chrome-icons/chevron-right.svg"
                                })
                                .size(px(9.0))
                                .flex_none()
                                .text_color(colors().subtle),
                        )
                    }),
            )
            // Folder / file icon.
            .child(
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(icon_color)
                    .child(file_tree_icon(
                        row.entry.kind,
                        expanded,
                        &row.entry.name,
                        icon_color,
                    )),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .truncate()
                    .pl_1()
                    .text_size(px(12.0))
                    .font_weight(if selected || is_directory {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(if selected && status.is_none() {
                        colors().foreground
                    } else {
                        name_color
                    })
                    .child(row.entry.name.clone()),
            )
            .when_some(status.map(git_status_trailing), |row, trailing| {
                row.child(trailing)
            })
            .into_any_element()
    }

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
        self.close_palette(cx);
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
        match self
            .file_port
            .create_entry(&self.project_root(), &directory, name, folder)
        {
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
            Err(error) => self.persistence_error = Some(error.to_string().into()),
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
