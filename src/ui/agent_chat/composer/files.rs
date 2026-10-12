//! The project file list behind `@` references. Scans run on the background
//! executor and are reused for a short time, so typing in the popup stays
//! responsive and the list never blocks the UI thread.

use std::time::{Duration, Instant};

use gpui::{Context, prelude::*};

use super::super::AgentChatView;
use super::popups::active_mention;

/// How long a completed scan is reused before the next `@` popup rescans.
const FILE_INDEX_TTL: Duration = Duration::from_secs(10);

impl AgentChatView {
    /// Starts a scan when the draft is in an `@` reference and the list is stale.
    pub(super) fn refresh_file_index_if_mentioning(&mut self, cx: &mut Context<Self>) {
        if active_mention(&self.draft).is_some() {
            self.refresh_file_index(cx);
        }
    }

    fn refresh_file_index(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.file_search.clone() else {
            return;
        };
        let state = &self.composer;
        let fresh = state.files_root.as_ref() == Some(&search.root)
            && state
                .files_scanned_at
                .is_some_and(|at| at.elapsed() < FILE_INDEX_TTL);
        if fresh || state.files_scanning {
            return;
        }
        self.composer.files_scanning = true;
        let root = search.root.clone();
        let scan = cx.background_spawn(async move {
            let mut paths = Vec::new();
            // The port keeps the files found before a failing directory, so use them either way.
            let _ = search.port.search_files(&search.root, &mut paths);
            paths
                .iter()
                .filter_map(|path| path.strip_prefix(&search.root).ok())
                .map(|relative| relative.to_string_lossy().into_owned())
                .collect::<Vec<String>>()
        });
        cx.spawn(async move |this, cx| {
            let files = scan.await;
            let _ = this.update(cx, |view, cx| {
                view.composer.files = files;
                view.composer.files_root = Some(root);
                view.composer.files_scanned_at = Some(Instant::now());
                view.composer.files_scanning = false;
                cx.notify();
            });
        })
        .detach();
    }
}
