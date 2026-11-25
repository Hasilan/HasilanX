use super::sidebar::agent_mark_icon;
use super::{ActiveModal, HasilanxApp, SettingsTab};
use crate::client::LOCAL_HOST_ID;
use crate::theme::{BackdropStyle, SidebarDensity, ThemePreset, UiPalette, agent_brand_colors};
use crate::time_fmt::{agent_display_name, shorten_path};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::GlobalState;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{Icon, Sizable, StyledExt, Theme};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use hasilanx_protocol::{AgentDescriptor, ExecutionMode, WorkspaceMode};

impl SettingsTab {
    fn all() -> &'static [Self] {
        &[
            Self::Appearance,
            Self::InputSession,
            Self::Agents,
            Self::Workflow,
            Self::Daemons,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::InputSession => "Input & Session",
            Self::Agents => "Agents",
            Self::Workflow => "Workflow",
            Self::Daemons => "Daemons",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Appearance => "Theme, new session atmosphere, sidebar density, and terminal font.",
            Self::InputSession => "IME-safe send key, new session defaults, and keyboard shortcuts.",
            Self::Agents => "Discovered AI coding agents, live model catalogs, and default agent.",
            Self::Workflow => "Repository-coupled .hasilanx.toml hooks, quality gates, and roles.",
            Self::Daemons => "Local daemon health and remote Tailscale / LAN workstations.",
        }
    }

    fn icon(self) -> Lucide {
        match self {
            Self::Appearance => Lucide::Palette,
            Self::InputSession => Lucide::SlidersHorizontal,
            Self::Agents => Lucide::Sparkles,
            Self::Workflow => Lucide::ShieldCheck,
            Self::Daemons => Lucide::Cpu,
        }
    }
}

fn backdrop_swatch_colors(style: BackdropStyle, p: &UiPalette) -> (Rgba, Rgba) {
    match (style, p.is_dark) {
        (BackdropStyle::Dusk, true) => (rgb(0x26397a), rgb(0x522a5c)),
        (BackdropStyle::Dusk, false) => (rgb(0x9fb3f4), rgb(0xefd1e7)),
        (BackdropStyle::Aurora, true) => (rgb(0x114552), rgb(0x194d42)),
        (BackdropStyle::Aurora, false) => (rgb(0x8fd6e8), rgb(0xc1f0df)),
        (BackdropStyle::Minimal, true) => (rgb(0x25213d), p.canvas),
        (BackdropStyle::Minimal, false) => (rgb(0xede9fc), p.canvas),
        (BackdropStyle::None, _) => (p.canvas, p.canvas),
    }
}

impl HasilanxApp {
    pub(crate) fn render_settings_modal(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();
        let active_tab = self.settings_tab;

        let active_host_id = self
            .selected
            .as_ref()
            .map(|k| k.host_id.clone())
            .unwrap_or_else(|| LOCAL_HOST_ID.to_string());
        let available_agents_count = self
            .host_states
            .get(&active_host_id)
            .or_else(|| self.host_states.get(LOCAL_HOST_ID))
            .map(|h| h.agents.iter().filter(|a| a.available).count())
            .unwrap_or(0);
        let connected_hosts_count = self
            .host_states
            .values()
            .filter(|h| h.connected)
            .count();
        let has_workflow = self.active_project_workflow().is_some();

        let tab_body = match active_tab {
            SettingsTab::Appearance => self.render_settings_appearance(cx),
            SettingsTab::InputSession => self.render_settings_input_session(cx),
            SettingsTab::Agents => self.render_settings_agents(&active_host_id, cx),
            SettingsTab::Workflow => self.render_settings_workflow(cx),
            SettingsTab::Daemons => self.render_settings_daemons(cx),
        };

        div()
            .id("modal-overlay-settings")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            // Sibling backdrop scrim (NOT an ancestor of the card, so clicks inside the card NEVER bubble here!)
            .child(
                div()
                    .id("modal-backdrop-settings")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(p.overlay_backdrop)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        GlobalState::suppress_text_selection(cx);
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.active_modal = ActiveModal::None;
                        this.settings_notice = None;
                        cx.notify();
                    })),
            )
            // Modal card occludes hit-testing above the sibling backdrop
            .child(
                div()
                    .id("modal-card-settings")
                    .relative()
                    .occlude()
                    .w(px(840.0))
                    .h(px(650.0))
                    .max_w_full()
                    .max_h_full()
                    .rounded_2xl()
                    .border_1()
                    .border_color(p.hairline_strong)
                    .bg(p.canvas)
                    .shadow_2xl()
                    .overflow_hidden()
                    .flex()
                    // Left navigation rail
                    .child(
                        div()
                            .w(px(222.0))
                            .h_full()
                            .flex_shrink_0()
                            .bg(p.sidebar)
                            .border_r_1()
                            .border_color(p.hairline)
                            .flex()
                            .flex_col()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        // Rail title row
                                        div()
                                            .h(px(54.0))
                                            .px_4()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        Icon::new(Lucide::Settings)
                                                            .size(px(14.0))
                                                            .text_color(p.accent),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_semibold()
                                                            .text_color(p.text)
                                                            .child("Settings"),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .px_1p5()
                                                    .py(px(2.0))
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(p.hairline)
                                                    .bg(p.surface)
                                                    .font_family(mono_family.clone())
                                                    .text_xs()
                                                    .text_color(p.text_faint)
                                                    .child("Ctrl+,"),
                                            ),
                                    )
                                    // Navigation items
                                    .child(
                                        div()
                                            .px_2p5()
                                            .py_1()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .children(SettingsTab::all().iter().copied().map(
                                                |tab| {
                                                    let selected = tab == active_tab;
                                                    let badge: Option<String> = match tab {
                                                        SettingsTab::Agents
                                                            if available_agents_count > 0 =>
                                                        {
                                                            Some(available_agents_count.to_string())
                                                        }
                                                        SettingsTab::Daemons => {
                                                            Some(connected_hosts_count.to_string())
                                                        }
                                                        _ => None,
                                                    };
                                                    let show_wf_dot = tab == SettingsTab::Workflow
                                                        && has_workflow;
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "settings-nav-{}",
                                                            tab.label()
                                                        )))
                                                        .h(px(34.0))
                                                        .px_2p5()
                                                        .rounded_lg()
                                                        .cursor_pointer()
                                                        .bg(if selected {
                                                            p.surface_selected
                                                        } else {
                                                            rgba(0x00000000)
                                                        })
                                                        .hover(|s| {
                                                            if selected {
                                                                s.bg(p.surface_selected)
                                                            } else {
                                                                s.bg(p.surface_hover)
                                                            }
                                                        })
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            |_, _, cx| {
                                                                GlobalState::suppress_text_selection(
                                                                    cx,
                                                                );
                                                            },
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.settings_tab = tab;
                                                                this.settings_notice = None;
                                                                cx.notify();
                                                            },
                                                        ))
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2p5()
                                                                .min_w_0()
                                                                .child(
                                                                    Icon::new(tab.icon())
                                                                        .size(px(14.0))
                                                                        .text_color(if selected {
                                                                            p.accent
                                                                        } else {
                                                                            p.text_muted
                                                                        }),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .when(selected, |d| {
                                                                            d.font_semibold()
                                                                        })
                                                                        .when(!selected, |d| {
                                                                            d.font_medium()
                                                                        })
                                                                        .text_color(if selected {
                                                                            p.text
                                                                        } else {
                                                                            p.text_muted
                                                                        })
                                                                        .truncate()
                                                                        .child(tab.label()),
                                                                ),
                                                        )
                                                        .when_some(badge, |el, txt| {
                                                            el.child(
                                                                div()
                                                                    .px_1p5()
                                                                    .py(px(1.0))
                                                                    .rounded_full()
                                                                    .bg(if selected {
                                                                        p.accent_soft
                                                                    } else {
                                                                        p.surface
                                                                    })
                                                                    .text_xs()
                                                                    .font_medium()
                                                                    .text_color(if selected {
                                                                        p.accent
                                                                    } else {
                                                                        p.text_faint
                                                                    })
                                                                    .child(txt),
                                                            )
                                                        })
                                                        .when(show_wf_dot, |el| {
                                                            el.child(
                                                                div()
                                                                    .w(px(6.0))
                                                                    .h(px(6.0))
                                                                    .rounded_full()
                                                                    .bg(p.success),
                                                            )
                                                        })
                                                },
                                            )),
                                    ),
                            )
                            // Bottom auto-save footer
                            .child(
                                div()
                                    .px_3p5()
                                    .py_3()
                                    .border_t_1()
                                    .border_color(p.hairline)
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .child(
                                                div()
                                                    .w(px(6.0))
                                                    .h(px(6.0))
                                                    .rounded_full()
                                                    .bg(p.success),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_muted)
                                                    .child("Saved automatically"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .font_family(mono_family.clone())
                                            .text_size(px(11.0))
                                            .text_color(p.text_faint)
                                            .truncate()
                                            .child("~/.config/hasilanx/ui.toml"),
                                    ),
                            ),
                    )
                    // Right content pane
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .bg(p.canvas)
                            .flex()
                            .flex_col()
                            // Sticky header
                            .child(
                                div()
                                    .h(px(54.0))
                                    .px_6()
                                    .flex_shrink_0()
                                    .border_b_1()
                                    .border_color(p.hairline)
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .text_color(p.text)
                                                    .child(active_tab.label()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_muted)
                                                    .truncate()
                                                    .child(active_tab.subtitle()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .flex_shrink_0()
                                            .child(
                                                div()
                                                    .px_1p5()
                                                    .py(px(2.0))
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(p.hairline)
                                                    .bg(p.surface)
                                                    .font_family(mono_family)
                                                    .text_xs()
                                                    .text_color(p.text_faint)
                                                    .child("Esc"),
                                            )
                                            .child(
                                                Button::new("close-settings-modal")
                                                    .xsmall()
                                                    .ghost()
                                                    .icon(Icon::new(Lucide::X))
                                                    .tooltip("Close Settings (Esc)")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.active_modal = ActiveModal::None;
                                                        this.settings_notice = None;
                                                        cx.notify();
                                                    })),
                                            ),
                                    ),
                            )
                            // Inline notice banner (never hidden behind backdrop!)
                            .when_some(self.settings_notice.clone(), |el, (is_err, msg)| {
                                el.child(
                                    div()
                                        .mx_6()
                                        .mt_3()
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(if is_err {
                                            p.danger_soft
                                        } else {
                                            p.success_soft
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .min_w_0()
                                                .text_xs()
                                                .text_color(if is_err {
                                                    p.danger
                                                } else {
                                                    p.success
                                                })
                                                .child(
                                                    Icon::new(if is_err {
                                                        Lucide::CircleAlert
                                                    } else {
                                                        Lucide::CircleCheck
                                                    })
                                                    .size(px(13.0)),
                                                )
                                                .child(div().truncate().child(msg)),
                                        )
                                        .child(
                                            Button::new("dismiss-settings-notice")
                                                .xsmall()
                                                .ghost()
                                                .icon(Icon::new(Lucide::X))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.settings_notice = None;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            })
                            // Scrollable category body
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scrollbar()
                                    .child(
                                        div()
                                            .px_6()
                                            .py_5()
                                            .flex()
                                            .flex_col()
                                            .gap_5()
                                            .child(tab_body),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_settings_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();
        let presets = ThemePreset::all();
        let font_sz = self.terminal_font_size;

        div()
            .flex()
            .flex_col()
            .gap_5()
            // 1. Visual Theme Cards (balanced 3x2 grid with live mini-window previews!)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .child("Theme"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child("Ctrl+Shift+T to cycle"),
                            ),
                    )
                    .children(presets.chunks(3).enumerate().map(|(row_idx, chunk)| {
                        div()
                            .id(SharedString::from(format!("settings-theme-row-{row_idx}")))
                            .flex()
                            .items_stretch()
                            .gap_2p5()
                            .children(chunk.iter().copied().map(|preset| {
                                let active = preset == self.theme_preset;
                                let tp = preset.palette();
                                div()
                                    .id(SharedString::from(format!(
                                        "set-theme-{}",
                                        preset.label()
                                    )))
                                    .flex_1()
                                    .min_w_0()
                                    .p_2()
                                    .rounded_xl()
                                    .border_1()
                                    .border_color(if active { p.accent } else { p.hairline })
                                    .bg(if active {
                                        p.surface_selected
                                    } else {
                                        p.surface
                                    })
                                    .hover(|s| {
                                        if active {
                                            s.bg(p.surface_selected)
                                        } else {
                                            s.bg(p.surface_hover).border_color(p.hairline_strong)
                                        }
                                    })
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        GlobalState::suppress_text_selection(cx);
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_theme(preset, cx);
                                    }))
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    // Live mini-app thumbnail showing real sidebar + canvas + bubble + accent!
                                    .child(
                                        div()
                                            .h(px(54.0))
                                            .w_full()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(tp.hairline_strong)
                                            .bg(tp.window_bg)
                                            .p(px(2.0))
                                            .overflow_hidden()
                                            .flex()
                                            .gap(px(2.0))
                                            // Mini sidebar
                                            .child(
                                                div()
                                                    .w(px(34.0))
                                                    .h_full()
                                                    .rounded_sm()
                                                    .bg(tp.sidebar)
                                                    .border_1()
                                                    .border_color(tp.hairline)
                                                    .p_1()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .h(px(6.0))
                                                            .w_full()
                                                            .rounded_sm()
                                                            .bg(tp.surface_selected),
                                                    )
                                                    .child(
                                                        div()
                                                            .h(px(4.0))
                                                            .w(px(20.0))
                                                            .rounded_full()
                                                            .bg(tp.hairline_strong),
                                                    )
                                                    .child(
                                                        div()
                                                            .h(px(4.0))
                                                            .w(px(16.0))
                                                            .rounded_full()
                                                            .bg(tp.hairline),
                                                    ),
                                            )
                                            // Mini main stage
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .h_full()
                                                    .rounded_sm()
                                                    .bg(tp.canvas)
                                                    .border_1()
                                                    .border_color(tp.hairline)
                                                    .p_1()
                                                    .flex()
                                                    .flex_col()
                                                    .justify_between()
                                                    // Mini user pill aligned right
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .flex()
                                                            .justify_end()
                                                            .child(
                                                                div()
                                                                    .w(px(44.0))
                                                                    .h(px(8.0))
                                                                    .rounded_sm()
                                                                    .bg(tp.user_pill),
                                                            ),
                                                    )
                                                    // Mini assistant bar
                                                    .child(
                                                        div()
                                                            .w(px(58.0))
                                                            .h(px(4.0))
                                                            .rounded_full()
                                                            .bg(tp.hairline_strong),
                                                    )
                                                    // Mini composer with accent send dot
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .h(px(13.0))
                                                            .px_1()
                                                            .rounded_sm()
                                                            .border_1()
                                                            .border_color(tp.hairline)
                                                            .bg(tp.surface_elevated)
                                                            .flex()
                                                            .items_center()
                                                            .justify_end()
                                                            .child(
                                                                div()
                                                                    .w(px(7.0))
                                                                    .h(px(7.0))
                                                                    .rounded_full()
                                                                    .bg(tp.accent),
                                                            ),
                                                    ),
                                            ),
                                    )
                                    // Card label + Light/Dark micro tag + Check
                                    .child(
                                        div()
                                            .px_0p5()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1p5()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .when(active, |d| d.font_semibold())
                                                            .when(!active, |d| d.font_medium())
                                                            .text_color(if active {
                                                                p.text
                                                            } else {
                                                                p.text_muted
                                                            })
                                                            .truncate()
                                                            .child(preset.label()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(p.text_faint)
                                                            .child(if preset.is_dark() {
                                                                "Dark"
                                                            } else {
                                                                "Light"
                                                            }),
                                                    ),
                                            )
                                            .when(active, |el| {
                                                el.child(
                                                    Icon::new(Lucide::Check)
                                                        .size(px(12.0))
                                                        .text_color(p.accent),
                                                )
                                            }),
                                    )
                            }))
                    })),
            )
            // 2. New session atmosphere (4 equal visual sky cards)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2p5()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(p.text)
                            .child("New session atmosphere"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_stretch()
                            .gap_2()
                            .children(BackdropStyle::all().iter().copied().map(|bs| {
                                let active = bs == self.backdrop_style;
                                let (top_c, mid_c) = backdrop_swatch_colors(bs, &p);
                                div()
                                    .id(SharedString::from(format!("set-backdrop-{}", bs.label())))
                                    .flex_1()
                                    .min_w_0()
                                    .p_1p5()
                                    .rounded_xl()
                                    .border_1()
                                    .border_color(if active { p.accent } else { p.hairline })
                                    .bg(if active {
                                        p.surface_selected
                                    } else {
                                        p.surface
                                    })
                                    .hover(|s| {
                                        if active {
                                            s.bg(p.surface_selected)
                                        } else {
                                            s.bg(p.surface_hover).border_color(p.hairline_strong)
                                        }
                                    })
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        GlobalState::suppress_text_selection(cx);
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_backdrop_style(bs, cx);
                                    }))
                                    .flex()
                                    .flex_col()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .h(px(26.0))
                                            .w_full()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(p.hairline)
                                            .overflow_hidden()
                                            .flex()
                                            .flex_col()
                                            .child(div().flex_1().w_full().bg(top_c))
                                            .child(div().flex_1().w_full().bg(mid_c)),
                                    )
                                    .child(
                                        div()
                                            .px_0p5()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .when(active, |d| d.font_medium())
                                                    .text_color(if active {
                                                        p.text
                                                    } else {
                                                        p.text_muted
                                                    })
                                                    .truncate()
                                                    .child(bs.label()),
                                            )
                                            .when(active, |el| {
                                                el.child(
                                                    Icon::new(Lucide::Check)
                                                        .size(px(11.0))
                                                        .text_color(p.accent),
                                                )
                                            }),
                                    )
                            })),
                    ),
            )
            // 3. Interface & Terminal grouped settings card
            .child(
                div()
                    .rounded_xl()
                    .border_1()
                    .border_color(p.hairline)
                    .bg(p.surface)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    // Row 1: Sidebar density
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(p.hairline)
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(p.text)
                                            .child("Sidebar density"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.text_muted)
                                            .child("Regular shows agent & live diff pills; Compact fits more sessions."),
                                    ),
                            )
                            .child(
                                div()
                                    .p_0p5()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(p.hairline)
                                    .bg(p.canvas)
                                    .flex()
                                    .items_center()
                                    .gap_0p5()
                                    .flex_shrink_0()
                                    .children(
                                        [SidebarDensity::Regular, SidebarDensity::Compact]
                                            .into_iter()
                                            .map(|d| {
                                                let active = d == self.sidebar_density;
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "set-density-{}",
                                                        d.label()
                                                    )))
                                                    .px_2p5()
                                                    .py_1()
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .bg(if active {
                                                        p.surface_selected
                                                    } else {
                                                        rgba(0x00000000)
                                                    })
                                                    .hover(|s| {
                                                        if active {
                                                            s.bg(p.surface_selected)
                                                        } else {
                                                            s.bg(p.surface_hover)
                                                        }
                                                    })
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        |_, _, cx| {
                                                            GlobalState::suppress_text_selection(
                                                                cx,
                                                            );
                                                        },
                                                    )
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.set_sidebar_density(d, cx);
                                                        },
                                                    ))
                                                    .text_xs()
                                                    .when(active, |el| el.font_medium())
                                                    .text_color(if active {
                                                        p.text
                                                    } else {
                                                        p.text_muted
                                                    })
                                                    .child(d.label())
                                            }),
                                    ),
                            ),
                    )
                    // Row 2: Terminal font size with live preview
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(p.text)
                                            .child("Terminal font size"),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(p.hairline)
                                            .bg(p.terminal_bg)
                                            .font_family(mono_family.clone())
                                            .text_size(px(font_sz))
                                            .text_color(p.text)
                                            .child("$ cargo test --workspace"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .flex_shrink_0()
                                    .child(
                                        Button::new("set-term-font-dec")
                                            .xsmall()
                                            .ghost()
                                            .icon(Icon::new(Lucide::Minus))
                                            .tooltip("Decrease terminal font size")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_terminal_font_size(
                                                    this.terminal_font_size - 1.0,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        div()
                                            .w(px(46.0))
                                            .text_center()
                                            .font_family(mono_family)
                                            .text_xs()
                                            .font_medium()
                                            .text_color(p.text)
                                            .child(format!("{}px", font_sz.round() as i32)),
                                    )
                                    .child(
                                        Button::new("set-term-font-inc")
                                            .xsmall()
                                            .ghost()
                                            .icon(Icon::new(Lucide::Plus))
                                            .tooltip("Increase terminal font size")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_terminal_font_size(
                                                    this.terminal_font_size + 1.0,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .when((font_sz - 13.0).abs() > 0.1, |el| {
                                        el.child(
                                            Button::new("set-term-font-reset")
                                                .xsmall()
                                                .ghost()
                                                .label("Reset")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.set_terminal_font_size(13.0, cx);
                                                })),
                                        )
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_settings_input_session(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();

        let shortcuts: &[(&str, &str)] = &[
            ("Command Palette", "Ctrl+K"),
            ("New Session", "Ctrl+N"),
            ("Split Session Arena", "Ctrl+\\"),
            ("Find in Session", "Ctrl+F"),
            ("Changes (Git Diff)", "Ctrl+Shift+D"),
            ("Files Explorer", "Ctrl+Shift+F"),
            ("GPU Terminal Dock", "Ctrl+Shift+J"),
            ("Toggle All Tools", "Ctrl+Shift+O"),
            ("Cycle Theme", "Ctrl+Shift+T"),
            ("Toggle Sidebar", "Ctrl+B"),
            ("Quick Jump Session", "Ctrl+1..9"),
            ("Back / Forward", "Alt+[ / ]"),
        ];

        div()
            .flex()
            .flex_col()
            .gap_5()
            // 1. Send shortcut (IME) — 2 clean side-by-side cards
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(p.text)
                            .child("Send shortcut (IME)"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_stretch()
                            .gap_2p5()
                            .child(self.render_send_key_card(
                                "send-key-ctrl-enter",
                                "Ctrl+Enter",
                                Some("IME safe"),
                                "Ctrl+Enter to send",
                                "Enter inserts a newline and never interrupts Japanese / CJK kanji conversion.",
                                !self.send_on_enter,
                                false,
                                cx,
                            ))
                            .child(self.render_send_key_card(
                                "send-key-enter",
                                "Enter",
                                None,
                                "Enter to send",
                                "Shift+Enter inserts a newline. Fastest for Latin-only input.",
                                self.send_on_enter,
                                true,
                                cx,
                            )),
                    ),
            )
            // 2. New Session Defaults (grouped card)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .child("New session defaults"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child("Overridden by .hasilanx.toml when present"),
                            ),
                    )
                    .child(
                        div()
                            .rounded_xl()
                            .border_1()
                            .border_color(p.hairline)
                            .bg(p.surface)
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            // Workspace mode row
                            .child(
                                div()
                                    .px_4()
                                    .py_3()
                                    .border_b_1()
                                    .border_color(p.hairline)
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_4()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_0p5()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_medium()
                                                    .text_color(p.text)
                                                    .child("Workspace checkout"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_muted)
                                                    .child("Run directly in repo or spawn an isolated Git worktree."),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .p_0p5()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(p.hairline)
                                            .bg(p.canvas)
                                            .flex()
                                            .items_center()
                                            .gap_0p5()
                                            .flex_shrink_0()
                                            .children(
                                                [
                                                    (WorkspaceMode::Local, "Local", Lucide::FolderOpen),
                                                    (
                                                        WorkspaceMode::Worktree,
                                                        "Worktree",
                                                        Lucide::GitBranch,
                                                    ),
                                                ]
                                                .into_iter()
                                                .map(|(mode, lbl, ic)| {
                                                    let active =
                                                        self.new_thread_workspace_mode == mode;
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "set-ws-mode-{lbl}"
                                                        )))
                                                        .px_2p5()
                                                        .py_1()
                                                        .rounded_md()
                                                        .cursor_pointer()
                                                        .bg(if active {
                                                            p.surface_selected
                                                        } else {
                                                            rgba(0x00000000)
                                                        })
                                                        .hover(|s| {
                                                            if active {
                                                                s.bg(p.surface_selected)
                                                            } else {
                                                                s.bg(p.surface_hover)
                                                            }
                                                        })
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            |_, _, cx| {
                                                                GlobalState::suppress_text_selection(
                                                                    cx,
                                                                );
                                                            },
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.set_default_workspace_mode(
                                                                    mode, cx,
                                                                );
                                                            },
                                                        ))
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .text_xs()
                                                        .when(active, |el| el.font_medium())
                                                        .text_color(if active {
                                                            p.text
                                                        } else {
                                                            p.text_muted
                                                        })
                                                        .child(Icon::new(ic).size(px(11.0)))
                                                        .child(lbl)
                                                }),
                                            ),
                                    ),
                            )
                            // Execution mode row
                            .child(
                                div()
                                    .px_4()
                                    .py_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_4()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_0p5()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_medium()
                                                    .text_color(p.text)
                                                    .child("Permission mode"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_muted)
                                                    .child("Tool approval posture for newly created sessions."),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .p_0p5()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(p.hairline)
                                            .bg(p.canvas)
                                            .flex()
                                            .items_center()
                                            .gap_0p5()
                                            .flex_shrink_0()
                                            .children(
                                                [
                                                    ExecutionMode::Ask,
                                                    ExecutionMode::AcceptEdits,
                                                    ExecutionMode::Auto,
                                                    ExecutionMode::Plan,
                                                ]
                                                .into_iter()
                                                .map(|mode| {
                                                    let active = self.draft_mode == mode;
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "set-exec-mode-{}",
                                                            mode.label()
                                                        )))
                                                        .px_2()
                                                        .py_1()
                                                        .rounded_md()
                                                        .cursor_pointer()
                                                        .bg(if active {
                                                            p.surface_selected
                                                        } else {
                                                            rgba(0x00000000)
                                                        })
                                                        .hover(|s| {
                                                            if active {
                                                                s.bg(p.surface_selected)
                                                            } else {
                                                                s.bg(p.surface_hover)
                                                            }
                                                        })
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            |_, _, cx| {
                                                                GlobalState::suppress_text_selection(
                                                                    cx,
                                                                );
                                                            },
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.set_default_execution_mode(
                                                                    mode, cx,
                                                                );
                                                            },
                                                        ))
                                                        .text_xs()
                                                        .when(active, |el| el.font_medium())
                                                        .text_color(if active {
                                                            p.text
                                                        } else {
                                                            p.text_muted
                                                        })
                                                        .child(mode.label())
                                                }),
                                            ),
                                    ),
                            ),
                    ),
            )
            // 3. Keyboard Shortcuts Cheat Sheet (2-column scannable card)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(p.text)
                            .child("Keyboard shortcuts"),
                    )
                    .child(
                        div()
                            .p_3()
                            .rounded_xl()
                            .border_1()
                            .border_color(p.hairline)
                            .bg(p.surface)
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .children(shortcuts.chunks(2).map(|pair| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_4()
                                    .children(pair.iter().map(|&(label, keys)| {
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .py_0p5()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_muted)
                                                    .truncate()
                                                    .child(label),
                                            )
                                            .child(
                                                div()
                                                    .px_1p5()
                                                    .py(px(1.0))
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(p.hairline)
                                                    .bg(p.canvas)
                                                    .font_family(mono_family.clone())
                                                    .text_xs()
                                                    .text_color(p.text)
                                                    .flex_shrink_0()
                                                    .child(keys),
                                            )
                                    }))
                            })),
                    ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_send_key_card(
        &self,
        id: &'static str,
        keycap: &'static str,
        badge: Option<&'static str>,
        title: &'static str,
        desc: &'static str,
        selected: bool,
        target_send_on_enter: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();

        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(if selected { p.accent } else { p.hairline })
            .bg(if selected {
                p.surface_selected
            } else {
                p.surface
            })
            .hover(|s| {
                if selected {
                    s.bg(p.surface_selected)
                } else {
                    s.bg(p.surface_hover).border_color(p.hairline_strong)
                }
            })
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                GlobalState::suppress_text_selection(cx);
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_send_on_enter(target_send_on_enter, window, cx);
            }))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.0))
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(p.hairline_strong)
                                    .bg(p.canvas)
                                    .font_family(mono_family)
                                    .text_xs()
                                    .font_medium()
                                    .text_color(p.text)
                                    .child(keycap),
                            )
                            .when_some(badge, |el, b| {
                                el.child(
                                    div()
                                        .px_1p5()
                                        .py(px(1.0))
                                        .rounded_full()
                                        .bg(p.accent_soft)
                                        .text_xs()
                                        .font_medium()
                                        .text_color(p.accent)
                                        .child(b),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded_full()
                            .border_1()
                            .border_color(if selected { p.accent } else { p.hairline_strong })
                            .bg(if selected { p.accent } else { rgba(0x00000000) })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |el| {
                                el.child(
                                    Icon::new(Lucide::Check)
                                        .size(px(10.0))
                                        .text_color(p.accent_fg),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(p.text)
                            .child(title),
                    )
                    .child(div().text_xs().text_color(p.text_muted).child(desc)),
            )
            .into_any_element()
    }

    fn render_settings_agents(&self, active_host_id: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();
        let hs = self
            .host_states
            .get(active_host_id)
            .or_else(|| self.host_states.get(LOCAL_HOST_ID));
        let host_label = hs
            .map(|h| h.label.clone())
            .unwrap_or_else(|| "Local".to_string());
        let host_id_ref = hs
            .map(|h| h.host_id.clone())
            .unwrap_or_else(|| LOCAL_HOST_ID.to_string());
        let agents: Vec<AgentDescriptor> = hs.map(|h| h.agents.clone()).unwrap_or_default();
        let default_agent = self
            .preferred_agent_id
            .as_deref()
            .unwrap_or(self.draft_agent_id.as_str());

        div()
            .flex()
            .flex_col()
            .gap_3p5()
            // Top action strip
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.text_muted)
                            .child(format!(
                                "Adapters discovered on {} ({} available)",
                                host_label,
                                agents.iter().filter(|a| a.available).count()
                            )),
                    )
                    .child(
                        Button::new("settings-refresh-models")
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(Lucide::RefreshCw))
                            .label("Refresh models")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.refresh_agent_models(&host_id_ref, cx);
                            })),
                    ),
            )
            // Agent list cards
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2p5()
                    .when(agents.is_empty(), |el| {
                        el.child(
                            div()
                                .p_4()
                                .rounded_xl()
                                .border_1()
                                .border_color(p.hairline)
                                .bg(p.surface)
                                .text_xs()
                                .text_color(p.text_faint)
                                .child("Waiting for daemon agent discovery..."),
                        )
                    })
                    .children(agents.into_iter().map(|agent| {
                        let is_default = agent.id == default_agent;
                        let (brand_bg, brand_fg) = agent_brand_colors(&agent.id, p.is_dark);
                        let brand_icon = agent_mark_icon(&agent.id);
                        let bin_name = std::path::Path::new(&agent.command)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(agent.command.as_str());
                        let cmd_preview = if agent.args.is_empty() {
                            bin_name.to_string()
                        } else {
                            format!("{bin_name} {}", agent.args.join(" "))
                        };
                        let def_model_label = agent.default_model.as_deref().map(|did| {
                            agent
                                .models
                                .iter()
                                .find(|m| m.id == did)
                                .map(|m| m.name.clone())
                                .unwrap_or_else(|| did.to_string())
                        });
                        let models_summary = if !agent.available {
                            "CLI binary not found on PATH".to_string()
                        } else if agent.models.is_empty() {
                            "Models load dynamically on refresh".to_string()
                        } else if let Some(def_m) = def_model_label {
                            format!("{} models · Default: {def_m}", agent.models.len())
                        } else {
                            format!("{} live models available", agent.models.len())
                        };
                        let aid = agent.id.clone();

                        div()
                            .px_3p5()
                            .py_3()
                            .rounded_xl()
                            .border_1()
                            .border_color(if is_default { p.accent } else { p.hairline })
                            .bg(if is_default {
                                p.surface_selected
                            } else {
                                p.surface
                            })
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .w(px(34.0))
                                            .h(px(34.0))
                                            .rounded_lg()
                                            .bg(brand_bg)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .flex_shrink_0()
                                            .child(
                                                Icon::new(brand_icon)
                                                    .size(px(16.0))
                                                    .text_color(brand_fg),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_0p5()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_semibold()
                                                            .text_color(p.text)
                                                            .child(agent_display_name(&agent.id)),
                                                    )
                                                    .child(
                                                        div()
                                                            .px_1p5()
                                                            .py(px(1.0))
                                                            .rounded_full()
                                                            .bg(if agent.available {
                                                                p.success_soft
                                                            } else {
                                                                p.canvas
                                                            })
                                                            .flex()
                                                            .items_center()
                                                            .gap_1()
                                                            .child(
                                                                div()
                                                                    .w(px(5.0))
                                                                    .h(px(5.0))
                                                                    .rounded_full()
                                                                    .bg(if agent.available {
                                                                        p.success
                                                                    } else {
                                                                        p.text_faint
                                                                    }),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(if agent.available {
                                                                        p.success
                                                                    } else {
                                                                        p.text_faint
                                                                    })
                                                                    .child(if agent.available {
                                                                        "Available"
                                                                    } else {
                                                                        "Not installed"
                                                                    }),
                                                            ),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .px_1p5()
                                                            .py(px(1.0))
                                                            .rounded_sm()
                                                            .bg(p.canvas)
                                                            .border_1()
                                                            .border_color(p.hairline)
                                                            .font_family(mono_family.clone())
                                                            .text_size(px(11.0))
                                                            .text_color(p.text_faint)
                                                            .flex_shrink_0()
                                                            .child(cmd_preview),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(p.text_muted)
                                                            .truncate()
                                                            .child(models_summary),
                                                    ),
                                            ),
                                    ),
                            )
                            .child(
                                div().flex_shrink_0().child(if is_default {
                                    div()
                                        .px_2p5()
                                        .py_1()
                                        .rounded_full()
                                        .bg(p.accent_soft)
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .text_xs()
                                        .font_medium()
                                        .text_color(p.accent)
                                        .child(Icon::new(Lucide::Check).size(px(11.0)))
                                        .child("Default")
                                        .into_any_element()
                                } else if agent.available {
                                    Button::new(SharedString::from(format!(
                                        "set-def-agent-{}",
                                        agent.id
                                    )))
                                    .xsmall()
                                    .ghost()
                                    .label("Set default")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_default_agent_pref(aid.clone(), cx);
                                    }))
                                    .into_any_element()
                                } else {
                                    div().into_any_element()
                                }),
                            )
                    })),
            )
            .into_any_element()
    }

    fn render_settings_workflow(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();

        let project_info: Option<(String, String)> = self
            .target_project_for_new_thread()
            .and_then(|(hid, pid, pname)| {
                let root = self
                    .host_states
                    .get(&hid)
                    .and_then(|h| h.projects.iter().find(|pr| pr.id == pid))
                    .map(|pr| pr.root_path.clone())?;
                Some((pname, root))
            });

        let wf = self.active_project_workflow().cloned();

        let Some((proj_name, proj_root)) = project_info else {
            return div()
                .p_5()
                .rounded_xl()
                .border_1()
                .border_color(p.hairline)
                .bg(p.surface)
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(p.text)
                        .child("No active repository"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.text_muted)
                        .child("Add a project folder from the sidebar to configure .hasilanx.toml workflows."),
                )
                .into_any_element();
        };

        let Some(cfg) = wf else {
            return div()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .px_3p5()
                        .py_2p5()
                        .rounded_xl()
                        .border_1()
                        .border_color(p.hairline)
                        .bg(p.surface)
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(Lucide::FolderOpen)
                                .size(px(14.0))
                                .text_color(p.accent),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(p.text)
                                .child(proj_name),
                        )
                        .child(
                            div()
                                .font_family(mono_family)
                                .text_xs()
                                .text_color(p.text_faint)
                                .truncate()
                                .child(shorten_path(&proj_root)),
                        ),
                )
                .child(
                    div()
                        .p_5()
                        .rounded_xl()
                        .border_1()
                        .border_color(p.hairline)
                        .bg(p.surface)
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(p.text)
                                .child("No .hasilanx.toml in this repository yet"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.text_muted)
                                .child("Auto-detect your toolchain (Rust, Node, Python, Go) and generate a ready-to-use workflow with Worktree hooks, post-turn Quality Gates + 1-click Auto-Fix, Implementer/Reviewer agent roles, and custom `/` slash commands."),
                        )
                        .child(
                            div().pt_1().child(
                                Button::new("settings-init-wf-btn")
                                    .xsmall()
                                    .primary()
                                    .icon(Icon::new(Lucide::Sparkles))
                                    .label("Generate .hasilanx.toml")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.init_active_project_workflow(cx);
                                        this.settings_notice = Some((
                                            false,
                                            "Generated .hasilanx.toml for this repository.".into(),
                                        ));
                                        cx.notify();
                                    })),
                            ),
                        ),
                )
                .into_any_element();
        };

        let impl_role = cfg
            .roles
            .implementer
            .as_deref()
            .map(agent_display_name)
            .unwrap_or("Default");
        let rev_role = cfg
            .roles
            .reviewer
            .as_deref()
            .map(agent_display_name)
            .unwrap_or("Auto alternate");
        let def_agent = cfg
            .defaults
            .agent
            .as_deref()
            .map(agent_display_name)
            .unwrap_or("Global");
        let def_mode = cfg
            .defaults
            .execution_mode
            .map(|m| m.label())
            .unwrap_or("Global");

        div()
            .flex()
            .flex_col()
            .gap_4()
            // Top active workflow banner + actions
            .child(
                div()
                    .px_3p5()
                    .py_2p5()
                    .rounded_xl()
                    .border_1()
                    .border_color(p.hairline)
                    .bg(p.surface)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .w(px(7.0))
                                    .h(px(7.0))
                                    .rounded_full()
                                    .bg(p.success)
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .child(proj_name),
                            )
                            .child(
                                div()
                                    .font_family(mono_family.clone())
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .truncate()
                                    .child(".hasilanx.toml"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                Button::new("settings-wf-open")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::FileCode))
                                    .label("Open file")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_modal = ActiveModal::None;
                                        this.open_file_in_inspector(".hasilanx.toml", cx);
                                    })),
                            )
                            .child(
                                Button::new("settings-wf-run")
                                    .xsmall()
                                    .primary()
                                    .icon(Icon::new(Lucide::Play))
                                    .label("Run checks")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_modal = ActiveModal::None;
                                        this.run_selected_project_checks(None, cx);
                                    })),
                            )
                            .child(
                                Button::new("settings-wf-reload")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::RefreshCw))
                                    .tooltip("Reload .hasilanx.toml from disk")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.reload_active_project_workflow(cx);
                                        this.settings_notice = Some((
                                            false,
                                            "Reloaded .hasilanx.toml from disk.".into(),
                                        ));
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            // Row 1: Roles & Defaults + Worktree Hooks (2 side-by-side cards)
            .child(
                div()
                    .flex()
                    .items_stretch()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .p_3p5()
                            .rounded_xl()
                            .border_1()
                            .border_color(p.hairline)
                            .bg(p.surface)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .child(
                                        Icon::new(Lucide::GitFork)
                                            .size(px(12.0))
                                            .text_color(p.accent),
                                    )
                                    .child("Roles & defaults"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .text_xs()
                                    .child(
                                        div()
                                            .flex()
                                            .justify_between()
                                            .child(div().text_color(p.text_muted).child("Implementer"))
                                            .child(div().font_medium().text_color(p.text).child(impl_role)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .justify_between()
                                            .child(div().text_color(p.text_muted).child("Reviewer"))
                                            .child(div().font_medium().text_color(p.text).child(rev_role)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .justify_between()
                                            .child(div().text_color(p.text_muted).child("Session defaults"))
                                            .child(
                                                div()
                                                    .text_color(p.text_muted)
                                                    .child(format!("{def_agent} · {def_mode}")),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .p_3p5()
                            .rounded_xl()
                            .border_1()
                            .border_color(p.hairline)
                            .bg(p.surface)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .child(
                                        Icon::new(Lucide::GitBranch)
                                            .size(px(12.0))
                                            .text_color(p.accent),
                                    )
                                    .child("Worktree hooks"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .when(cfg.worktree.copy_files.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .text_xs()
                                                .text_color(p.text_faint)
                                                .child("No copy_files configured"),
                                        )
                                    })
                                    .children(cfg.worktree.copy_files.iter().map(|f| {
                                        div()
                                            .px_1p5()
                                            .py(px(1.0))
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(p.hairline)
                                            .bg(p.canvas)
                                            .font_family(mono_family.clone())
                                            .text_xs()
                                            .text_color(p.text_muted)
                                            .child(f.clone())
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child(if cfg.worktree.setup.is_empty() {
                                        "No setup scripts".to_string()
                                    } else {
                                        format!(
                                            "{} setup hook(s): {}",
                                            cfg.worktree.setup.len(),
                                            cfg.worktree.setup.join(" && ")
                                        )
                                    }),
                            ),
                    ),
            )
            // Row 2: Quality Gates card
            .child(
                div()
                    .p_3p5()
                    .rounded_xl()
                    .border_1()
                    .border_color(p.hairline)
                    .bg(p.surface)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .child(
                                        Icon::new(Lucide::ShieldCheck)
                                            .size(px(13.0))
                                            .text_color(p.accent),
                                    )
                                    .child(format!(
                                        "Quality gates ({})",
                                        cfg.checks.commands.len()
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .when(cfg.checks.run_after_turn, |el| {
                                        el.child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.0))
                                                .rounded_full()
                                                .bg(p.accent_soft)
                                                .text_xs()
                                                .text_color(p.accent)
                                                .child("Auto after turn"),
                                        )
                                    })
                                    .when(cfg.checks.only_when_dirty, |el| {
                                        el.child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.0))
                                                .rounded_full()
                                                .bg(p.canvas)
                                                .border_1()
                                                .border_color(p.hairline)
                                                .text_xs()
                                                .text_color(p.text_muted)
                                                .child("Only when dirty"),
                                        )
                                    }),
                            ),
                    )
                    .children(cfg.checks.commands.iter().map(|spec| {
                        let cmd_one = spec
                            .command
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        div()
                            .px_2p5()
                            .py_1p5()
                            .rounded_lg()
                            .bg(p.canvas)
                            .border_1()
                            .border_color(p.hairline)
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .flex_shrink_0()
                                    .child(spec.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_family(mono_family.clone())
                                    .text_xs()
                                    .text_color(p.text_muted)
                                    .whitespace_nowrap()
                                    .truncate()
                                    .child(cmd_one),
                            )
                    })),
            )
            // Row 3: Custom slash commands
            .when(!cfg.commands.is_empty(), |el| {
                el.child(
                    div()
                        .p_3p5()
                        .rounded_xl()
                        .border_1()
                        .border_color(p.hairline)
                        .bg(p.surface)
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(p.text)
                                .child(format!(
                                    "Repository / commands ({})",
                                    cfg.commands.len()
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_1p5()
                                .children(cfg.commands.iter().map(|c| {
                                    let name = c.name.trim().trim_start_matches('/');
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_lg()
                                        .bg(p.canvas)
                                        .border_1()
                                        .border_color(p.hairline)
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .font_family(mono_family.clone())
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(p.accent)
                                                .child(format!("/{name}")),
                                        )
                                        .when(!c.description.trim().is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_muted)
                                                    .child(c.description.clone()),
                                            )
                                        })
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_settings_daemons(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();

        let hosts: Vec<(String, String, bool, bool, String, String, String, usize, usize)> = self
            .host_order
            .iter()
            .filter_map(|hid| {
                self.host_states.get(hid).map(|h| {
                    (
                        h.host_id.clone(),
                        h.label.clone(),
                        h.is_local,
                        h.connected,
                        h.status_message.clone(),
                        h.hostname.clone(),
                        h.daemon_version.clone(),
                        h.projects.len(),
                        h.threads.len(),
                    )
                })
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap_5()
            // 1. Connected Daemons
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(p.text)
                            .child("Connected daemons"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(hosts.into_iter().map(
                                |(
                                    hid,
                                    label,
                                    is_local,
                                    connected,
                                    status_msg,
                                    hostname,
                                    ver,
                                    proj_cnt,
                                    thread_cnt,
                                )| {
                                    let hid_rm = hid.clone();
                                    let mut meta = Vec::new();
                                    if !hostname.is_empty() {
                                        meta.push(hostname);
                                    }
                                    if !ver.is_empty() {
                                        meta.push(format!("v{ver}"));
                                    }
                                    let proj_w = if proj_cnt == 1 { "project" } else { "projects" };
                                    let sess_w = if thread_cnt == 1 { "session" } else { "sessions" };
                                    meta.push(format!(
                                        "{proj_cnt} {proj_w} · {thread_cnt} {sess_w}"
                                    ));
                                    div()
                                        .px_3p5()
                                        .py_3()
                                        .rounded_xl()
                                        .border_1()
                                        .border_color(p.hairline)
                                        .bg(p.surface)
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_3()
                                                .min_w_0()
                                                .child(
                                                    div()
                                                        .w(px(30.0))
                                                        .h(px(30.0))
                                                        .rounded_lg()
                                                        .bg(if connected {
                                                            p.success_soft
                                                        } else {
                                                            p.approval_soft
                                                        })
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .flex_shrink_0()
                                                        .child(
                                                            Icon::new(if is_local {
                                                                Lucide::Cpu
                                                            } else {
                                                                Lucide::Globe
                                                            })
                                                            .size(px(14.0))
                                                            .text_color(if connected {
                                                                p.success
                                                            } else {
                                                                p.approval
                                                            }),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_0p5()
                                                        .min_w_0()
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .font_semibold()
                                                                        .text_color(p.text)
                                                                        .child(label),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .px_1p5()
                                                                        .py(px(1.0))
                                                                        .rounded_full()
                                                                        .bg(if connected {
                                                                            p.success_soft
                                                                        } else {
                                                                            p.approval_soft
                                                                        })
                                                                        .text_xs()
                                                                        .text_color(if connected {
                                                                            p.success
                                                                        } else {
                                                                            p.approval
                                                                        })
                                                                        .child(status_msg),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .font_family(mono_family.clone())
                                                                .text_xs()
                                                                .text_color(p.text_faint)
                                                                .truncate()
                                                                .child(meta.join(" · ")),
                                                        ),
                                                ),
                                        )
                                        .when(!is_local, |el| {
                                            el.child(
                                                Button::new(SharedString::from(format!(
                                                    "rm-host-{hid}"
                                                )))
                                                .xsmall()
                                                .ghost()
                                                .icon(Icon::new(Lucide::Trash))
                                                .tooltip("Disconnect remote host")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.remove_remote_host(hid_rm.clone(), cx);
                                                })),
                                            )
                                        })
                                },
                            )),
                    ),
            )
            // 2. Connect Remote Daemon (cleanly labeled 2-row form)
            .child(
                div()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(p.hairline)
                    .bg(p.surface)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .child("Connect remote daemon (Tailscale / LAN)"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_muted)
                                    .child("Control agents & PTY terminals running on another Linux/macOS box."),
                            ),
                    )
                    // Row 1: Display name + URL
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(180.0))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(p.text_muted)
                                            .child("Host label"),
                                    )
                                    .child(Input::new(&self.remote_name_input).xsmall()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(p.text_muted)
                                            .child("Daemon URL"),
                                    )
                                    .child(Input::new(&self.remote_url_input).xsmall()),
                            ),
                    )
                    // Row 2: Auth token
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(p.text_muted)
                                    .child("Bearer token"),
                            )
                            .child(Input::new(&self.remote_token_input).xsmall()),
                    )
                    // Footer action row
                    .child(
                        div()
                            .pt_1()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child("Find your token in ~/.config/hasilanx/daemon.toml"),
                            )
                            .child(
                                Button::new("confirm-remote-modal")
                                    .xsmall()
                                    .primary()
                                    .icon(Icon::new(Lucide::Plus))
                                    .label("Connect host")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_remote_host(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
