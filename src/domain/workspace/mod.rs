pub mod layout;
mod migrate;
mod ops;
mod projects;
mod tab_moves;
mod types;

pub use types::*;

pub const MAX_NAME_CHARS: usize = 128;
pub const MAX_SESSION_TITLE_CHARS: usize = 256;
pub const MAX_AGENT_TASK_TITLE_CHARS: usize = 80;
pub const DEFAULT_CONTAINER_NAME: &str = "Terminal";

pub(crate) fn parse_user_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (!name.is_empty() && name.chars().count() <= MAX_NAME_CHARS).then_some(name)
}

pub(crate) fn clipped_title(title: &str, max_chars: usize) -> Option<String> {
    let title = title.trim();
    (!title.is_empty()).then(|| title.chars().take(max_chars).collect())
}

#[cfg(test)]
mod legacy_fixtures;
#[cfg(test)]
mod project_tests;
#[cfg(test)]
mod tests;
