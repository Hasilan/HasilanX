mod client;
mod notify;
mod state;
mod terminal;
mod theme;
mod time_fmt;
mod ui;

use client::MultiHostManager;
use gpui_kit::*;
use theme::UiPreferences;
use tracing_subscriber::EnvFilter;
use ui::{
    CloseOverlay, ComposerAcceptAutocomplete, ComposerNextAutocomplete, ComposerPrevAutocomplete,
    CycleTheme, HasilanxApp, NavigateBack, NavigateForward, NewThreadAction, OpenSettings,
    QuickJump1, QuickJump2, QuickJump3, QuickJump4, QuickJump5, QuickJump6, QuickJump7,
    QuickJump8, QuickJump9, SendPromptAction, ToggleAllTools, ToggleCommandPalette,
    ToggleDiffPane, ToggleFilesPane, ToggleSessionSearch, ToggleSidebar, ToggleSplitPane,
    ToggleTerminalDock,
};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("hasilanx_desktop=info")),
        )
        .init();

    let prefs = UiPreferences::load();
    let (hosts_mgr, initial_hosts) =
        MultiHostManager::start().expect("failed to initialize multi-host manager");

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            prefs.theme.apply(cx);

            cx.bind_keys([
                KeyBinding::new("ctrl-k", ToggleCommandPalette, Some("!Terminal")),
                KeyBinding::new("cmd-k", ToggleCommandPalette, Some("!Terminal")),
                KeyBinding::new("ctrl-n", NewThreadAction, Some("!Terminal")),
                KeyBinding::new("cmd-n", NewThreadAction, Some("!Terminal")),
                KeyBinding::new("ctrl-b", ToggleSidebar, Some("!Terminal")),
                KeyBinding::new("cmd-b", ToggleSidebar, Some("!Terminal")),
                KeyBinding::new("ctrl-shift-d", ToggleDiffPane, None),
                KeyBinding::new("cmd-shift-d", ToggleDiffPane, None),
                KeyBinding::new("ctrl-shift-f", ToggleFilesPane, None),
                KeyBinding::new("cmd-shift-f", ToggleFilesPane, None),
                KeyBinding::new("alt-[", NavigateBack, Some("!Terminal")),
                KeyBinding::new("cmd-[", NavigateBack, Some("!Terminal")),
                KeyBinding::new("alt-]", NavigateForward, Some("!Terminal")),
                KeyBinding::new("cmd-]", NavigateForward, Some("!Terminal")),
                KeyBinding::new("ctrl-`", ToggleTerminalDock, None),
                KeyBinding::new("ctrl-shift-j", ToggleTerminalDock, None),
                KeyBinding::new("cmd-shift-j", ToggleTerminalDock, None),
                KeyBinding::new("ctrl-enter", SendPromptAction, Some("!Terminal")),
                KeyBinding::new("cmd-enter", SendPromptAction, Some("!Terminal")),
                KeyBinding::new(
                    "tab",
                    ComposerAcceptAutocomplete,
                    Some("ComposerAutocomplete > Input"),
                ),
                KeyBinding::new(
                    "up",
                    ComposerPrevAutocomplete,
                    Some("ComposerAutocomplete > Input"),
                ),
                KeyBinding::new(
                    "down",
                    ComposerNextAutocomplete,
                    Some("ComposerAutocomplete > Input"),
                ),
                KeyBinding::new("ctrl-,", OpenSettings, Some("!Terminal")),
                KeyBinding::new("cmd-,", OpenSettings, Some("!Terminal")),
                KeyBinding::new("ctrl-shift-t", CycleTheme, None),
                KeyBinding::new("ctrl-shift-o", ToggleAllTools, None),
                KeyBinding::new("cmd-shift-o", ToggleAllTools, None),
                KeyBinding::new("ctrl-f", ToggleSessionSearch, Some("!Terminal")),
                KeyBinding::new("cmd-f", ToggleSessionSearch, Some("!Terminal")),
                KeyBinding::new("ctrl-\\", ToggleSplitPane, None),
                KeyBinding::new("cmd-\\", ToggleSplitPane, None),
                KeyBinding::new("ctrl-1", QuickJump1, Some("!Terminal")),
                KeyBinding::new("ctrl-2", QuickJump2, Some("!Terminal")),
                KeyBinding::new("ctrl-3", QuickJump3, Some("!Terminal")),
                KeyBinding::new("ctrl-4", QuickJump4, Some("!Terminal")),
                KeyBinding::new("ctrl-5", QuickJump5, Some("!Terminal")),
                KeyBinding::new("ctrl-6", QuickJump6, Some("!Terminal")),
                KeyBinding::new("ctrl-7", QuickJump7, Some("!Terminal")),
                KeyBinding::new("ctrl-8", QuickJump8, Some("!Terminal")),
                KeyBinding::new("ctrl-9", QuickJump9, Some("!Terminal")),
                KeyBinding::new("cmd-1", QuickJump1, Some("!Terminal")),
                KeyBinding::new("cmd-2", QuickJump2, Some("!Terminal")),
                KeyBinding::new("cmd-3", QuickJump3, Some("!Terminal")),
                KeyBinding::new("cmd-4", QuickJump4, Some("!Terminal")),
                KeyBinding::new("cmd-5", QuickJump5, Some("!Terminal")),
                KeyBinding::new("cmd-6", QuickJump6, Some("!Terminal")),
                KeyBinding::new("cmd-7", QuickJump7, Some("!Terminal")),
                KeyBinding::new("cmd-8", QuickJump8, Some("!Terminal")),
                KeyBinding::new("cmd-9", QuickJump9, Some("!Terminal")),
                KeyBinding::new("escape", CloseOverlay, Some("!Terminal")),
            ]);

            let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("HasilanX".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };

            let prefs_win = prefs.clone();
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| {
                    HasilanxApp::new(
                        hosts_mgr.clone(),
                        initial_hosts.clone(),
                        prefs_win.clone(),
                        window,
                        cx,
                    )
                })
            })
            .expect("failed to open HasilanX window");
        });
}
