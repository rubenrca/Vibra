//! The terminal's tabs can stay out of sight: the center card rises over the
//! tab strip to the top of the window, and pointing at the top edge brings it
//! back down with the tabs. Only the center moves; the sidebars, the window
//! controls, and the titlebar's buttons stay where they are. The tabs leave
//! again once the pointer has been away from them for a moment.

use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, Timer, Window, div, prelude::*, px};

use super::chrome::{PANEL_GAP, ease_out_cubic};
use super::{TITLEBAR_HEIGHT, WorkspaceSection, WorkspaceView};

/// How far below the top edge the pointer brings the tabs back.
const REVEAL_ZONE: f32 = 10.0;
/// Room under the tab strip the pointer may stray into before it counts as gone.
const HIDE_SLACK: f32 = 14.0;
/// How long the pointer stays away before the tabs leave.
const HIDE_DELAY: Duration = Duration::from_millis(450);
/// How often the pointer is checked while the tabs are out.
const WATCH_TICK: Duration = Duration::from_millis(100);
const SLIDE_DURATION: Duration = Duration::from_millis(180);
const SLIDE_FRAME: Duration = Duration::from_millis(16);
/// How far the tabs drift up as the card comes over them.
pub(super) const TAB_RISE: f32 = 8.0;

impl WorkspaceView {
    /// Whether the tabs wait for the pointer before showing. Only with both
    /// sidebars open: a closed sidebar leaves its titlebar buttons over the
    /// center, and the card must not cover them.
    pub(super) fn tabs_auto_hide(&self) -> bool {
        self.settings.auto_hide_tabs
            && self.workspace_section == WorkspaceSection::Workspace
            && self.left_sidebar_progress >= 0.999
            && self.right_sidebar_progress >= 0.999
    }

    /// Whether the tab strip should be on screen now.
    pub(super) fn tabs_wanted(&self) -> bool {
        !self.tabs_auto_hide() || self.tabs_revealed
    }

    /// How far the center card rises over the tab strip, in points.
    pub(super) fn tabs_lift(&self) -> f32 {
        if self.workspace_section != WorkspaceSection::Workspace {
            return 0.0;
        }
        (TITLEBAR_HEIGHT - PANEL_GAP) * (1.0 - self.tabs_progress)
    }

    /// Menus, prompts, and drags keep the tabs out until they finish.
    fn tabs_pinned(&self, cx: &Context<Self>) -> bool {
        self.context_menu.is_some()
            || self.ide_menu_open
            || self.rename_prompt.is_some()
            || self.palette_mode.is_some()
            || cx.has_active_drag()
    }

    pub(super) fn reveal_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs_revealed {
            return;
        }
        self.tabs_revealed = true;
        self.watch_tabs_pointer(window, cx);
        cx.notify();
    }

    /// Runs on every render: slides the tabs toward where they should be.
    pub(super) fn sync_tabs_reveal(&mut self, cx: &mut Context<Self>) {
        let target = if self.tabs_wanted() { 1.0 } else { 0.0 };
        if self.tabs_slide_target != target {
            self.slide_tabs(target, cx);
        }
    }

    /// Hides the tabs once the pointer has been away from them long enough.
    /// Only runs while they are out, so a still window does no work.
    fn watch_tabs_pointer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._tabs_watch_task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut away_since: Option<Instant> = None;
            loop {
                Timer::after(WATCH_TICK).await;
                let watching = this
                    .update_in(cx, |this, window, cx| {
                        if !this.tabs_revealed {
                            return false;
                        }
                        if !this.tabs_auto_hide() {
                            this.tabs_revealed = false;
                            return false;
                        }
                        let y: f32 = window.mouse_position().y.into();
                        if y <= TITLEBAR_HEIGHT + HIDE_SLACK || this.tabs_pinned(cx) {
                            away_since = None;
                            return true;
                        }
                        if away_since.get_or_insert_with(Instant::now).elapsed() < HIDE_DELAY {
                            return true;
                        }
                        this.tabs_revealed = false;
                        cx.notify();
                        false
                    })
                    .unwrap_or(false);
                if !watching {
                    break;
                }
            }
        }));
    }

    /// Eases the tab strip toward `to` (1 shown, 0 hidden under the card).
    fn slide_tabs(&mut self, to: f32, cx: &mut Context<Self>) {
        self.tabs_slide_target = to;
        let from = self.tabs_progress;
        let started = Instant::now();
        self._tabs_slide_task = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(SLIDE_FRAME).await;
                let sliding = this
                    .update(cx, |this, cx| {
                        let t = (started.elapsed().as_secs_f32() / SLIDE_DURATION.as_secs_f32())
                            .min(1.0);
                        this.tabs_progress = from + (to - from) * ease_out_cubic(t);
                        cx.notify();
                        t < 1.0
                    })
                    .unwrap_or(false);
                if !sliding {
                    break;
                }
            }
        }));
    }

    /// A thin strip along the top edge, over the center, that brings the
    /// tabs back. It only watches the pointer; clicks reach what lies under it.
    pub(super) fn tabs_reveal_zone(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        (self.tabs_auto_hide() && !self.tabs_revealed).then(|| {
            div()
                .id("tabs-reveal-zone")
                .absolute()
                .top_0()
                .left(px(self.left_sidebar_width()))
                .right(px(self.right_sidebar_width()))
                .h(px(REVEAL_ZONE))
                .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                    if *hovered {
                        this.reveal_tabs(window, cx);
                    }
                }))
                .into_any_element()
        })
    }
}
