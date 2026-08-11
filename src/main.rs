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
    App, AppContext, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions,
    actions, px, size,
};

use crate::infrastructure::alacritty::AlacrittyTerminalPort;
use crate::infrastructure::files::LocalFileSystemPort;
use crate::infrastructure::git::GitCliPort;
use crate::infrastructure::persistence::WorkspaceRepository;
use crate::infrastructure::settings::SettingsRepository;
use crate::ui::agent_marks::VibraAssets;
use crate::ui::workspace_view::{WorkspaceDependencies, WorkspaceView};

actions!(
    vibra,
    [
        NewWorkspace,
        NewTerminalTab,
        CloseTerminal,
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
        CheckForUpdates,
        Quit
    ]
);

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
            KeyBinding::new("cmd-u", CheckForUpdates, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &CheckForUpdates, _cx| {
            infrastructure::sparkle::check_for_updates();
        });

        // Packaged builds carry SUFeedURL; development runs are a no-op.
        infrastructure::sparkle::start();

        let bounds = Bounds::centered(None, size(px(1240.0), px(780.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.0), px(580.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Vibra".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(gpui::point(px(14.0), px(14.0))),
                }),
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
            eprintln!("Vibra automation: {error:#}");
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
