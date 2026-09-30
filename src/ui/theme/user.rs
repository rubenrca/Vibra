//! User theme identities, legacy aliases, and light/dark family discovery.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use super::{Theme, ThemeFamily, pretty_label, scheme_is_dark, theme_from_scheme};
use crate::ui::theme_import;

const MAX_USER_THEMES: usize = 200;
const USER_PREFIX: &str = "user:";

#[derive(Default)]
struct UserThemeCatalog {
    families: Vec<ThemeFamily>,
    aliases: HashMap<String, String>,
}

impl UserThemeCatalog {
    fn find(&self, id: &str) -> Option<&ThemeFamily> {
        if let Some(family) = self.families.iter().find(|family| family.id == id) {
            return Some(family);
        }
        let canonical = self.aliases.get(id).map_or(id, String::as_str);
        self.families.iter().find(|family| family.id == canonical)
    }
}

fn catalog() -> &'static RwLock<UserThemeCatalog> {
    static CATALOG: OnceLock<RwLock<UserThemeCatalog>> = OnceLock::new();
    CATALOG.get_or_init(|| RwLock::new(UserThemeCatalog::default()))
}

pub fn user_themes_directory() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".vibra/themes"))
}

pub fn user_themes() -> Vec<ThemeFamily> {
    catalog()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .families
        .clone()
}

pub fn refresh_user_themes() {
    let next = user_themes_directory()
        .map(|path| load_catalog(&path))
        .unwrap_or_default();
    *catalog().write().unwrap_or_else(|error| error.into_inner()) = next;
}

pub(super) fn find_user_theme(id: &str) -> Option<ThemeFamily> {
    catalog()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .find(id)
        .cloned()
}

#[derive(Clone)]
struct LoadedTheme {
    filename: String,
    stem: String,
    label: String,
    dark: bool,
    theme: Theme,
}

struct PreparedFamily {
    theme: ThemeFamily,
    group: String,
    filenames: Vec<String>,
}

fn load_catalog(directory: &Path) -> UserThemeCatalog {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return UserThemeCatalog::default();
    };
    let mut loaded = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if loaded.len() == MAX_USER_THEMES
            && loaded
                .last_key_value()
                .is_some_and(|(last, _)| &path >= last)
        {
            continue;
        }
        if let Some(theme) = load_theme(path.clone()) {
            loaded.insert(path, theme);
            if loaded.len() > MAX_USER_THEMES {
                loaded.pop_last();
            }
        }
    }
    let loaded = loaded.into_values().collect::<Vec<_>>();
    let legacy = prepare_families(loaded.clone(), true);
    let prepared = prepare_families(loaded, false);
    let mut catalog = UserThemeCatalog::default();

    for prepared in &prepared {
        let family = &prepared.theme;
        for filename in &prepared.filenames {
            catalog
                .aliases
                .insert(stable_id("file", filename), family.id.clone());
            if let Some(stem) = Path::new(filename)
                .file_stem()
                .and_then(|stem| stem.to_str())
            {
                let suffix_alias =
                    format!("{USER_PREFIX}{}:{}", slug(stem), hex_identity(filename));
                catalog.aliases.insert(suffix_alias, family.id.clone());
            }
        }
        // A removed partner still leaves the same logical family available.
        catalog
            .aliases
            .entry(stable_id("pair", &prepared.group))
            .or_insert(family.id.clone());
    }
    let mut seen = std::collections::HashSet::new();
    for legacy in legacy {
        let mut id = legacy.theme.id;
        let filename = &legacy.filenames[0];
        if !seen.insert(id.clone()) {
            id.push(':');
            id.push_str(&hex_identity(filename));
        }
        if let Some(canonical) = catalog.aliases.get(&stable_id("file", filename)).cloned() {
            catalog.aliases.insert(id, canonical);
        }
    }
    catalog.families = prepared
        .into_iter()
        .map(|prepared| prepared.theme)
        .collect();
    catalog
}

fn load_theme(path: PathBuf) -> Option<LoadedTheme> {
    if !theme_import::is_theme_file(&path) {
        return None;
    }
    let scheme = theme_import::parse_theme_file(&path).ok()?;
    let filename = path.file_name()?.to_str()?.to_owned();
    let stem = path.file_stem()?.to_str()?.to_owned();
    Some(LoadedTheme {
        label: scheme
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| pretty_label(&stem)),
        dark: scheme_is_dark(&scheme),
        theme: theme_from_scheme(&scheme),
        filename,
        stem,
    })
}

fn prepare_families(loaded: Vec<LoadedTheme>, legacy: bool) -> Vec<PreparedFamily> {
    let mut groups: BTreeMap<String, Vec<LoadedTheme>> = BTreeMap::new();
    for theme in loaded {
        let group = if legacy {
            legacy_group(&theme.stem)
        } else {
            raw_group(&theme.stem)
        };
        groups.entry(group).or_default().push(theme);
    }
    let mut families = Vec::new();
    for (group, mut members) in groups {
        members.sort_by(|a, b| {
            a.stem
                .cmp(&b.stem)
                .then_with(|| a.filename.cmp(&b.filename))
        });
        let light = members.iter().find(|theme| !theme.dark);
        let dark = members.iter().find(|theme| theme.dark);
        if members.len() == 2
            && let (Some(light), Some(dark)) = (light, dark)
        {
            families.push(PreparedFamily {
                theme: ThemeFamily {
                    id: if legacy {
                        format!(
                            "{USER_PREFIX}{}",
                            if slug(&group).is_empty() {
                                "theme"
                            } else {
                                &group
                            }
                        )
                    } else {
                        stable_id("pair", &group)
                    },
                    label: preferred_label(&light.label, &dark.label, &group, legacy),
                    light: light.theme,
                    dark: dark.theme,
                },
                group,
                filenames: vec![light.filename.clone(), dark.filename.clone()],
            });
        } else {
            families.extend(members.into_iter().map(|theme| PreparedFamily {
                theme: ThemeFamily {
                    id: if legacy {
                        format!("{USER_PREFIX}{}", slug(&theme.stem))
                    } else {
                        stable_id("file", &theme.filename)
                    },
                    label: theme.label,
                    light: theme.theme,
                    dark: theme.theme,
                },
                group: group.clone(),
                filenames: vec![theme.filename],
            }));
        }
    }
    families.sort_by(|a, b| {
        a.theme
            .label
            .to_ascii_lowercase()
            .cmp(&b.theme.label.to_ascii_lowercase())
            .then_with(|| a.theme.id.cmp(&b.theme.id))
            .then_with(|| a.filenames[0].cmp(&b.filenames[0]))
    });
    families
}

fn preferred_label(light: &str, dark: &str, group: &str, legacy: bool) -> String {
    let key = if legacy { legacy_group } else { raw_group };
    if key(light) == key(dark) || key(light) == group {
        light.to_owned()
    } else if key(dark) == group {
        dark.to_owned()
    } else {
        pretty_label(group)
    }
}

fn raw_group(stem: &str) -> String {
    let lower = stem.to_ascii_lowercase();
    [
        "-dark", "_dark", "-light", "_light", "-darker", "-lighter", " dark", " light",
    ]
    .iter()
    .find_map(|suffix| lower.strip_suffix(suffix))
    .unwrap_or(&lower)
    .to_owned()
}

fn legacy_group(stem: &str) -> String {
    let raw = raw_group(stem);
    let id = slug(&raw);
    if !id.is_empty() {
        id
    } else if raw.is_ascii() {
        "theme".into()
    } else {
        raw
    }
}

fn slug(raw: &str) -> String {
    let mut output = String::new();
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
        } else if !output.is_empty() && !output.ends_with('-') {
            output.push('-');
        }
    }
    output.trim_matches('-').to_owned()
}

fn stable_id(kind: &str, identity: &str) -> String {
    format!("{USER_PREFIX}{kind}:{}", hex_identity(identity))
}

fn hex_identity(identity: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(identity.len() * 2);
    for byte in identity.bytes() {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::TerminalRgb;

    #[test]
    fn user_themes_load_and_pair_light_dark_files() {
        let root = std::env::temp_dir().join(format!("vibra-themes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nord.yaml"),
            r###"
name: Nord
background: "#2E3440"
foreground: "#D8DEE9"
details: darker
accent: "#81A1C1"
terminal_colors:
  normal:
    black: "#3B4252"
    red: "#BF616A"
    green: "#A3BE8C"
    yellow: "#EBCB8B"
    blue: "#81A1C1"
    magenta: "#B48EAD"
    cyan: "#88C0D0"
    white: "#E5E9F0"
  bright:
    black: "#4C566A"
    red: "#BF616A"
    green: "#A3BE8C"
    yellow: "#EBCB8B"
    blue: "#81A1C1"
    magenta: "#B48EAD"
    cyan: "#8FBCBB"
    white: "#ECEFF4"
"###,
        )
        .unwrap();
        std::fs::write(
            root.join("nord-light.yaml"),
            r###"
name: Nord Light
background: "#ECEFF4"
foreground: "#2E3440"
details: lighter
accent: "#5E81AC"
"###,
        )
        .unwrap();
        std::fs::write(
            root.join("solo.conf"),
            "background = #101010\nforeground = #ffffff\npalette = 1=#ff8080\n",
        )
        .unwrap();

        let catalog = load_catalog(&root);
        std::fs::remove_dir_all(&root).unwrap();
        let nord = catalog.find("user:nord").unwrap();
        assert!(nord.dark.is_dark());
        assert!(!nord.light.is_dark());
        assert_eq!(
            nord.dark.terminal_palette().ansi[1],
            TerminalRgb::new(0xbf, 0x61, 0x6a)
        );
        let solo = catalog.find("user:solo").unwrap();
        assert_eq!(solo.light.background, solo.dark.background);
    }

    #[test]
    fn colliding_user_theme_slugs_remain_selectable_and_stable() {
        let root = std::env::temp_dir().join(format!("vibra-theme-ids-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = "background = 101010\nforeground = ffffff\n";
        for filename in [
            "same-name.conf",
            "same_name.conf",
            "same-name.yaml",
            "紫.conf",
            "緑.conf",
        ] {
            let source = if filename == "緑.conf" {
                "background = eceff4\nforeground = 111111\n"
            } else {
                source
            };
            std::fs::write(root.join(filename), source).unwrap();
        }
        let catalog = load_catalog(&root);
        let first = &catalog.families;
        let ids = |themes: &[ThemeFamily]| {
            themes
                .iter()
                .map(|theme| theme.id.clone())
                .collect::<Vec<_>>()
        };
        let first_ids = ids(first);
        assert_eq!(first.len(), 5);
        assert_eq!(
            first_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            5
        );
        assert!(catalog.find("user:same-name").is_some());
        assert!(catalog.find("user:").is_some());
        assert_eq!(first_ids, ids(&load_catalog(&root).families));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn matching_unicode_light_dark_filenames_preserve_the_legacy_pair_id() {
        let root = std::env::temp_dir().join(format!("vibra-theme-pair-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("紫-dark.conf"),
            "background = 101010\nforeground = ffffff\n",
        )
        .unwrap();
        std::fs::write(
            root.join("紫-light.conf"),
            "background = eceff4\nforeground = 111111\n",
        )
        .unwrap();
        let catalog = load_catalog(&root);
        let families = &catalog.families;
        assert_eq!(families.len(), 1);
        assert_eq!(catalog.find("user:theme").unwrap().id, families[0].id);
        assert!(families[0].dark.is_dark());
        assert!(!families[0].light.is_dark());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_theme_survives_adding_and_removing_a_colliding_slug() {
        let root =
            std::env::temp_dir().join(format!("vibra-theme-stable-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("same_name.conf"),
            "background = 101010\nforeground = ffffff\n",
        )
        .unwrap();
        let original = load_catalog(&root);
        let selected = original.find("user:same-name").unwrap().id.clone();
        let background = original.find(&selected).unwrap().dark.background;
        let old_suffixed = format!("user:same-name:{}", hex_identity("same_name.conf"));

        std::fs::write(
            root.join("same-name.conf"),
            "background = 202020\nforeground = ffffff\n",
        )
        .unwrap();
        let collision = load_catalog(&root);
        assert_eq!(
            collision.find(&selected).unwrap().dark.background,
            background
        );
        assert_eq!(collision.find(&old_suffixed).unwrap().id, selected);
        std::fs::remove_file(root.join("same-name.conf")).unwrap();
        let remaining = load_catalog(&root);
        assert_eq!(
            remaining.find(&selected).unwrap().dark.background,
            background
        );
        assert_eq!(remaining.find(&old_suffixed).unwrap().id, selected);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paired_theme_and_member_preferences_follow_a_removed_or_added_partner() {
        let root =
            std::env::temp_dir().join(format!("vibra-theme-member-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let dark = root.join("nord-dark.conf");
        let light = root.join("nord-light.conf");
        std::fs::write(&dark, "background = 101010\nforeground = ffffff\n").unwrap();
        let single = load_catalog(&root);
        let member_id = single.find("user:nord-dark").unwrap().id.clone();
        std::fs::write(&light, "background = eceff4\nforeground = 111111\n").unwrap();
        let paired = load_catalog(&root);
        let pair_id = paired.find("user:nord").unwrap().id.clone();
        assert_eq!(paired.find(&member_id).unwrap().id, pair_id);
        std::fs::remove_file(light).unwrap();
        let remaining = load_catalog(&root);
        assert_eq!(remaining.find(&pair_id).unwrap().id, member_id);
        std::fs::remove_dir_all(root).unwrap();
    }
}
