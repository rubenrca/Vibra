//! The permission request that needs an answer, docked above the composer
//! card where the reader is already looking.

use gpui::{AnyElement, Context, MouseButton, Rgba, SharedString, div, prelude::*, px};

use super::super::AgentChatView;
use super::super::tool_card::icon;
use crate::domain::agent_chat::{
    ChatItem, PermissionItem, PermissionOption, PermissionOptionKind, ToolCallItem, ToolKind,
};
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

/// Gap the composer puts between its parts. The banner cancels it so it sits
/// directly on the card.
const COMPOSER_GAP: f32 = 4.0;

impl AgentChatView {
    /// The oldest unanswered permission request, with its answers; `None` when
    /// nothing is waiting.
    pub(super) fn approval_dock(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let items = &self.timeline.items;
        let (index, permission, waiting) = oldest_pending(items)?;
        let request_id = permission.request_id;
        let call = linked_call(items, permission.tool_call_id.as_deref());

        let header = div()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(11.5))
            .text_color(colors().warning)
            .child(icon("chrome-icons/shield.svg", colors().warning))
            .child(approval_label(call.map(|call| call.kind)))
            .when(waiting > 1, |header| {
                header.child(
                    div()
                        .ml_auto()
                        .flex_none()
                        .text_color(colors().muted)
                        .child(format!("1/{waiting}")),
                )
            });

        let detail = match approval_detail(&permission.title, call) {
            ApprovalDetail::Command(command) => div()
                .font_family(MONO_FONT)
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(colors().foreground)
                .child(command),
            ApprovalDetail::Paths(paths) => div()
                .flex()
                .flex_col()
                .font_family(MONO_FONT)
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(colors().foreground)
                .children(
                    paths
                        .into_iter()
                        .map(|path| div().min_w(px(0.0)).child(path)),
                ),
            ApprovalDetail::Title(title) => div()
                .text_size(px(13.0))
                .line_height(px(19.0))
                .text_color(colors().foreground)
                .child(title),
        };
        let detail = div()
            .id("chat-approval-detail")
            .w_full()
            .min_w(px(0.0))
            .max_h(px(80.0))
            .overflow_y_scroll()
            .child(detail);

        let actions = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap(px(6.0))
            .children(
                ordered_options(&permission.options)
                    .into_iter()
                    .enumerate()
                    .map(|(option_index, option)| {
                        self.answer_button(request_id, index, option_index, option, cx)
                    }),
            );

        Some(
            div()
                .w_full()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .px(px(14.0))
                .pt(px(10.0))
                .pb(px(10.0))
                .mb(px(-COMPOSER_GAP))
                .rounded_t(px(14.0))
                .rounded_b(px(4.0))
                .border_1()
                .border_b_0()
                .border_color(surface_tint(colors().warning, colors().terminal))
                .bg(surface_tint(colors().elevated, colors().terminal))
                .child(header)
                .child(detail)
                .child(actions)
                .into_any_element(),
        )
    }

    /// One answer as a button. Allow once is the primary action, reject once an
    /// outline, and the "always" answers quiet ghosts.
    fn answer_button(
        &self,
        request_id: u64,
        index: usize,
        option_index: usize,
        option: &PermissionOption,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let option_id = option.id.clone();
        let primary = option.kind == PermissionOptionKind::AllowOnce;
        let outline = option.kind == PermissionOptionKind::RejectOnce;
        div()
            .id(SharedString::from(format!(
                "chat-approval-{request_id}-{option_index}"
            )))
            .flex_none()
            .max_w(px(180.0))
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(Rgba {
                a: 0.0,
                ..colors().accent
            })
            .text_size(px(12.0))
            .cursor_pointer()
            .truncate()
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
            .when(outline, |button| {
                button
                    .border_color(colors().border_subtle)
                    .text_color(colors().foreground)
                    .hover(|button| button.bg(colors().hover))
            })
            .when(!primary && !outline, |button| {
                button
                    .text_color(colors().muted)
                    .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.answer_permission(request_id, option_id.clone(), index, cx);
            }))
            .child(option.name.clone())
            .into_any_element()
    }
}

/// The oldest permission request still waiting for an answer, its index in
/// `items`, and how many requests are waiting in all.
fn oldest_pending(items: &[ChatItem]) -> Option<(usize, &PermissionItem, usize)> {
    let mut waiting = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match item {
            ChatItem::Permission(permission) if permission.resolution.is_none() => {
                Some((index, permission))
            }
            _ => None,
        });
    let (index, first) = waiting.next()?;
    Some((index, first, waiting.count() + 1))
}

/// The tool call a request is for, when the timeline has it.
fn linked_call<'a>(items: &'a [ChatItem], tool_call_id: Option<&str>) -> Option<&'a ToolCallItem> {
    let id = tool_call_id?;
    items.iter().find_map(|item| match item {
        ChatItem::ToolCall(call) if call.id == id => Some(call),
        _ => None,
    })
}

/// Heading for a request, named after the kind of tool it is for.
fn approval_label(kind: Option<ToolKind>) -> &'static str {
    match kind {
        Some(ToolKind::Execute) => "Command approval",
        Some(ToolKind::Edit | ToolKind::Delete | ToolKind::Move) => "File change approval",
        Some(ToolKind::Read) => "File read approval",
        _ => "Permission request",
    }
}

/// What a request is about, shown under its heading.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ApprovalDetail {
    /// The command a shell call would run.
    Command(String),
    /// The paths a file call touches, one per line.
    Paths(Vec<String>),
    /// The request's own title.
    Title(String),
}

/// The command for a shell call, the paths for a file call, and the request's
/// own title for anything else.
fn approval_detail(title: &str, call: Option<&ToolCallItem>) -> ApprovalDetail {
    match call {
        Some(call) if call.kind == ToolKind::Execute && !call.title.trim().is_empty() => {
            ApprovalDetail::Command(call.title.clone())
        }
        Some(call)
            if matches!(
                call.kind,
                ToolKind::Read | ToolKind::Edit | ToolKind::Delete | ToolKind::Move
            ) && !call.locations.is_empty() =>
        {
            ApprovalDetail::Paths(call.locations.clone())
        }
        _ => ApprovalDetail::Title(title.to_owned()),
    }
}

/// The answers in the order they sit in the row: the quiet "always" choices
/// first, then reject once, and allow once last as the primary action.
fn ordered_options(options: &[PermissionOption]) -> Vec<&PermissionOption> {
    let mut ordered: Vec<&PermissionOption> = options.iter().collect();
    ordered.sort_by_key(|option| action_rank(option.kind));
    ordered
}

fn action_rank(kind: PermissionOptionKind) -> u8 {
    match kind {
        PermissionOptionKind::AllowAlways | PermissionOptionKind::RejectAlways => 0,
        PermissionOptionKind::RejectOnce => 1,
        PermissionOptionKind::AllowOnce => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::{NoticeLevel, PermissionResolution, ToolStatus};

    fn request(request_id: u64, resolved: bool) -> ChatItem {
        ChatItem::Permission(PermissionItem {
            request_id,
            tool_call_id: None,
            title: format!("Request {request_id}"),
            options: Vec::new(),
            resolution: resolved.then_some(PermissionResolution::Cancelled),
        })
    }

    fn tool_call(id: &str, kind: ToolKind, title: &str, locations: &[&str]) -> ToolCallItem {
        ToolCallItem {
            id: id.to_owned(),
            title: title.to_owned(),
            kind,
            status: ToolStatus::Pending,
            content: Vec::new(),
            locations: locations.iter().map(|path| (*path).to_owned()).collect(),
        }
    }

    fn answer(id: &str, kind: PermissionOptionKind) -> PermissionOption {
        PermissionOption {
            id: id.to_owned(),
            name: id.to_owned(),
            kind,
        }
    }

    #[test]
    fn oldest_pending_skips_answered_and_other_items() {
        let items = vec![
            ChatItem::UserMessage {
                text: "go".to_owned(),
                files: Vec::new(),
            },
            request(1, true),
            request(2, false),
            ChatItem::Notice {
                level: NoticeLevel::Info,
                text: "note".to_owned(),
            },
            request(3, false),
        ];
        let (index, permission, waiting) = oldest_pending(&items).expect("two are waiting");
        assert_eq!(index, 2);
        assert_eq!(permission.request_id, 2);
        assert_eq!(waiting, 2);
    }

    #[test]
    fn oldest_pending_is_none_when_nothing_waits() {
        assert!(oldest_pending(&[]).is_none());
        assert!(oldest_pending(&[request(1, true)]).is_none());
    }

    #[test]
    fn linked_call_finds_the_call_by_id() {
        let items = vec![ChatItem::ToolCall(tool_call(
            "a",
            ToolKind::Read,
            "Read",
            &[],
        ))];
        assert_eq!(
            linked_call(&items, Some("a")).map(|call| call.id.as_str()),
            Some("a")
        );
        assert!(linked_call(&items, Some("b")).is_none());
        assert!(linked_call(&items, None).is_none());
    }

    #[test]
    fn label_names_the_kind_of_tool() {
        assert_eq!(approval_label(Some(ToolKind::Execute)), "Command approval");
        assert_eq!(approval_label(Some(ToolKind::Edit)), "File change approval");
        assert_eq!(
            approval_label(Some(ToolKind::Delete)),
            "File change approval"
        );
        assert_eq!(approval_label(Some(ToolKind::Move)), "File change approval");
        assert_eq!(approval_label(Some(ToolKind::Read)), "File read approval");
        assert_eq!(approval_label(Some(ToolKind::Fetch)), "Permission request");
        assert_eq!(approval_label(None), "Permission request");
    }

    #[test]
    fn detail_shows_the_command_the_paths_or_the_title() {
        let shell = tool_call("a", ToolKind::Execute, "cargo test", &[]);
        assert_eq!(
            approval_detail("Run", Some(&shell)),
            ApprovalDetail::Command("cargo test".to_owned())
        );

        let edit = tool_call("b", ToolKind::Edit, "Edit", &["src/main.rs", "src/lib.rs"]);
        assert_eq!(
            approval_detail("Allow edit", Some(&edit)),
            ApprovalDetail::Paths(vec!["src/main.rs".to_owned(), "src/lib.rs".to_owned()])
        );

        let unlocated = tool_call("c", ToolKind::Read, "Read", &[]);
        assert_eq!(
            approval_detail("Allow read", Some(&unlocated)),
            ApprovalDetail::Title("Allow read".to_owned())
        );
        assert_eq!(
            approval_detail("Fetch page", None),
            ApprovalDetail::Title("Fetch page".to_owned())
        );
    }

    #[test]
    fn options_put_allow_once_last() {
        let options = vec![
            answer("allow", PermissionOptionKind::AllowOnce),
            answer("reject", PermissionOptionKind::RejectOnce),
            answer("allow-all", PermissionOptionKind::AllowAlways),
            answer("reject-all", PermissionOptionKind::RejectAlways),
        ];
        let ids: Vec<&str> = ordered_options(&options)
            .into_iter()
            .map(|option| option.id.as_str())
            .collect();
        assert_eq!(ids, vec!["allow-all", "reject-all", "reject", "allow"]);
    }
}
