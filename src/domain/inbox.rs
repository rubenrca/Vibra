//! What happened in the terminals while the user looked elsewhere. The Inbox
//! lives only as long as the app: its panes do not survive a restart either.

use std::collections::HashSet;

use uuid::Uuid;

pub const MAX_INBOX_ITEMS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxKind {
    Finished,
    NeedsPermission,
    NeedsAttention,
    AutomationStarted,
    AutomationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    pub id: u64,
    pub kind: InboxKind,
    /// The terminal to open; `None` once it was closed or never started.
    pub pane_id: Option<Uuid>,
    pub title: String,
    pub detail: String,
    pub at: u64,
    pub read: bool,
}

#[derive(Debug, Default)]
pub struct Inbox {
    items: Vec<InboxItem>,
    next_id: u64,
}

impl Inbox {
    /// Newest first.
    pub fn items(&self) -> impl Iterator<Item = &InboxItem> {
        self.items.iter().rev()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn unread_count(&self) -> usize {
        self.items.iter().filter(|item| !item.read).count()
    }

    pub fn push(
        &mut self,
        kind: InboxKind,
        pane_id: Option<Uuid>,
        title: String,
        detail: String,
        at: u64,
        read: bool,
    ) -> u64 {
        // A pane has one live state: a newer event supersedes its unread ones.
        if let Some(pane_id) = pane_id {
            self.items
                .retain(|item| item.read || item.pane_id != Some(pane_id) || is_automation(item));
        }
        self.next_id += 1;
        self.items.push(InboxItem {
            id: self.next_id,
            kind,
            pane_id,
            title,
            detail,
            at,
            read,
        });
        if self.items.len() > MAX_INBOX_ITEMS {
            let overflow = self.items.len() - MAX_INBOX_ITEMS;
            self.items.drain(..overflow);
        }
        self.next_id
    }

    pub fn item(&self, id: u64) -> Option<&InboxItem> {
        self.items.iter().find(|item| item.id == id)
    }

    pub fn mark_read(&mut self, id: u64) -> bool {
        self.items
            .iter_mut()
            .find(|item| item.id == id && !item.read)
            .map(|item| item.read = true)
            .is_some()
    }

    /// Seeing a pane acknowledges everything it reported.
    pub fn mark_pane_read(&mut self, pane_id: Uuid) -> bool {
        let mut changed = false;
        for item in &mut self.items {
            if item.pane_id == Some(pane_id) && !item.read {
                item.read = true;
                changed = true;
            }
        }
        changed
    }

    pub fn mark_all_read(&mut self) -> bool {
        let mut changed = false;
        for item in &mut self.items {
            changed |= !item.read;
            item.read = true;
        }
        changed
    }

    pub fn clear(&mut self) -> bool {
        let changed = !self.items.is_empty();
        self.items.clear();
        changed
    }

    /// Keeps the history of closed panes but stops offering to open them.
    pub fn forget_closed_panes(&mut self, live: &HashSet<Uuid>) -> bool {
        let mut changed = false;
        for item in &mut self.items {
            if item.pane_id.is_some_and(|pane| !live.contains(&pane)) {
                item.pane_id = None;
                changed = true;
            }
        }
        changed
    }
}

fn is_automation(item: &InboxItem) -> bool {
    matches!(
        item.kind,
        InboxKind::AutomationStarted | InboxKind::AutomationFailed
    )
}

/// Short Spanish relative time for list rows.
pub fn relative_time(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..=59 => "ahora".to_owned(),
        60..=3_599 => format!("hace {} min", seconds / 60),
        3_600..=86_399 => format!("hace {} h", seconds / 3_600),
        _ => format!("hace {} d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(inbox: &mut Inbox, kind: InboxKind, pane: Option<Uuid>, read: bool) -> u64 {
        inbox.push(kind, pane, "t".into(), "d".into(), 0, read)
    }

    #[test]
    fn newer_pane_events_replace_unread_ones_and_keep_history() {
        let mut inbox = Inbox::default();
        let pane = Uuid::new_v4();
        let other = Uuid::new_v4();
        let seen = push(&mut inbox, InboxKind::Finished, Some(pane), true);
        push(&mut inbox, InboxKind::NeedsAttention, Some(pane), false);
        push(&mut inbox, InboxKind::Finished, Some(other), false);
        push(&mut inbox, InboxKind::AutomationStarted, Some(pane), false);
        let latest = push(&mut inbox, InboxKind::NeedsPermission, Some(pane), false);
        let kinds: Vec<_> = inbox.items().map(|item| item.kind).collect();
        assert_eq!(
            kinds,
            [
                InboxKind::NeedsPermission,
                InboxKind::AutomationStarted,
                InboxKind::Finished,
                InboxKind::Finished,
            ]
        );
        assert!(inbox.item(seen).is_some());
        assert_eq!(inbox.unread_count(), 3);
        assert!(inbox.mark_pane_read(pane));
        assert!(inbox.item(latest).unwrap().read);
        assert_eq!(inbox.unread_count(), 1);
        assert!(inbox.mark_all_read());
        assert!(!inbox.mark_all_read());
    }

    #[test]
    fn closed_panes_lose_their_link_and_history_is_bounded() {
        let mut inbox = Inbox::default();
        let pane = Uuid::new_v4();
        let id = push(&mut inbox, InboxKind::Finished, Some(pane), false);
        assert!(inbox.forget_closed_panes(&HashSet::new()));
        assert_eq!(inbox.item(id).unwrap().pane_id, None);
        for _ in 0..MAX_INBOX_ITEMS + 5 {
            push(&mut inbox, InboxKind::AutomationStarted, None, false);
        }
        assert_eq!(inbox.items().count(), MAX_INBOX_ITEMS);
        assert!(inbox.item(id).is_none());
        assert!(inbox.clear());
        assert!(inbox.is_empty());
    }

    #[test]
    fn relative_times_are_short() {
        assert_eq!(relative_time(100, 90), "ahora");
        assert_eq!(relative_time(600, 0), "hace 10 min");
        assert_eq!(relative_time(7_200, 0), "hace 2 h");
        assert_eq!(relative_time(172_800, 0), "hace 2 d");
        assert_eq!(relative_time(0, 10), "ahora");
    }
}
