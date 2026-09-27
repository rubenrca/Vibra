use super::*;

impl WorkspaceView {
    pub(super) fn inbox_code_modes(
        &self,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode = state.map(|state| state.code_mode).unwrap_or_default();
        segmented_control()
            .children(
                [
                    (CodeMode::Hunks, "Hunks"),
                    (CodeMode::FullFile, "Full file"),
                ]
                .into_iter()
                .map(|(value, label)| {
                    segment(
                        SharedString::from(format!("inbox-code-mode-{label}")),
                        label,
                        mode == value,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_inbox_code_mode(value, cx)),
                    )
                }),
            )
            .into_any_element()
    }

    pub(super) fn inbox_diff_body(
        &self,
        _item: &WorkItem,
        state: Option<&ItemState>,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .flex()
            .flex_col();
        if let Some(error) = state.and_then(|state| state.diff.error.as_ref()) {
            body = body.child(message(error, true));
        }
        if let Some(view) = state.and_then(|state| state.review.as_ref()) {
            body = body.child(view.clone());
        } else if state.is_none_or(|state| state.diff.loading || state.diff.error.is_none()) {
            body = body.child(message("Loading pull request files…", false));
        }
        if state
            .and_then(|state| state.diff.data.as_ref())
            .is_some_and(|diff| diff.truncated)
        {
            body = body.child(message(
                "The file list is truncated. View the rest on GitHub.",
                true,
            ));
        }
        body.into_any_element()
    }
}

pub(super) fn segmented_control() -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .p(px(2.0))
        .gap(px(2.0))
        .border_1()
        .border_color(colors().border_subtle)
        .rounded(px(7.0))
        .bg(surface_tint(colors().panel, colors().background))
}

pub(super) fn segment(
    id: SharedString,
    label: impl Into<SharedString>,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(26.0))
        .px(px(10.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(5.0))
        .text_size(px(12.0))
        .cursor_pointer()
        .text_color(if selected {
            colors().foreground
        } else {
            colors().subtle
        })
        .when(selected, |button| {
            button.bg(surface_tint(colors().selection, colors().background))
        })
        .hover(|button| button.text_color(colors().foreground))
        .child(label.into())
}
