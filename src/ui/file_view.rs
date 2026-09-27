//! Read-only documents loaded independently of Git, with virtualized source lines.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    ClipboardItem, Context, IntoElement, Render, ScrollHandle, SharedString, StyledText, Task,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::ports::files::FileSystemPort;
use crate::ui::syntax::{Highlighter, SyntaxSpan};
use crate::ui::theme::{MONO_FONT, colors};

struct SourceLine {
    text: SharedString,
    spans: Vec<SyntaxSpan>,
}

struct Document {
    text: String,
    lines: Vec<SourceLine>,
    columns: usize,
}

impl Document {
    fn new(path: &str, text: String) -> Self {
        let mut highlighter = Highlighter::for_path(path);
        let mut columns = 0;
        let lines = text
            .trim_start_matches('\u{feff}')
            .split('\n')
            .map(|line| {
                let line = line.trim_end_matches('\r').replace('\t', "    ");
                columns = columns.max(line.chars().count());
                let spans = highlighter.highlight_line(&line);
                SourceLine {
                    text: line.into(),
                    spans,
                }
            })
            .collect();
        Self {
            text,
            lines,
            columns,
        }
    }
}

pub(crate) struct FileView {
    root: PathBuf,
    pub(crate) path: PathBuf,
    port: Arc<dyn FileSystemPort>,
    document: Option<Arc<Document>>,
    error: Option<SharedString>,
    loading: bool,
    generation: u64,
    font_size: f32,
    scroll: UniformListScrollHandle,
    horizontal_scroll: ScrollHandle,
    _load: Option<Task<()>>,
}

impl FileView {
    pub(crate) fn new(
        root: PathBuf,
        path: PathBuf,
        port: Arc<dyn FileSystemPort>,
        font_size: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            root,
            path,
            port,
            document: None,
            error: None,
            loading: false,
            generation: 0,
            font_size,
            scroll: UniformListScrollHandle::new(),
            horizontal_scroll: ScrollHandle::new(),
            _load: None,
        };
        view.reload(cx);
        view
    }

    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        let root = self.root.clone();
        let path = self.path.clone();
        let port = self.port.clone();
        let load = cx.background_spawn(async move {
            port.read_text_file(&root, &path)
                .map(|text| Arc::new(Document::new(&path.to_string_lossy(), text)))
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(document) => {
                        this.document = Some(document);
                        this.error = None;
                    }
                    Err(error) => {
                        this.document = None;
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn set_font_size(&mut self, font_size: f32, cx: &mut Context<Self>) {
        if self.font_size != font_size {
            self.font_size = font_size;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn text(&self) -> Option<&str> {
        self.document
            .as_ref()
            .map(|document| document.text.as_str())
    }
}

impl Render for FileView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let relative_path = self
            .path
            .strip_prefix(&self.root)
            .unwrap_or(&self.path)
            .to_string_lossy()
            .into_owned();
        let document = self.document.clone();
        let font_size = self.font_size;
        let row_height = font_size + 8.0;
        let action = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(5.0))
                .text_size(px(11.0))
                .text_color(colors().muted)
                .cursor_pointer()
                .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
                .child(label)
        };
        div()
            .size_full()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .h(px(36.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .border_b_1()
                    .border_color(colors().border_subtle)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(colors().muted)
                            .child(relative_path),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.0))
                            .text_color(colors().subtle)
                            .child("Read only"),
                    )
                    .when(document.is_some(), |bar| {
                        bar.child(action("file-copy", "Copy").on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(document) = &this.document {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        document.text.clone(),
                                    ));
                                }
                            },
                        )))
                    })
                    .child(
                        action("file-reload", "Reload")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .child(
                        action("file-open-external", "Open in app").on_click(
                            cx.listener(|this, _, _, cx| cx.open_with_system(&this.path)),
                        ),
                    ),
            )
            .when_some(document, |view, document| {
                let width = (document.columns as f32 * font_size * 0.65 + 80.0).max(320.0);
                let count = document.lines.len();
                view.child(
                    div()
                        .id("file-horizontal-scroll")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_x_scroll()
                        .track_scroll(&self.horizontal_scroll)
                        .child(
                            uniform_list(
                                "file-source-lines",
                                count,
                                cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                                    range
                                        .map(|index| {
                                            let line = &document.lines[index];
                                            div()
                                                .h(px(row_height))
                                                .flex()
                                                .items_center()
                                                .font_family(MONO_FONT)
                                                .text_size(px(font_size))
                                                .child(
                                                    div()
                                                        .w(px(56.0))
                                                        .flex_none()
                                                        .pr(px(12.0))
                                                        .text_right()
                                                        .text_color(colors().subtle)
                                                        .child((index + 1).to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .whitespace_nowrap()
                                                        .text_color(colors().foreground)
                                                        .child(
                                                            StyledText::new(line.text.clone())
                                                                .with_highlights(
                                                                    line.spans.iter().map(|span| {
                                                                        (
                                                                            span.range.clone(),
                                                                            span.kind
                                                                                .highlight_style(),
                                                                        )
                                                                    }),
                                                                ),
                                                        ),
                                                )
                                        })
                                        .collect()
                                }),
                            )
                            .track_scroll(self.scroll.clone())
                            .w(px(width))
                            .min_w_full()
                            .h_full(),
                        ),
                )
            })
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .p(px(24.0))
                        .text_size(px(13.0))
                        .text_color(colors().muted)
                        .child(error),
                )
            })
            .when(self.loading && self.document.is_none(), |view| {
                view.child(
                    div()
                        .p(px(24.0))
                        .text_color(colors().muted)
                        .child("Opening file…"),
                )
            })
    }
}
