mod domain;
mod infrastructure;
mod ports;
mod ui;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use directories::BaseDirs;
use gpui::{
    Action, App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, SystemMenuType,
    TitlebarOptions, WindowBounds, WindowOptions, actions, px, size,
};

use crate::infrastructure::alacritty::AlacrittyTerminalPort;
use crate::infrastructure::files::LocalFileSystemPort;
use crate::infrastructure::git::GitCliPort;
use crate::infrastructure::persistence::WorkspaceRepository;
use crate::infrastructure::settings::{MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, SettingsRepository};
use crate::ui::agent_marks::VibraAssets;
use crate::ui::workspace_view::{WorkspaceDependencies, WorkspaceView};

actions!(
    vibra,
    [
        NewWorkspace,
        NewTerminalTab,
        CloseTerminal,
        ToggleDevTerminal,
        ToggleLeftSidebar,
        ToggleRightSidebar,
        PreviousWorkspace,
        NextWorkspace,
        CopyTerminal,
        PasteTerminal,
        SearchTerminal,
        SearchTerminalNext,
        SearchTerminalPrevious,
        IncreaseTerminalFontSize,
        DecreaseTerminalFontSize,
        ResetTerminalFontSize,
        ClearTerminalScrollback,
        SplitPaneLeft,
        SplitPaneRight,
        SplitPaneUp,
        SplitPaneDown,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        PreviousPane,
        NextPane,
        ResizePaneLeft,
        ResizePaneRight,
        ResizePaneUp,
        ResizePaneDown,
        EqualizePanes,
        TogglePaneZoom,
        ToggleCommandPalette,
        QuickOpen,
        OpenIde,
        ShowSettings,
        CheckForUpdates,
        Quit
    ]
);

/// Ghostty-style tab jump. `1..=8` select that tab (or the last one if there
/// aren't enough); `9` always selects the last tab.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = vibra, no_json)]
pub struct GoToTab {
    pub index: usize,
}

fn launch_directory() -> PathBuf {
    resolve_launch_directory(
        std::env::args_os().skip(1).map(PathBuf::from),
        std::env::current_dir().ok(),
        BaseDirs::new().map(|directories| directories.home_dir().to_path_buf()),
    )
}

fn resolve_launch_directory(
    arguments: impl IntoIterator<Item = PathBuf>,
    current_directory: Option<PathBuf>,
    home_directory: Option<PathBuf>,
) -> PathBuf {
    arguments
        .into_iter()
        .find(|path| path.is_dir())
        .or_else(|| {
            current_directory.filter(|path| path != std::path::Path::new("/") && path.is_dir())
        })
        .or_else(|| {
            home_directory.and_then(|home| {
                let development = home.join("Dev");
                development
                    .is_dir()
                    .then_some(development)
                    .or_else(|| home.is_dir().then_some(home))
            })
        })
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn run() -> Result<()> {
    let repository = WorkspaceRepository::for_current_user()
        .context("no se pudo resolver el directorio de datos de Vibra")?;
    let settings_repository = SettingsRepository::for_current_user()
        .context("no se pudo resolver settings.json de Vibra")?;
    let initial_settings = settings_repository.load().unwrap_or_default();
    let launch_directory = launch_directory();

    Application::new()
        .with_assets(VibraAssets)
        .run(move |cx: &mut App| {
            cx.text_system()
                .add_fonts(vec![
                    Cow::Borrowed(include_bytes!("../Resources/Fonts/JetBrainsMono[wght].ttf")),
                    Cow::Borrowed(include_bytes!(
                        "../Resources/Fonts/JetBrainsMono-Italic[wght].ttf"
                    )),
                ])
                .expect("no se pudo cargar JetBrains Mono");

            cx.bind_keys([
                KeyBinding::new("cmd-n", NewWorkspace, None),
                KeyBinding::new("cmd-t", NewTerminalTab, None),
                KeyBinding::new("cmd-w", CloseTerminal, None),
                KeyBinding::new("cmd-j", ToggleDevTerminal, None),
                KeyBinding::new("cmd-b", ToggleLeftSidebar, None),
                KeyBinding::new("alt-cmd-b", ToggleRightSidebar, None),
                KeyBinding::new("ctrl-cmd-[", PreviousWorkspace, None),
                KeyBinding::new("ctrl-cmd-]", NextWorkspace, None),
                KeyBinding::new("cmd-c", CopyTerminal, Some("Terminal")),
                KeyBinding::new("cmd-v", PasteTerminal, Some("Terminal")),
                KeyBinding::new("cmd-f", SearchTerminal, Some("Terminal")),
                KeyBinding::new("cmd-g", SearchTerminalNext, Some("Terminal")),
                KeyBinding::new("shift-cmd-g", SearchTerminalPrevious, Some("Terminal")),
                KeyBinding::new("cmd-=", IncreaseTerminalFontSize, Some("Terminal")),
                KeyBinding::new("cmd--", DecreaseTerminalFontSize, Some("Terminal")),
                KeyBinding::new("cmd-0", ResetTerminalFontSize, Some("Terminal")),
                KeyBinding::new("cmd-k", ClearTerminalScrollback, Some("Terminal")),
                KeyBinding::new("cmd-1", GoToTab { index: 1 }, None),
                KeyBinding::new("cmd-2", GoToTab { index: 2 }, None),
                KeyBinding::new("cmd-3", GoToTab { index: 3 }, None),
                KeyBinding::new("cmd-4", GoToTab { index: 4 }, None),
                KeyBinding::new("cmd-5", GoToTab { index: 5 }, None),
                KeyBinding::new("cmd-6", GoToTab { index: 6 }, None),
                KeyBinding::new("cmd-7", GoToTab { index: 7 }, None),
                KeyBinding::new("cmd-8", GoToTab { index: 8 }, None),
                KeyBinding::new("cmd-9", GoToTab { index: 9 }, None),
                KeyBinding::new("cmd-d", SplitPaneRight, None),
                KeyBinding::new("shift-cmd-d", SplitPaneDown, None),
                KeyBinding::new("ctrl-alt-cmd-left", SplitPaneLeft, None),
                KeyBinding::new("ctrl-alt-cmd-right", SplitPaneRight, None),
                KeyBinding::new("ctrl-alt-cmd-up", SplitPaneUp, None),
                KeyBinding::new("ctrl-alt-cmd-down", SplitPaneDown, None),
                KeyBinding::new("alt-cmd-left", FocusPaneLeft, None),
                KeyBinding::new("alt-cmd-right", FocusPaneRight, None),
                KeyBinding::new("alt-cmd-up", FocusPaneUp, None),
                KeyBinding::new("alt-cmd-down", FocusPaneDown, None),
                KeyBinding::new("cmd-[", PreviousPane, None),
                KeyBinding::new("cmd-]", NextPane, None),
                KeyBinding::new("ctrl-alt-left", ResizePaneLeft, None),
                KeyBinding::new("ctrl-alt-right", ResizePaneRight, None),
                KeyBinding::new("ctrl-alt-up", ResizePaneUp, None),
                KeyBinding::new("ctrl-alt-down", ResizePaneDown, None),
                KeyBinding::new("ctrl-alt-e", EqualizePanes, None),
                KeyBinding::new("shift-cmd-enter", TogglePaneZoom, None),
                KeyBinding::new("shift-cmd-p", ToggleCommandPalette, None),
                KeyBinding::new("cmd-p", QuickOpen, None),
                KeyBinding::new("shift-cmd-e", OpenIde, None),
                KeyBinding::new("cmd-,", ShowSettings, None),
                KeyBinding::new("cmd-u", CheckForUpdates, None),
                KeyBinding::new("cmd-q", Quit, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_action(|_: &CheckForUpdates, _cx| {
                infrastructure::sparkle::check_for_updates();
            });

            // Application menu becomes "Vibra" in the macOS menu bar (click the name).
            cx.set_menus(vec![
                Menu {
                    name: "Vibra".into(),
                    items: vec![
                        MenuItem::action("Settings…", ShowSettings),
                        MenuItem::action("Check for Updates…", CheckForUpdates),
                        MenuItem::separator(),
                        MenuItem::os_submenu("Services", SystemMenuType::Services),
                        MenuItem::separator(),
                        MenuItem::action("Quit Vibra", Quit),
                    ],
                },
                Menu {
                    name: "File".into(),
                    items: vec![
                        MenuItem::action("New Workspace", NewWorkspace),
                        MenuItem::action("New Terminal Tab", NewTerminalTab),
                        MenuItem::separator(),
                        MenuItem::action("Open Current Folder in…", OpenIde),
                        MenuItem::separator(),
                        MenuItem::action("Close Terminal", CloseTerminal),
                    ],
                },
                Menu {
                    name: "Edit".into(),
                    items: vec![
                        MenuItem::action("Copy", CopyTerminal),
                        MenuItem::action("Paste", PasteTerminal),
                    ],
                },
                Menu {
                    name: "View".into(),
                    items: vec![
                        MenuItem::action("Toggle Sessions Sidebar", ToggleLeftSidebar),
                        MenuItem::action("Toggle Files / Git", ToggleRightSidebar),
                        MenuItem::action("Toggle Dev Terminal", ToggleDevTerminal),
                        MenuItem::separator(),
                        MenuItem::action("Command Palette", ToggleCommandPalette),
                        MenuItem::action("Quick Open", QuickOpen),
                    ],
                },
                Menu {
                    name: "Window".into(),
                    items: vec![
                        MenuItem::action("Previous Workspace", PreviousWorkspace),
                        MenuItem::action("Next Workspace", NextWorkspace),
                        MenuItem::separator(),
                        MenuItem::action("Go to Tab 1", GoToTab { index: 1 }),
                        MenuItem::action("Go to Tab 2", GoToTab { index: 2 }),
                        MenuItem::action("Go to Tab 3", GoToTab { index: 3 }),
                        MenuItem::action("Go to Tab 4", GoToTab { index: 4 }),
                        MenuItem::action("Go to Tab 5", GoToTab { index: 5 }),
                        MenuItem::action("Go to Tab 6", GoToTab { index: 6 }),
                        MenuItem::action("Go to Tab 7", GoToTab { index: 7 }),
                        MenuItem::action("Go to Tab 8", GoToTab { index: 8 }),
                        MenuItem::action("Go to Last Tab", GoToTab { index: 9 }),
                    ],
                },
            ]);

            // Packaged builds carry SUFeedURL; development runs are a no-op.
            infrastructure::sparkle::start();

            let (mut window_width, mut window_height) = (
                initial_settings.window_width,
                initial_settings.window_height,
            );
            if let Some(display) = cx.primary_display() {
                let display_bounds = display.bounds();
                let display_width: f32 = display_bounds.size.width.into();
                let display_height: f32 = display_bounds.size.height.into();
                window_width = window_width.min((display_width - 40.0).max(MIN_WINDOW_WIDTH));
                window_height = window_height.min((display_height - 40.0).max(MIN_WINDOW_HEIGHT));
            }
            let bounds = Bounds::centered(None, size(px(window_width), px(window_height)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Vibra".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(gpui::point(px(14.0), px(14.0))),
                    }),
                    // On macOS, `is_movable: true` lets AppKit claim drags that begin on
                    // interactive titlebar controls. Vibra opts into window movement only on
                    // the explicit empty regions of its custom titlebar instead.
                    is_movable: !cfg!(target_os = "macos"),
                    ..Default::default()
                },
                |window, cx| {
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window);
                    cx.new(|cx| {
                        WorkspaceView::new(
                            WorkspaceDependencies {
                                repository,
                                settings_repository,
                                terminal_port: Arc::new(AlacrittyTerminalPort),
                                file_port: Arc::new(LocalFileSystemPort),
                                git_port: Arc::new(GitCliPort),
                            },
                            launch_directory,
                            focus_handle,
                            cx,
                        )
                    })
                },
            )
            .expect("no se pudo abrir la ventana principal de Vibra");

            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });

    Ok(())
}

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    match infrastructure::automation::run_cli(&arguments) {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("Vibra agent tracking: {error:#}");
            std::process::exit(2);
        }
    }
    if let Err(error) = run() {
        eprintln!("Vibra no pudo iniciar: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod launch_directory_tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn finder_launch_uses_the_users_dev_directory_instead_of_root() {
        let home = std::env::temp_dir().join(format!("vibra-home-{}", Uuid::new_v4()));
        let development = home.join("Dev");
        std::fs::create_dir_all(&development).unwrap();

        let resolved = resolve_launch_directory(Vec::new(), Some("/".into()), Some(home.clone()));

        assert_eq!(resolved, development);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn explicit_directory_still_has_priority() {
        let root = std::env::temp_dir().join(format!("vibra-launch-{}", Uuid::new_v4()));
        let explicit = root.join("explicit");
        let current = root.join("current");
        let home = root.join("home");
        std::fs::create_dir_all(&explicit).unwrap();
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(home.join("Dev")).unwrap();

        let resolved = resolve_launch_directory(
            vec![root.join("missing"), explicit.clone()],
            Some(current),
            Some(home),
        );

        assert_eq!(resolved, explicit);
        std::fs::remove_dir_all(root).unwrap();
    }
}
