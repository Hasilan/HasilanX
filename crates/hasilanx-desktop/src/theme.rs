use gpui_kit::component::{Theme, ThemeMode, ThemeTokens};
use gpui_kit::{App, Hsla, Rgba, SharedString, rgb, rgba};
use hasilanx_protocol::{ExecutionMode, WorkspaceMode};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

const PREFERRED_MONO_FONTS: &[&str] = &[
    "JetBrains Mono",
    "JetBrains Mono NL",
    "Fira Code",
    "Cascadia Mono",
    "Adwaita Mono",
    "Source Code Pro",
    "Iosevka",
    "Hack",
    "IBM Plex Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Liberation Mono",
    "Ubuntu Mono",
    "SF Mono",
    "Menlo",
    "Monaco",
    "FreeMono",
    "Courier New",
];

pub fn resolve_mono_font_family(cx: &App) -> SharedString {
    static RESOLVED: OnceLock<SharedString> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            let installed = cx.text_system().all_font_names();
            for candidate in PREFERRED_MONO_FONTS {
                if installed.iter().any(|name| name == candidate) {
                    return SharedString::from(*candidate);
                }
            }
            if let Some(mono) = installed
                .iter()
                .find(|name| name.to_ascii_lowercase().contains("mono"))
            {
                return SharedString::from(mono.clone());
            }
            Theme::global(cx).mono_font_family.clone()
        })
        .clone()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreset {
    #[default]
    #[serde(alias = "light")]
    ZeronLight,
    #[serde(alias = "t3_dark")]
    ZeronDark,
    Grove,
    Ocean,
    Ember,
    Iris,
}

impl ThemePreset {
    pub fn all() -> &'static [Self] {
        &[
            Self::ZeronLight,
            Self::ZeronDark,
            Self::Grove,
            Self::Ocean,
            Self::Ember,
            Self::Iris,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::ZeronDark => "Zeron Dark",
            Self::ZeronLight => "Zeron Light",
            Self::Grove => "Grove",
            Self::Ocean => "Ocean",
            Self::Ember => "Ember",
            Self::Iris => "Iris",
        }
    }

    pub fn next(&self) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|p| p == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }

    pub fn is_dark(&self) -> bool {
        !matches!(self, Self::ZeronLight)
    }

    pub fn palette(&self) -> UiPalette {
        match self {
            Self::ZeronDark => UiPalette {
                is_dark: true,
                window_bg: rgb(0x07070a),
                canvas: rgb(0x13131a),
                sidebar: rgb(0x0e0e14),
                surface: rgb(0x1a1a24),
                surface_elevated: rgb(0x21212e),
                surface_hover: rgb(0x292938),
                surface_selected: rgb(0x272342),
                user_pill: rgb(0x222230),
                hairline: rgb(0x262634),
                hairline_strong: rgb(0x39394e),
                tree_line: rgba(0xf2f2f726),
                text: rgb(0xf2f2f7),
                text_muted: rgb(0xa3a3b2),
                text_faint: rgb(0x6d6d7d),
                accent: rgb(0x8b7cf6),
                accent_hover: rgb(0x9e91f9),
                accent_soft: rgba(0x8b7cf628),
                accent_fg: rgb(0xffffff),
                running: rgb(0x60a5fa),
                running_soft: rgba(0x60a5fa24),
                approval: rgb(0xfbbf24),
                approval_soft: rgba(0xfbbf2424),
                success: rgb(0x34d399),
                success_soft: rgba(0x34d39922),
                danger: rgb(0xf87171),
                danger_soft: rgba(0xf8717124),
                terminal_bg: rgb(0x0c0c11),
                overlay_backdrop: rgba(0x00000099),
            },
            Self::ZeronLight => UiPalette {
                is_dark: false,
                window_bg: rgb(0xececf1),
                canvas: rgb(0xffffff),
                sidebar: rgb(0xf7f7fa),
                surface: rgb(0xf2f2f7),
                surface_elevated: rgb(0xffffff),
                surface_hover: rgb(0xe8e9f0),
                surface_selected: rgb(0xebe6fb),
                user_pill: rgb(0xefeff4),
                hairline: rgb(0xe3e4ec),
                hairline_strong: rgb(0xcfd2df),
                tree_line: rgba(0x11182722),
                text: rgb(0x16161d),
                text_muted: rgb(0x5a5a68),
                text_faint: rgb(0x8e8e9c),
                accent: rgb(0x6b4ce6),
                accent_hover: rgb(0x5b3bd8),
                accent_soft: rgba(0x6b4ce61c),
                accent_fg: rgb(0xffffff),
                running: rgb(0x2563eb),
                running_soft: rgba(0x2563eb18),
                approval: rgb(0xd97706),
                approval_soft: rgba(0xd977061a),
                success: rgb(0x059669),
                success_soft: rgba(0x05966918),
                danger: rgb(0xdc2626),
                danger_soft: rgba(0xdc262618),
                terminal_bg: rgb(0xfbfbfd),
                overlay_backdrop: rgba(0x0f101855),
            },
            Self::Grove => UiPalette {
                is_dark: true,
                window_bg: rgb(0x060a08),
                canvas: rgb(0x0c1310),
                sidebar: rgb(0x09100d),
                surface: rgb(0x121d18),
                surface_elevated: rgb(0x17251f),
                surface_hover: rgb(0x1e3028),
                surface_selected: rgb(0x1f362c),
                user_pill: rgb(0x182620),
                hairline: rgb(0x1e3028),
                hairline_strong: rgb(0x2d473c),
                tree_line: rgba(0xecfdf524),
                text: rgb(0xecfdf5),
                text_muted: rgb(0x94b8a9),
                text_faint: rgb(0x5d7c6f),
                accent: rgb(0x10b981),
                accent_hover: rgb(0x34d399),
                accent_soft: rgba(0x10b98126),
                accent_fg: rgb(0x042f2e),
                running: rgb(0x38bdf8),
                running_soft: rgba(0x38bdf822),
                approval: rgb(0xf59e0b),
                approval_soft: rgba(0xf59e0b22),
                success: rgb(0x34d399),
                success_soft: rgba(0x34d39920),
                danger: rgb(0xf87171),
                danger_soft: rgba(0xf8717122),
                terminal_bg: rgb(0x060a08),
                overlay_backdrop: rgba(0x00000099),
            },
            Self::Ocean => UiPalette {
                is_dark: true,
                window_bg: rgb(0x050910),
                canvas: rgb(0x0a111d),
                sidebar: rgb(0x080e18),
                surface: rgb(0x101a2c),
                surface_elevated: rgb(0x152238),
                surface_hover: rgb(0x1c2d4a),
                surface_selected: rgb(0x1e3456),
                user_pill: rgb(0x15233a),
                hairline: rgb(0x1c2c47),
                hairline_strong: rgb(0x2b4269),
                tree_line: rgba(0xf0f9ff24),
                text: rgb(0xf0f9ff),
                text_muted: rgb(0x93adc9),
                text_faint: rgb(0x5a7391),
                accent: rgb(0x38bdf8),
                accent_hover: rgb(0x7dd3fc),
                accent_soft: rgba(0x38bdf826),
                accent_fg: rgb(0x082f49),
                running: rgb(0x60a5fa),
                running_soft: rgba(0x60a5fa22),
                approval: rgb(0xfbbf24),
                approval_soft: rgba(0xfbbf2422),
                success: rgb(0x34d399),
                success_soft: rgba(0x34d39920),
                danger: rgb(0xfb7185),
                danger_soft: rgba(0xfb718522),
                terminal_bg: rgb(0x050910),
                overlay_backdrop: rgba(0x00000099),
            },
            Self::Ember => UiPalette {
                is_dark: true,
                window_bg: rgb(0x0a0706),
                canvas: rgb(0x130e0c),
                sidebar: rgb(0x0f0b0a),
                surface: rgb(0x1b1411),
                surface_elevated: rgb(0x231a16),
                surface_hover: rgb(0x2d211c),
                surface_selected: rgb(0x38261e),
                user_pill: rgb(0x251b16),
                hairline: rgb(0x2f221c),
                hairline_strong: rgb(0x47332a),
                tree_line: rgba(0xfff7ed24),
                text: rgb(0xfff7ed),
                text_muted: rgb(0xc2a89a),
                text_faint: rgb(0x7d6558),
                accent: rgb(0xf97316),
                accent_hover: rgb(0xfb923c),
                accent_soft: rgba(0xf9731626),
                accent_fg: rgb(0x1c0a00),
                running: rgb(0x38bdf8),
                running_soft: rgba(0x38bdf822),
                approval: rgb(0xfbbf24),
                approval_soft: rgba(0xfbbf2422),
                success: rgb(0x4ade80),
                success_soft: rgba(0x4ade8020),
                danger: rgb(0xf87171),
                danger_soft: rgba(0xf8717122),
                terminal_bg: rgb(0x0a0706),
                overlay_backdrop: rgba(0x00000099),
            },
            Self::Iris => UiPalette {
                is_dark: true,
                window_bg: rgb(0x08060e),
                canvas: rgb(0x100c1a),
                sidebar: rgb(0x0d0915),
                surface: rgb(0x171226),
                surface_elevated: rgb(0x1e1732),
                surface_hover: rgb(0x271e40),
                surface_selected: rgb(0x30244e),
                user_pill: rgb(0x201936),
                hairline: rgb(0x2a2042),
                hairline_strong: rgb(0x3e3061),
                tree_line: rgba(0xf5f3ff24),
                text: rgb(0xf5f3ff),
                text_muted: rgb(0xb3a5d1),
                text_faint: rgb(0x716391),
                accent: rgb(0xa78bfa),
                accent_hover: rgb(0xc4b5fd),
                accent_soft: rgba(0xa78bfa28),
                accent_fg: rgb(0x1e1038),
                running: rgb(0x38bdf8),
                running_soft: rgba(0x38bdf822),
                approval: rgb(0xfbbf24),
                approval_soft: rgba(0xfbbf2422),
                success: rgb(0x4ade80),
                success_soft: rgba(0x4ade8020),
                danger: rgb(0xfb7185),
                danger_soft: rgba(0xfb718522),
                terminal_bg: rgb(0x08060e),
                overlay_backdrop: rgba(0x00000099),
            },
        }
    }

    pub fn apply(self, cx: &mut App) {
        let mode = if self.is_dark() {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        Theme::change(mode, None, cx);

        let mono_family = resolve_mono_font_family(cx);
        let p = self.palette();
        let theme = Theme::global_mut(cx);
        theme.mono_font_family = mono_family;
        theme.colors.background = p.canvas.into();
        theme.colors.foreground = p.text.into();
        theme.colors.muted = p.surface.into();
        theme.colors.muted_foreground = p.text_muted.into();
        theme.colors.secondary = p.surface.into();
        theme.colors.secondary_hover = p.surface_hover.into();
        theme.colors.secondary_active = p.surface_selected.into();
        theme.colors.secondary_foreground = p.text.into();
        theme.colors.sidebar = p.sidebar.into();
        theme.colors.sidebar_border = p.hairline.into();
        theme.colors.sidebar_foreground = p.text.into();
        theme.colors.sidebar_accent = p.surface_selected.into();
        theme.colors.sidebar_accent_foreground = p.text.into();
        theme.colors.border = p.hairline.into();
        theme.colors.input = p.hairline_strong.into();
        theme.colors.primary = p.accent.into();
        theme.colors.primary_hover = p.accent_hover.into();
        theme.colors.primary_active = p.accent.into();
        theme.colors.primary_foreground = p.accent_fg.into();
        theme.colors.button_primary = p.accent.into();
        theme.colors.button_primary_hover = p.accent_hover.into();
        theme.colors.button_primary_active = p.accent.into();
        theme.colors.button_primary_foreground = p.accent_fg.into();
        theme.colors.button_secondary = p.surface_elevated.into();
        theme.colors.button_secondary_hover = p.surface_hover.into();
        theme.colors.button_secondary_active = p.surface_selected.into();
        theme.colors.button_secondary_foreground = p.text.into();
        theme.colors.danger = p.danger.into();
        theme.colors.danger_foreground = rgb(0xffffff).into();
        theme.colors.button_danger = p.danger.into();
        theme.colors.button_danger_hover = p.danger.into();
        theme.colors.button_danger_active = p.danger.into();
        theme.colors.button_danger_foreground = rgb(0xffffff).into();
        theme.colors.accent = p.surface_hover.into();
        theme.colors.accent_foreground = p.text.into();
        theme.colors.popover = p.surface_elevated.into();
        theme.colors.popover_foreground = p.text.into();
        theme.colors.caret = p.accent.into();
        theme.colors.ring = p.accent.into();
        theme.colors.scrollbar_thumb = p.hairline_strong.into();
        theme.colors.scrollbar_thumb_hover = p.text_faint.into();
        theme.tokens = ThemeTokens::from(theme.colors);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackdropStyle {
    #[default]
    Dusk,
    Aurora,
    Minimal,
    None,
}

impl BackdropStyle {
    pub fn all() -> &'static [Self] {
        &[Self::Dusk, Self::Aurora, Self::Minimal, Self::None]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Dusk => "Dusk Halftone",
            Self::Aurora => "Aurora",
            Self::Minimal => "Soft Glow",
            Self::None => "Plain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidebarDensity {
    #[default]
    Regular,
    Compact,
}

impl SidebarDensity {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Regular => "Regular",
            Self::Compact => "Compact",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UiPalette {
    pub is_dark: bool,
    pub window_bg: Rgba,
    pub canvas: Rgba,
    pub sidebar: Rgba,
    pub surface: Rgba,
    pub surface_elevated: Rgba,
    pub surface_hover: Rgba,
    pub surface_selected: Rgba,
    pub user_pill: Rgba,
    pub hairline: Rgba,
    pub hairline_strong: Rgba,
    pub tree_line: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub text_faint: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_soft: Rgba,
    pub accent_fg: Rgba,
    pub running: Rgba,
    pub running_soft: Rgba,
    pub approval: Rgba,
    pub approval_soft: Rgba,
    pub success: Rgba,
    pub success_soft: Rgba,
    pub danger: Rgba,
    pub danger_soft: Rgba,
    pub terminal_bg: Rgba,
    pub overlay_backdrop: Rgba,
}

impl UiPalette {
    pub fn running_hsla(&self) -> Hsla {
        self.running.into()
    }
}

fn default_terminal_font_size() -> f32 {
    13.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiPreferences {
    #[serde(default)]
    pub theme: ThemePreset,
    /// When false (default, recommended for Japanese / CJK IME), Ctrl/Cmd+Enter sends
    /// and plain Enter inserts a newline without interfering with IME conversion.
    #[serde(default)]
    pub send_on_enter: bool,
    #[serde(default)]
    pub backdrop: BackdropStyle,
    #[serde(default)]
    pub sidebar_density: SidebarDensity,
    #[serde(default)]
    pub pinned_threads: Vec<String>,
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: f32,
    #[serde(default)]
    pub default_workspace_mode: WorkspaceMode,
    #[serde(default)]
    pub default_execution_mode: ExecutionMode,
    #[serde(default)]
    pub default_agent_id: Option<String>,
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            theme: ThemePreset::default(),
            send_on_enter: false,
            backdrop: BackdropStyle::default(),
            sidebar_density: SidebarDensity::default(),
            pinned_threads: Vec::new(),
            terminal_font_size: default_terminal_font_size(),
            default_workspace_mode: WorkspaceMode::Local,
            default_execution_mode: ExecutionMode::Ask,
            default_agent_id: None,
        }
    }
}

impl UiPreferences {
    fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("hasilanx/ui.toml"))
    }

    pub fn load() -> Self {
        let Some(p) = Self::path() else {
            return Self::default();
        };
        fs::read_to_string(p)
            .ok()
            .and_then(|raw| toml::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = Self::path() else {
            return;
        };
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = fs::write(p, text);
        }
    }
}

/// Deterministic project badge hue from project name (Zeron-style colored initial tiles).
pub fn project_tile_colors(name: &str, is_dark: bool) -> (Rgba, Rgba) {
    let hash: usize = name
        .bytes()
        .fold(5381usize, |acc, b| acc.wrapping_mul(33).wrapping_add(b as usize));
    if is_dark {
        const DARK_TILES: &[(u32, u32)] = &[
            (0x2c2445, 0xc4b5fd), // violet
            (0x16332a, 0x6ee7b7), // emerald
            (0x172d47, 0x7dd3fc), // sky
            (0x3a2518, 0xfdba74), // amber
            (0x3d1c2e, 0xf9a8d4), // rose
            (0x18343a, 0x67e8f9), // cyan
        ];
        let (bg, fg) = DARK_TILES[hash % DARK_TILES.len()];
        (rgb(bg), rgb(fg))
    } else {
        const LIGHT_TILES: &[(u32, u32)] = &[
            (0xede9fe, 0x6d28d9),
            (0xd1fae5, 0x047857),
            (0xe0f2fe, 0x0369a1),
            (0xffedd5, 0xc2410c),
            (0xfce7f3, 0xbe185d),
            (0xcffafe, 0x0e7490),
        ];
        let (bg, fg) = LIGHT_TILES[hash % LIGHT_TILES.len()];
        (rgb(bg), rgb(fg))
    }
}

/// Distinct agent brand colors (Codex emerald, Antigravity dusk violet, OpenCode warm amber, Custom sky).
pub fn agent_brand_colors(agent_id: &str, is_dark: bool) -> (Rgba, Rgba) {
    let lower = agent_id.to_ascii_lowercase();
    if is_dark {
        if lower.contains("codex") {
            (rgba(0x10b98124), rgb(0x34d399))
        } else if lower.contains("agy") || lower.contains("antigravity") {
            (rgba(0x8e74c82c), rgb(0xc4b5fd))
        } else if lower.contains("opencode") {
            (rgba(0xf59e0b24), rgb(0xfbbf24))
        } else {
            (rgba(0x38bdf824), rgb(0x7dd3fc))
        }
    } else if lower.contains("codex") {
        (rgba(0x05966918), rgb(0x059669))
    } else if lower.contains("agy") || lower.contains("antigravity") {
        (rgba(0x6b4ce618), rgb(0x6b4ce6))
    } else if lower.contains("opencode") {
        (rgba(0xd9770618), rgb(0xc2410c))
    } else {
        (rgba(0x0284c718), rgb(0x0369a1))
    }
}
