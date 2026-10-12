//! Timeline cards for permission requests, plans and notices.

use gpui::{AnyElement, Context, FontWeight, Rgba, SharedString, div, prelude::*, px};

use super::super::AgentChatView;
use super::super::tool_card::{call_summary, icon};
use crate::domain::agent_chat::{
    ChatItem, NoticeLevel, PermissionItem, PermissionOptionKind, PermissionResolution, PlanEntry,
    PlanEntryStatus,
};
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

impl AgentChatView {
    /// A request for approval: the tool it is for, and one button per option.
    /// Once answered it collapses to one line naming the chosen option.
    pub(super) fn permission_card(
        &self,
        index: usize,
        permission: &PermissionItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(resolution) = &permission.resolution {
            return resolved_permission(permission, resolution);
        }
        let request_id = permission.request_id;
        let tool = permission.tool_call_id.as_deref().and_then(|id| {
            self.timeline.items.iter().find_map(|item| match item {
                ChatItem::ToolCall(call) if call.id == id => Some(call),
                _ => None,
            })
        });
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded(px(12.0))
            .border_1()
            .border_color(surface_tint(colors().warning, colors().terminal))
            .bg(surface_tint(colors().elevated, colors().terminal))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.5))
                    .text_color(colors().warning)
                    .child(icon("chrome-icons/shield.svg", colors().warning))
                    .child("Permission requested"),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .line_height(px(19.0))
                    .text_color(colors().foreground)
                    .child(permission.title.clone()),
            )
            .when_some(tool, |card, call| {
                card.child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(MONO_FONT)
                        .text_size(px(12.0))
                        .text_color(colors().muted)
                        .child(call_summary(call)),
                )
            })
            .child(
                div().flex().flex_wrap().gap_2().children(
                    permission
                        .options
                        .iter()
                        .enumerate()
                        .map(|(option_index, option)| {
                            let option_id = option.id.clone();
                            // The first allow option is the primary action; the rest stay quiet.
                            let primary = option.kind == PermissionOptionKind::AllowOnce;
                            div()
                                .id(SharedString::from(format!(
                                    "chat-permission-{index}-{option_index}"
                                )))
                                .px_3()
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .text_size(px(12.0))
                                .when(primary, |button| {
                                    button
                                        .bg(Rgba {
                                            a: 0.16,
                                            ..colors().accent
                                        })
                                        .text_color(colors().accent)
                                        .hover(|button| {
                                            button.bg(Rgba {
                                                a: 0.24,
                                                ..colors().accent
                                            })
                                        })
                                })
                                .when(!primary, |button| {
                                    button
                                        .bg(colors().hover)
                                        .text_color(colors().foreground)
                                        .hover(|button| button.bg(colors().selection))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.answer_permission(
                                        request_id,
                                        option_id.clone(),
                                        index,
                                        cx,
                                    );
                                }))
                                .child(option.name.clone())
                        }),
                ),
            )
            .into_any_element()
    }
}

/// One muted line that names the answer, with an icon for its kind.
fn resolved_permission(
    permission: &PermissionItem,
    resolution: &PermissionResolution,
) -> AnyElement {
    let (icon_path, color, answer) = match resolution {
        PermissionResolution::Cancelled => (
            "chrome-icons/minus.svg",
            colors().subtle,
            "Cancelled".to_owned(),
        ),
        PermissionResolution::Selected(option_id) => {
            match permission
                .options
                .iter()
                .find(|option| option.id == *option_id)
            {
                Some(option) => match option.kind {
                    PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways => (
                        "chrome-icons/check.svg",
                        colors().success,
                        option.name.clone(),
                    ),
                    PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways => (
                        "chrome-icons/close.svg",
                        colors().danger,
                        option.name.clone(),
                    ),
                },
                None => ("chrome-icons/minus.svg", colors().subtle, option_id.clone()),
            }
        }
    };
    div()
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .min_w(px(0.0))
        .text_size(px(12.0))
        .text_color(colors().muted)
        .child(icon(icon_path, color))
        .child(
            div()
                .min_w(px(0.0))
                .truncate()
                .child(format!("{} · {answer}", permission.title)),
        )
        .into_any_element()
}

/// Compact checklist for the agent's plan.
pub(super) fn plan(entries: &[PlanEntry]) -> AnyElement {
    let done = entries
        .iter()
        .filter(|entry| entry.status == PlanEntryStatus::Completed)
        .count();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p_3()
        .rounded(px(12.0))
        .bg(surface_tint(colors().elevated, colors().terminal))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Plan"),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(colors().subtle)
                        .child(format!("{done}/{} done", entries.len())),
                ),
        )
        .children(entries.iter().map(|entry| {
            let (icon_path, color) = match entry.status {
                PlanEntryStatus::Completed => (Some("chrome-icons/check.svg"), colors().success),
                PlanEntryStatus::InProgress => (Some("chrome-icons/refresh.svg"), colors().accent),
                PlanEntryStatus::Pending => (None, colors().subtle),
            };
            div()
                .flex()
                .items_start()
                .gap_2()
                .text_size(px(12.5))
                .line_height(px(18.0))
                .text_color(if entry.status == PlanEntryStatus::Completed {
                    colors().muted
                } else {
                    colors().foreground
                })
                .child(plan_marker(icon_path, color))
                .child(div().flex_1().min_w(px(0.0)).child(entry.content.clone()))
        }))
        .into_any_element()
}

fn plan_marker(icon_path: Option<&'static str>, color: Rgba) -> AnyElement {
    let marker = div()
        .w(px(13.0))
        .h(px(18.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    match icon_path {
        Some(path) => marker.child(icon(path, color)).into_any_element(),
        None => marker
            .child(
                div()
                    .size(px(9.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(color),
            )
            .into_any_element(),
    }
}

/// Info notices are quiet muted text. Errors are danger-tinted text with a
/// small icon, not a box.
pub(super) fn notice(level: NoticeLevel, text: &str) -> AnyElement {
    match level {
        NoticeLevel::Info => div()
            .w_full()
            .px_1()
            .text_size(px(12.0))
            .line_height(px(18.0))
            .text_color(colors().muted)
            .child(text.to_owned())
            .into_any_element(),
        NoticeLevel::Error => div()
            .w_full()
            .px_1()
            .flex()
            .items_start()
            .gap_2()
            .text_size(px(12.0))
            .line_height(px(18.0))
            .text_color(colors().danger)
            .child(
                div()
                    .h(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(icon("chrome-icons/issue-canceled.svg", colors().danger)),
            )
            .child(div().min_w(px(0.0)).child(text.to_owned()))
            .into_any_element(),
    }
}
