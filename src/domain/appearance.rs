use serde::{Deserialize, Serialize};

/// How the app chooses light versus dark for themes that support both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceMode {
    Light,
    Dark,
    #[default]
    #[serde(other)]
    System,
}
