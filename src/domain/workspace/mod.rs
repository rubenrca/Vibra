pub mod layout;
mod migrate;
mod ops;
mod projects;
mod types;

pub use types::*;

pub const MAX_NAME_CHARS: usize = 128;
pub const MAX_SESSION_TITLE_CHARS: usize = 256;

#[cfg(test)]
mod legacy_fixtures;
#[cfg(test)]
mod project_tests;
#[cfg(test)]
mod tests;
