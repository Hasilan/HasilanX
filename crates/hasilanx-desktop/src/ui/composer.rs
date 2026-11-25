use super::{
    ComposerAcceptAutocomplete, ComposerNextAutocomplete, ComposerPrevAutocomplete, HasilanxApp,
};
use super::files_view::{WorkspaceFileEntry, ext_badge};
use super::sidebar::agent_mark_icon;
use crate::client::LOCAL_HOST_ID;
use crate::theme::agent_brand_colors;
use crate::time_fmt::agent_display_name;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::GlobalState;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable, StyledExt, Theme};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use hasilanx_protocol::{
    AgentDescriptor, Attachment, ExecutionMode, ModelInfo, QueuedPrompt, RpcCommand,
    ThinkingOption, ThreadStatus, WorkspaceMode,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComposerTrigger {
    Mention { start_byte: usize, query: String },
    Slash { query: String },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SlashCommandSpec {
    pub(crate) id: &'static str,
    pub(crate) command: &'static str,
    pub(crate) description: &'static str,
    pub(crate) icon: Lucide,
}

#[derive(Debug, Clone)]
pub(crate) struct ComposerSlashOption {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) description: String,
    pub(crate) icon: Lucide,
    pub(crate) is_project: bool,
    pub(crate) project_prompt: Option<String>,
}

const SLASH_COMMANDS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        id: "check",
        command: "/check",
        description: "Run .hasilanx.toml quality gate checks",
        icon: Lucide::CircleCheck,
    },
    SlashCommandSpec {
        id: "init-workflow",
        command: "/init-workflow",
        description: "Generate .hasilanx.toml workflow for this repo",
        icon: Lucide::Sparkles,
    },
    SlashCommandSpec {
        id: "review",
        command: "/review",
        description: "Review the current git diff for bugs & edge cases",
        icon: Lucide::Sparkles,
    },
    SlashCommandSpec {
        id: "test",
        command: "/test",
        description: "Run the project test suite and summarize failures",
        icon: Lucide::Check,
    },
    SlashCommandSpec {
        id: "explain",
        command: "/explain",
        description: "Explain repository architecture and entry points",
        icon: Lucide::Info,
    },
    SlashCommandSpec {
        id: "parallel",
        command: "/parallel",
        description: "Fork an alternate agent in an isolated worktree side-by-side",
        icon: Lucide::GitFork,
    },
    SlashCommandSpec {
        id: "split",
        command: "/split",
        description: "Toggle side-by-side Split Session view (Ctrl+\\)",
        icon: Lucide::Columns2,
    },
    SlashCommandSpec {
        id: "plan",
        command: "/plan",
        description: "Switch permission mode to Plan",
        icon: Lucide::Layers,
    },
    SlashCommandSpec {
        id: "auto",
        command: "/auto",
        description: "Switch permission mode to Auto",
        icon: Lucide::Zap,
    },
    SlashCommandSpec {
        id: "ask",
        command: "/ask",
        description: "Switch permission mode to Ask before tools",
        icon: Lucide::Shield,
    },
    SlashCommandSpec {
        id: "worktree",
        command: "/worktree",
        description: "Toggle Isolated Git Worktree mode for new sessions",
        icon: Lucide::GitBranch,
    },
    SlashCommandSpec {
        id: "diff",
        command: "/diff",
        description: "Toggle Changes inspector (Ctrl+Shift+D)",
        icon: Lucide::FileDiff,
    },
    SlashCommandSpec {
        id: "files",
        command: "/files",
        description: "Toggle Files explorer (Ctrl+Shift+F)",
        icon: Lucide::FileCode,
    },
    SlashCommandSpec {
        id: "terminal",
        command: "/terminal",
        description: "Toggle integrated GPU Terminal (Ctrl+Shift+J)",
        icon: Lucide::SquareTerminal,
    },
];

pub(crate) fn detect_composer_trigger(text: &str) -> Option<ComposerTrigger> {
    if text.is_empty() {
        return None;
    }
    // Single-line slash command at the beginning of input
    if !text.contains('\n') {
        let trimmed_start = text.trim_start();
        if let Some(rest) = trimmed_start.strip_prefix('/') {
            if !rest.chars().any(char::is_whitespace) {
                return Some(ComposerTrigger::Slash {
                    query: rest.to_ascii_lowercase(),
                });
            }
        }
    }

    // Trailing @file token (must be at start or preceded by whitespace)
    let bytes = text.as_bytes();
    let mut start = text.len();
    for (idx, ch) in text.char_indices().rev() {
        if ch.is_whitespace() {
            break;
        }
        start = idx;
    }
    if start < bytes.len() && bytes[start] == b'@' {
        let query = &text[start + 1..];
        if !query.chars().any(char::is_whitespace) {
            return Some(ComposerTrigger::Mention {
                start_byte: start,
                query: query.to_string(),
            });
        }
    }
    None
}

pub(crate) fn matching_slash_commands(query: &str) -> Vec<SlashCommandSpec> {
    let q = query.trim().to_ascii_lowercase();
    SLASH_COMMANDS
        .iter()
        .copied()
        .filter(|c| {
            q.is_empty()
                || c.id.contains(&q)
                || c.description.to_ascii_lowercase().contains(&q)
        })
        .collect()
}

pub(crate) fn fuzzy_match_files(
    entries: &[WorkspaceFileEntry],
    query: &str,
    limit: usize,
) -> Vec<WorkspaceFileEntry> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return entries.iter().take(limit).cloned().collect();
    }

    let mut scored: Vec<(i32, &WorkspaceFileEntry)> = entries
        .iter()
        .filter_map(|e| {
            let path_l = e.rel_path.to_ascii_lowercase();
            let file_l = path_l.rsplit('/').next().unwrap_or(&path_l);
            let score = if file_l == q {
                1000
            } else if file_l.starts_with(&q) {
                750 - (file_l.len() as i32)
            } else if file_l.contains(&q) {
                550 - (file_l.len() as i32)
            } else if path_l.contains(&q) {
                350 - (path_l.len() as i32)
            } else if is_subsequence(&q, &path_l) {
                180 - (path_l.len() as i32)
            } else {
                return None;
            };
            Some((score, e))
        })
        .collect();

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.rel_path.len().cmp(&b.1.rel_path.len()))
            .then_with(|| a.1.rel_path.cmp(&b.1.rel_path))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, e)| e.clone())
        .collect()
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut n_chars = needle.chars();
    let Some(mut cur) = n_chars.next() else {
        return true;
    };
    for h in haystack.chars() {
        if h == cur {
            if let Some(next) = n_chars.next() {
                cur = next;
            } else {
                return true;
            }
        }
    }
    false
}

impl HasilanxApp {
    pub(crate) fn render_composer(&self, is_hero: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();
        let app = cx.entity().clone();

        let (
            host_id,
            host_label,
            project_name,
            branch_name,
            active_agent_id,
            active_model,
            active_thinking,
            execution_mode,
            status,
            workspace_mode,
            queued,
        ) = if let Some((hs, thread)) = self.selected_thread() {
            let q: Vec<QueuedPrompt> = hs
                .prompt_queue
                .iter()
                .filter(|q| q.thread_id == thread.id)
                .cloned()
                .collect();
            let pname = hs
                .projects
                .iter()
                .find(|pr| pr.id == thread.project_id)
                .map(|pr| pr.name.clone());
            (
                hs.host_id.clone(),
                hs.label.clone(),
                pname,
                thread.branch_name.clone(),
                thread.active_agent_id.clone(),
                thread.active_model.clone(),
                thread.active_thinking.clone(),
                thread.execution_mode,
                thread.status,
                thread.workspace_mode,
                q,
            )
        } else {
            let target = self.target_project_for_new_thread();
            (
                target
                    .as_ref()
                    .map(|(h, _, _)| h.clone())
                    .unwrap_or_else(|| LOCAL_HOST_ID.to_string()),
                "Local".to_string(),
                target.map(|(_, _, n)| n),
                None,
                self.draft_agent_id.clone(),
                self.draft_model.clone(),
                self.draft_thinking.clone(),
                self.draft_mode,
                ThreadStatus::Idle,
                self.new_thread_workspace_mode,
                Vec::new(),
            )
        };

        let is_running = matches!(
            status,
            ThreadStatus::Running | ThreadStatus::WaitingApproval
        );

        let agents: Vec<AgentDescriptor> = self
            .host_states
            .get(&host_id)
            .map(|h| h.agents.clone())
            .unwrap_or_default();

        let current_agent = agents.iter().find(|a| a.id == active_agent_id);
        let current_agent_models: Vec<ModelInfo> = current_agent
            .map(|a| a.models.clone())
            .unwrap_or_default();
        let current_agent_default_model: Option<String> =
            current_agent.and_then(|a| a.default_model.clone());

        let default_model_display = current_agent_default_model.as_deref().map(|did| {
            current_agent_models
                .iter()
                .find(|m| m.id == did)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| did.to_string())
        });

        let active_model_label = active_model
            .as_deref()
            .map(|mid| {
                current_agent_models
                    .iter()
                    .find(|m| m.id == mid)
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| mid.to_string())
            })
            .or_else(|| default_model_display.clone())
            .unwrap_or_else(|| agent_display_name(&active_agent_id).to_string());

        // Resolve live Thinking / Reasoning Effort options for the active model
        let effective_model_id = active_model
            .as_deref()
            .or(current_agent_default_model.as_deref());
        let matched_model = effective_model_id
            .and_then(|mid| current_agent_models.iter().find(|m| m.id == mid));
        let (thinking_options, default_thinking_id): (Vec<ThinkingOption>, Option<String>) =
            if let Some(m) = matched_model {
                if !m.thinking_options.is_empty() {
                    (m.thinking_options.clone(), m.default_thinking.clone())
                } else if active_model.is_none() {
                    (
                        current_agent
                            .map(|a| a.thinking_options.clone())
                            .unwrap_or_default(),
                        current_agent.and_then(|a| a.default_thinking.clone()),
                    )
                } else {
                    (Vec::new(), None)
                }
            } else {
                (
                    current_agent
                        .map(|a| a.thinking_options.clone())
                        .unwrap_or_default(),
                    current_agent.and_then(|a| a.default_thinking.clone()),
                )
            };

        let default_thinking_display = default_thinking_id.as_deref().map(|did| {
            thinking_options
                .iter()
                .find(|t| t.id == did)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| did.to_string())
        });

        let active_thinking_label = active_thinking
            .as_deref()
            .map(|tid| {
                thinking_options
                    .iter()
                    .find(|t| t.id == tid)
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| tid.to_string())
            })
            .or_else(|| default_thinking_display.clone())
            .unwrap_or_else(|| "Thinking".to_string());

        let mode_icon = match execution_mode {
            ExecutionMode::Ask => Lucide::Shield,
            ExecutionMode::AcceptEdits => Lucide::Check,
            ExecutionMode::Auto => Lucide::Zap,
            ExecutionMode::Plan => Lucide::Layers,
        };

        let (brand_bg, brand_fg) = agent_brand_colors(&active_agent_id, p.is_dark);
        let brand_icon = agent_mark_icon(&active_agent_id);

        // 1. Zeron-Style Unified Tabbed Agent + Model Picker Popover (like model-picker-dark.png!)
        let unified_model_popover = {
            let agents_list = agents.clone();
            let cur_agent = active_agent_id.clone();
            let cur_model = active_model.clone();
            let model_input = self.model_input.clone();
            let host_for_refresh = host_id.clone();
            let app_c = app.clone();
            let mono_c = mono_family.clone();
            let trigger_label = active_model_label.clone();

            Popover::new("composer-model-popover")
                .anchor(Anchor::BottomRight)
                .p_2()
                .w(px(370.0))
                .trigger(
                    Button::new("trigger-unified-model")
                        .xsmall()
                        .ghost()
                        .rounded_full()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    div()
                                        .w(px(16.0))
                                        .h(px(16.0))
                                        .rounded_full()
                                        .bg(brand_bg)
                                        .text_color(brand_fg)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(Icon::new(brand_icon).size(px(10.0))),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_medium()
                                        .text_color(p.text)
                                        .max_w(px(160.0))
                                        .truncate()
                                        .child(trigger_label),
                                ),
                        ),
                )
                .content(move |_, _, cx| {
                    let selected_agent_id = app_c.read(cx).active_agent_id_for_picker(&cur_agent);
                    let agent_obj = agents_list
                        .iter()
                        .find(|a| a.id == selected_agent_id)
                        .or_else(|| agents_list.first());
                    let models_list: Vec<ModelInfo> = agent_obj
                        .map(|a| a.models.clone())
                        .unwrap_or_default();
                    let def_model_id = agent_obj.and_then(|a| a.default_model.clone());
                    let def_sub = def_model_id
                        .as_deref()
                        .map(|did| {
                            models_list
                                .iter()
                                .find(|m| m.id == did)
                                .map(|m| m.name.clone())
                                .unwrap_or_else(|| did.to_string())
                        })
                        .unwrap_or_else(|| "Agent configured default".to_string());

                    let is_same_agent = selected_agent_id == cur_agent;
                    let is_default = is_same_agent && cur_model.is_none();
                    let query = model_input.read(cx).value().trim().to_lowercase();
                    let filtered: Vec<&ModelInfo> = models_list
                        .iter()
                        .filter(|m| {
                            query.is_empty()
                                || m.id.to_lowercase().contains(&query)
                                || m.name.to_lowercase().contains(&query)
                                || m.description
                                    .as_deref()
                                    .map(|d| d.to_lowercase().contains(&query))
                                    .unwrap_or(false)
                        })
                        .collect();
                    let total_count = models_list.len();
                    let app_def = app_c.clone();
                    let app_custom = app_c.clone();
                    let app_ref = app_c.clone();
                    let hid_ref = host_for_refresh.clone();
                    let mi_def = model_input.clone();
                    let sel_aid_def = selected_agent_id.clone();

                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        // Header row: "Model" + Refresh
                        .child(
                            div()
                                .px_1()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(p.text)
                                                .child("Model"),
                                        )
                                        .when(total_count > 0, |el| {
                                            el.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_faint)
                                                    .child(format!("· {total_count}")),
                                            )
                                        }),
                                )
                                .child(
                                    Button::new("pop-model-refresh")
                                        .xsmall()
                                        .ghost()
                                        .icon(Icon::new(Lucide::RefreshCw))
                                        .tooltip("Refresh live models from agents")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            app_ref.update(cx, |this, cx| {
                                                this.refresh_agent_models(&hid_ref, cx);
                                            });
                                        })),
                                ),
                        )
                        // Zeron segmented Agent tabs (Codex | Antigravity | OpenCode)
                        .child(
                            div()
                                .p_1()
                                .rounded_lg()
                                .bg(p.surface)
                                .border_1()
                                .border_color(p.hairline)
                                .flex()
                                .items_center()
                                .gap_1()
                                .children(agents_list.iter().map(|a| {
                                    let tab_active = a.id == selected_agent_id;
                                    let aid = a.id.clone();
                                    let app_tab = app_c.clone();
                                    let (_, tab_fg) = agent_brand_colors(&a.id, p.is_dark);
                                    let tab_icon = agent_mark_icon(&a.id);
                                    div()
                                        .id(SharedString::from(format!("pop-agent-tab-{}", a.id)))
                                        .flex_1()
                                        .h(px(28.0))
                                        .px_2()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .bg(if tab_active {
                                            p.surface_elevated
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .when(tab_active, |el| {
                                            el.border_1().border_color(p.hairline_strong)
                                        })
                                        .hover(|s| {
                                            if tab_active {
                                                s.bg(p.surface_elevated)
                                            } else {
                                                s.bg(p.surface_hover)
                                            }
                                        })
                                        .on_click(cx.listener(move |_, _, window, cx| {
                                            app_tab.update(cx, |this, cx| {
                                                this.set_thread_agent(aid.clone(), cx);
                                            });
                                            window.refresh();
                                        }))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .gap_1p5()
                                        .text_xs()
                                        .when(tab_active, |el| el.font_medium())
                                        .text_color(if tab_active { p.text } else { p.text_muted })
                                        .child(
                                            Icon::new(tab_icon).size(px(11.0)).text_color(
                                                if a.available {
                                                    tab_fg
                                                } else {
                                                    p.text_faint
                                                },
                                            ),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .child(agent_display_name(&a.id)),
                                        )
                                })),
                        )
                        // Search / Custom model input
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().flex_1().child(Input::new(&model_input).xsmall()))
                                .when(!query.is_empty(), |el| {
                                    let mi_clear = model_input.clone();
                                    el.child(
                                        Button::new("pop-model-apply")
                                            .xsmall()
                                            .primary()
                                            .label("Set")
                                            .on_click(cx.listener(
                                                move |pop_state, _, window, cx| {
                                                    app_custom.update(cx, |this, cx| {
                                                        this.apply_custom_model_input(cx);
                                                    });
                                                    mi_clear.update(cx, |s, cx| {
                                                        s.set_value("", window, cx);
                                                    });
                                                    pop_state.dismiss(window, cx);
                                                    window.refresh();
                                                },
                                            )),
                                    )
                                }),
                        )
                        // Scrollable live models
                        .child(
                            div()
                                .max_h(px(270.0))
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .overflow_y_scrollbar()
                                .when(query.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .id("pop-model-default")
                                            .px_2p5()
                                            .py_1p5()
                                            .rounded_lg()
                                            .cursor_pointer()
                                            .bg(if is_default {
                                                p.surface_selected
                                            } else {
                                                rgba(0x00000000)
                                            })
                                            .hover(|s| s.bg(p.surface_hover))
                                            .on_click(cx.listener(
                                                move |pop_state, _, window, cx| {
                                                    app_def.update(cx, |this, cx| {
                                                        this.set_thread_agent(
                                                            sel_aid_def.clone(),
                                                            cx,
                                                        );
                                                        this.select_model(None, cx);
                                                    });
                                                    mi_def.update(cx, |s, cx| {
                                                        s.set_value("", window, cx);
                                                    });
                                                    pop_state.dismiss(window, cx);
                                                    window.refresh();
                                                },
                                            ))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_medium()
                                                            .text_color(p.text)
                                                            .child("Default"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(p.text_faint)
                                                            .truncate()
                                                            .child(def_sub),
                                                    ),
                                            )
                                            .when(is_default, |e| {
                                                e.child(
                                                    Icon::new(Lucide::Check)
                                                        .size(px(12.0))
                                                        .text_color(p.accent),
                                                )
                                            }),
                                    )
                                })
                                .when(total_count == 0, |el| {
                                    el.child(
                                        div()
                                            .px_2p5()
                                            .py_3()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_xs()
                                            .text_color(p.text_faint)
                                            .child(Spinner::new().xsmall().color(p.running_hsla()))
                                            .child("Fetching live models from agent..."),
                                    )
                                })
                                .children(filtered.into_iter().map(|m| {
                                    let is_sel =
                                        is_same_agent && cur_model.as_deref() == Some(m.id.as_str());
                                    let mid = m.id.clone();
                                    let aid_for_m = selected_agent_id.clone();
                                    let app_m = app_c.clone();
                                    let mi_clear = model_input.clone();
                                    let show_id_sub = m.description.is_none() && m.name != m.id;
                                    div()
                                        .id(SharedString::from(format!("pop-model-{}", m.id)))
                                        .px_2p5()
                                        .py_1p5()
                                        .rounded_lg()
                                        .cursor_pointer()
                                        .bg(if is_sel {
                                            p.surface_selected
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .hover(|s| s.bg(p.surface_hover))
                                        .on_click(cx.listener(move |pop_state, _, window, cx| {
                                            app_m.update(cx, |this, cx| {
                                                if this.active_agent_id_for_picker(&aid_for_m)
                                                    != aid_for_m
                                                {
                                                    this.set_thread_agent(aid_for_m.clone(), cx);
                                                }
                                                this.select_model(Some(mid.clone()), cx);
                                            });
                                            mi_clear.update(cx, |s, cx| {
                                                s.set_value("", window, cx);
                                            });
                                            pop_state.dismiss(window, cx);
                                            window.refresh();
                                        }))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .min_w_0()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_medium()
                                                        .text_color(p.text)
                                                        .truncate()
                                                        .child(m.name.clone()),
                                                )
                                                .when_some(m.description.clone(), |el, desc| {
                                                    el.child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(p.text_faint)
                                                            .truncate()
                                                            .child(desc),
                                                    )
                                                })
                                                .when(show_id_sub, |el| {
                                                    el.child(
                                                        div()
                                                            .text_xs()
                                                            .font_family(mono_c.clone())
                                                            .text_color(p.text_faint)
                                                            .truncate()
                                                            .child(m.id.clone()),
                                                    )
                                                }),
                                        )
                                        .when(is_sel, |el| {
                                            el.child(
                                                Icon::new(Lucide::Check)
                                                    .size(px(12.0))
                                                    .text_color(p.accent),
                                            )
                                        })
                                })),
                        )
                })
        };

        // 2. Thinking / Reasoning Effort Popover
        let thinking_popover: Option<AnyElement> = if thinking_options.is_empty() {
            None
        } else {
            let opts = thinking_options.clone();
            let cur_th = active_thinking.clone();
            let def_th_display = default_thinking_display.clone();
            let app_c = app.clone();
            Some(
                Popover::new("composer-thinking-popover")
                    .anchor(Anchor::BottomRight)
                    .p_1p5()
                    .w(px(270.0))
                    .trigger(
                        Button::new("trigger-thinking")
                            .xsmall()
                            .ghost()
                            .rounded_full()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(if active_thinking.is_some() {
                                        p.accent
                                    } else {
                                        p.text_muted
                                    })
                                    .child(Icon::new(Lucide::SlidersHorizontal).size(px(11.0)))
                                    .child(active_thinking_label),
                            ),
                    )
                    .content(move |_, _, cx| {
                        let is_default = cur_th.is_none();
                        let app_def = app_c.clone();
                        let def_sub = def_th_display
                            .clone()
                            .unwrap_or_else(|| "Model default".to_string());
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text_faint)
                                    .child("Reasoning effort"),
                            )
                            .child(
                                div()
                                    .id("pop-think-default")
                                    .px_2()
                                    .py_1p5()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .bg(if is_default {
                                        p.surface_selected
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .hover(|s| s.bg(p.surface_hover))
                                    .on_click(cx.listener(move |pop_state, _, window, cx| {
                                        app_def.update(cx, |this, cx| {
                                            this.set_thread_thinking(None, cx);
                                        });
                                        pop_state.dismiss(window, cx);
                                        window.refresh();
                                    }))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_medium()
                                                    .text_color(p.text)
                                                    .child("Default"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_faint)
                                                    .child(def_sub),
                                            ),
                                    )
                                    .when(is_default, |el| {
                                        el.child(
                                            Icon::new(Lucide::Check)
                                                .size(px(12.0))
                                                .text_color(p.accent),
                                        )
                                    }),
                            )
                            .children(
                                opts.iter()
                                    .filter(|opt| !opt.id.eq_ignore_ascii_case("default"))
                                    .map(|opt| {
                                        let is_sel = cur_th.as_deref() == Some(opt.id.as_str());
                                        let tid = opt.id.clone();
                                        let app_t = app_c.clone();
                                        div()
                                            .id(SharedString::from(format!("pop-think-{}", opt.id)))
                                            .px_2()
                                            .py_1p5()
                                            .rounded_lg()
                                            .cursor_pointer()
                                            .bg(if is_sel {
                                                p.surface_selected
                                            } else {
                                                rgba(0x00000000)
                                            })
                                            .hover(|s| s.bg(p.surface_hover))
                                            .on_click(cx.listener(
                                                move |pop_state, _, window, cx| {
                                                    app_t.update(cx, |this, cx| {
                                                        this.set_thread_thinking(
                                                            Some(tid.clone()),
                                                            cx,
                                                        );
                                                    });
                                                    pop_state.dismiss(window, cx);
                                                    window.refresh();
                                                },
                                            ))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_medium()
                                                            .text_color(p.text)
                                                            .child(opt.name.clone()),
                                                    )
                                                    .when_some(
                                                        opt.description.clone(),
                                                        |el, desc| {
                                                            el.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(p.text_faint)
                                                                    .truncate()
                                                                    .child(desc),
                                                            )
                                                        },
                                                    ),
                                            )
                                            .when(is_sel, |el| {
                                                el.child(
                                                    Icon::new(Lucide::Check)
                                                        .size(px(12.0))
                                                        .text_color(p.accent),
                                                )
                                            })
                                    }),
                            )
                    })
                    .into_any_element(),
            )
        };

        // 3. Permission Mode Popover
        let mode_popover = {
            let app_c = app.clone();
            Popover::new("composer-mode-popover")
                .anchor(Anchor::BottomLeft)
                .p_1p5()
                .w(px(290.0))
                .trigger(
                    Button::new("trigger-mode")
                        .xsmall()
                        .ghost()
                        .rounded_full()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(p.text_muted)
                                .child(Icon::new(mode_icon).size(px(11.0)))
                                .child(execution_mode.label())
                                .child(
                                    Icon::new(Lucide::ChevronDown)
                                        .size(px(10.0))
                                        .text_color(p.text_faint),
                                ),
                        ),
                )
                .content(move |_, _, cx| {
                    let modes = [
                        (
                            ExecutionMode::Ask,
                            Lucide::Shield,
                            "Ask",
                            "Confirm before edits or shell commands",
                        ),
                        (
                            ExecutionMode::AcceptEdits,
                            Lucide::Check,
                            "Accept Edits",
                            "Auto-allow edits, ask before shell",
                        ),
                        (
                            ExecutionMode::Auto,
                            Lucide::Zap,
                            "Auto",
                            "Run tools without confirmation",
                        ),
                        (
                            ExecutionMode::Plan,
                            Lucide::Layers,
                            "Plan",
                            "Draft a plan before changing files",
                        ),
                    ];
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(p.text_faint)
                                .child("Permission mode"),
                        )
                        .children(modes.into_iter().map(|(mode, icon, label, desc)| {
                            let is_active = mode == execution_mode;
                            let app_m = app_c.clone();
                            div()
                                .id(SharedString::from(format!("pop-mode-{label}")))
                                .px_2()
                                .py_1p5()
                                .rounded_lg()
                                .cursor_pointer()
                                .bg(if is_active {
                                    p.surface_selected
                                } else {
                                    rgba(0x00000000)
                                })
                                .hover(|s| s.bg(p.surface_hover))
                                .on_click(cx.listener(move |pop_state, _, window, cx| {
                                    app_m.update(cx, |this, cx| {
                                        this.set_thread_mode(mode, cx);
                                    });
                                    pop_state.dismiss(window, cx);
                                    window.refresh();
                                }))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap_2()
                                        .child(
                                            Icon::new(icon).size(px(12.0)).text_color(
                                                if is_active { p.accent } else { p.text_muted },
                                            ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_medium()
                                                        .text_color(p.text)
                                                        .child(label),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(p.text_faint)
                                                        .child(desc),
                                                ),
                                        ),
                                )
                                .when(is_active, |el| {
                                    el.child(
                                        Icon::new(Lucide::Check)
                                            .size(px(12.0))
                                            .text_color(p.accent),
                                    )
                                })
                        }))
                })
        };

        // 4. Workspace Mode Popover (Current checkout vs Worktree)
        let workspace_popover = {
            let app_c = app.clone();
            let ws_label = match workspace_mode {
                WorkspaceMode::Local => "Current checkout",
                WorkspaceMode::Worktree => "Isolated worktree",
            };
            Popover::new("composer-workspace-popover")
                .anchor(Anchor::BottomLeft)
                .p_1p5()
                .w(px(270.0))
                .trigger(
                    Button::new("trigger-workspace")
                        .xsmall()
                        .ghost()
                        .rounded_full()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(if workspace_mode == WorkspaceMode::Worktree {
                                    p.accent
                                } else {
                                    p.text_muted
                                })
                                .child(Icon::new(Lucide::FolderOpen).size(px(11.0)))
                                .child(ws_label)
                                .child(
                                    Icon::new(Lucide::ChevronDown)
                                        .size(px(10.0))
                                        .text_color(p.text_faint),
                                ),
                        ),
                )
                .content(move |_, _, cx| {
                    let modes = [
                        (
                            WorkspaceMode::Local,
                            "Current checkout",
                            "Work directly in the repository folder",
                        ),
                        (
                            WorkspaceMode::Worktree,
                            "Isolated worktree",
                            "Create an isolated Git worktree for new sessions",
                        ),
                    ];
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(p.text_faint)
                                .child("Workspace"),
                        )
                        .children(modes.into_iter().map(|(wm, label, desc)| {
                            let is_active = wm == workspace_mode;
                            let app_w = app_c.clone();
                            div()
                                .id(SharedString::from(format!("pop-ws-{label}")))
                                .px_2()
                                .py_1p5()
                                .rounded_lg()
                                .cursor_pointer()
                                .bg(if is_active {
                                    p.surface_selected
                                } else {
                                    rgba(0x00000000)
                                })
                                .hover(|s| s.bg(p.surface_hover))
                                .on_click(cx.listener(move |pop_state, _, window, cx| {
                                    app_w.update(cx, |this, cx| {
                                        this.new_thread_workspace_mode = wm;
                                        cx.notify();
                                    });
                                    pop_state.dismiss(window, cx);
                                    window.refresh();
                                }))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(p.text)
                                                .child(label),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(p.text_faint)
                                                .child(desc),
                                        ),
                                )
                                .when(is_active, |el| {
                                    el.child(
                                        Icon::new(Lucide::Check)
                                            .size(px(12.0))
                                            .text_color(p.accent),
                                    )
                                })
                        }))
                })
        };

        let hid_q = host_id.clone();

        // Core elevated Zeron rounded-2xl composer surface
        let composer_card = div()
            .w_full()
            .rounded(px(16.0))
            .border_1()
            .border_color(if status == ThreadStatus::Running {
                p.accent
            } else {
                p.hairline_strong
            })
            .bg(p.surface_elevated)
            .when(is_hero, |el| el.shadow_lg())
            .when(!is_hero, |el| el.shadow_sm())
            .overflow_hidden()
            .flex()
            .flex_col()
            // Subtle progress line while running
            .when(status == ThreadStatus::Running, |el| {
                el.child(div().h(px(2.0)).w_full().bg(p.accent))
            })
            // Queued follow-up strip
            .when(!queued.is_empty(), |el| {
                el.child(
                    div()
                        .px_3p5()
                        .py_2()
                        .border_b_1()
                        .border_color(p.hairline)
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.text_faint)
                                .child(format!("Queued ({})", queued.len())),
                        )
                        .children(queued.into_iter().map(|q| {
                            let hq = hid_q.clone();
                            let tid_q = q.thread_id.clone();
                            let qid = q.id.clone();
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(p.surface)
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .text_xs()
                                .text_color(p.text_muted)
                                .child(div().truncate().child(q.prompt))
                                .child(
                                    Button::new(SharedString::from(format!("rm-q-{}", q.id)))
                                        .xsmall()
                                        .ghost()
                                        .icon(Icon::new(Lucide::X))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.hosts_mgr.send(
                                                &hq,
                                                RpcCommand::RemoveQueuedPrompt {
                                                    thread_id: tid_q.clone(),
                                                    queue_id: qid.clone(),
                                                },
                                            );
                                            cx.notify();
                                        })),
                                )
                        })),
                )
            })
            // Attachment chips
            .when(!self.pending_attachments.is_empty(), |el| {
                el.child(
                    div()
                        .px_3p5()
                        .pt_2p5()
                        .flex()
                        .flex_wrap()
                        .gap_1p5()
                        .children(self.pending_attachments.iter().enumerate().map(
                            |(idx, att)| {
                                div()
                                    .pl_2()
                                    .pr_1()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(p.surface)
                                    .border_1()
                                    .border_color(p.hairline)
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(p.text_muted)
                                    .child(Icon::new(Lucide::Paperclip).size(px(11.0)))
                                    .child(att.label.clone())
                                    .child(
                                        Button::new(SharedString::from(format!("rm-att-{idx}")))
                                            .xsmall()
                                            .ghost()
                                            .icon(Icon::new(Lucide::X))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if idx < this.pending_attachments.len() {
                                                    this.pending_attachments.remove(idx);
                                                    cx.notify();
                                                }
                                            })),
                                    )
                            },
                        )),
                )
            })
            // Borderless auto-grow textarea with generous Zeron insets
            .child(
                div().px_4().pt_3().pb_1p5().child(
                    Textarea::new(&self.prompt_input)
                        .appearance(false)
                        .bordered(false),
                ),
            )
            // Zeron-style bottom control row inside composer pill
            .child(
                div()
                    .px_3()
                    .pb_2p5()
                    .pt_0p5()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    // Left: Attach + Mode (+ Worktree isolation badge in docked mode)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("composer-attach-btn")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::Paperclip))
                                    .tooltip("Attach files...")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pick_attachment_files(cx);
                                    })),
                            )
                            .child(mode_popover)
                            .when(!is_hero && workspace_mode == WorkspaceMode::Worktree, |el| {
                                el.child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_full()
                                        .bg(p.accent_soft)
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .text_xs()
                                        .text_color(p.accent)
                                        .child(Icon::new(Lucide::GitBranch).size(px(10.0)))
                                        .child(
                                            branch_name
                                                .clone()
                                                .unwrap_or_else(|| "Worktree".to_string()),
                                        ),
                                )
                            }),
                    )
                    // Right: Unified Model pill + Thinking pill + Circular Send/Stop button
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(unified_model_popover)
                            .when_some(thinking_popover, |el, pop| el.child(pop))
                            .when(is_running, |el| {
                                el.child(
                                    Button::new("composer-steer-btn")
                                        .xsmall()
                                        .ghost()
                                        .label("Steer")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.send_or_queue_prompt(true, window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("composer-queue-btn")
                                        .xsmall()
                                        .ghost()
                                        .label("Queue")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.send_or_queue_prompt(false, window, cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id("composer-stop-circle")
                                        .w(px(28.0))
                                        .h(px(28.0))
                                        .rounded_full()
                                        .bg(p.danger)
                                        .hover(|s| s.opacity(0.9))
                                        .cursor_pointer()
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            GlobalState::suppress_text_selection(cx);
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_selected_turn(cx);
                                        }))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            Icon::new(Lucide::Square)
                                                .size(px(11.0))
                                                .text_color(rgb(0xffffff)),
                                        ),
                                )
                            })
                            .when(!is_running, |el| {
                                let has_draft = !self
                                    .prompt_input
                                    .read(cx)
                                    .value()
                                    .trim()
                                    .is_empty()
                                    || !self.pending_attachments.is_empty();
                                el.child(
                                    div()
                                        .id("composer-send-circle")
                                        .w(px(28.0))
                                        .h(px(28.0))
                                        .rounded_full()
                                        .bg(if has_draft { p.accent } else { p.surface })
                                        .when(!has_draft, |d| d.border_1().border_color(p.hairline))
                                        .hover(|s| {
                                            if has_draft {
                                                s.bg(p.accent_hover)
                                            } else {
                                                s.bg(p.surface_hover)
                                            }
                                        })
                                        .cursor_pointer()
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            GlobalState::suppress_text_selection(cx);
                                        })
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.send_or_queue_prompt(false, window, cx);
                                        }))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            Icon::new(Lucide::ArrowUp).size(px(14.0)).text_color(
                                                if has_draft {
                                                    p.accent_fg
                                                } else {
                                                    p.text_faint
                                                },
                                            ),
                                        ),
                                )
                            }),
                    ),
            );

        let autocomplete_el = self.render_composer_autocomplete(cx);
        let has_autocomplete = autocomplete_el.is_some();
        let composer_shell = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1p5()
            .when(has_autocomplete, |el| {
                el.key_context("ComposerAutocomplete")
            })
            .on_action(cx.listener(|this, _: &ComposerAcceptAutocomplete, window, cx| {
                this.apply_selected_composer_autocomplete(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ComposerNextAutocomplete, _, cx| {
                let text = this.prompt_input.read(cx).value().to_string();
                if let Some(trigger) = detect_composer_trigger(&text) {
                    let count = this.composer_autocomplete_count(&trigger);
                    if count > 0 {
                        this.composer_popup_idx = (this.composer_popup_idx + 1) % count;
                        cx.notify();
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &ComposerPrevAutocomplete, _, cx| {
                let text = this.prompt_input.read(cx).value().to_string();
                if let Some(trigger) = detect_composer_trigger(&text) {
                    let count = this.composer_autocomplete_count(&trigger);
                    if count > 0 {
                        this.composer_popup_idx = if this.composer_popup_idx == 0 {
                            count - 1
                        } else {
                            this.composer_popup_idx - 1
                        };
                        cx.notify();
                    }
                }
            }))
            .when_some(autocomplete_el, |el, popup| el.child(popup))
            .child(composer_card);

        if is_hero {
            // Zeron Hero Layout: context row above-right + floating composer + checkout/branch footer below-left
            div()
                .w_full()
                .max_w(px(680.0))
                .mx_auto()
                .flex()
                .flex_col()
                .gap_2()
                // Subtle project & host line aligned top-right (just like Zeron app-screenshot.jpg!)
                .child(
                    div()
                        .px_1()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap_3()
                        .when_some(project_name, |el, pname| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(p.text_muted)
                                    .child(Icon::new(Lucide::FolderOpen).size(px(11.0)))
                                    .child(pname),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(p.text_muted)
                                .child(Icon::new(Lucide::Cpu).size(px(11.0)))
                                .child(host_label),
                        ),
                )
                .child(composer_shell)
                // Subtle checkout & branch footer below-left (just like Zeron app-screenshot.jpg!)
                .child(
                    div()
                        .px_1()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(workspace_popover)
                                .when_some(branch_name, |el, br| {
                                    el.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_xs()
                                            .text_color(p.text_faint)
                                            .child(Icon::new(Lucide::GitBranch).size(px(11.0)))
                                            .child(br),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.text_faint)
                                .child(if self.send_on_enter {
                                    "Enter to send · @file · /cmd"
                                } else {
                                    "Ctrl+Enter to send · @file · /cmd"
                                }),
                        ),
                )
                .into_any_element()
        } else {
            // Docked bottom floating pill composer inside active session
            div()
                .w_full()
                .px_5()
                .pb_3p5()
                .pt_1p5()
                .child(
                    div()
                        .w_full()
                        .max_w(px(760.0))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .child(composer_shell),
                )
                .into_any_element()
        }
    }

    fn matching_composer_slash_options(&self, query: &str) -> Vec<ComposerSlashOption> {
        let q = query.trim().to_ascii_lowercase();
        let mut out: Vec<ComposerSlashOption> = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        if let Some(cfg) = self.active_project_workflow() {
            for cmd in &cfg.commands {
                let raw_name = cmd.name.trim().trim_start_matches('/');
                if raw_name.is_empty() {
                    continue;
                }
                let id_lower = raw_name.to_ascii_lowercase();
                let desc = if cmd.description.trim().is_empty() {
                    cmd.prompt.lines().next().unwrap_or("Project command").trim().to_string()
                } else {
                    cmd.description.trim().to_string()
                };
                if q.is_empty()
                    || id_lower.contains(&q)
                    || desc.to_ascii_lowercase().contains(&q)
                {
                    if seen_ids.insert(id_lower.clone()) {
                        out.push(ComposerSlashOption {
                            id: id_lower,
                            command: format!("/{raw_name}"),
                            description: desc,
                            icon: Lucide::Sparkles,
                            is_project: true,
                            project_prompt: Some(cmd.prompt.clone()),
                        });
                    }
                }
            }
        }

        for c in matching_slash_commands(&q) {
            if seen_ids.insert(c.id.to_string()) {
                out.push(ComposerSlashOption {
                    id: c.id.to_string(),
                    command: c.command.to_string(),
                    description: c.description.to_string(),
                    icon: c.icon,
                    is_project: false,
                    project_prompt: None,
                });
            }
        }

        out.truncate(9);
        out
    }

    fn composer_autocomplete_count(&self, trigger: &ComposerTrigger) -> usize {
        match trigger {
            ComposerTrigger::Mention { query, .. } => {
                fuzzy_match_files(&self.files_inspector.entries, query, 7).len()
            }
            ComposerTrigger::Slash { query } => self.matching_composer_slash_options(query).len(),
        }
    }

    fn render_composer_autocomplete(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = self.prompt_input.read(cx).value().to_string();
        let trigger = detect_composer_trigger(&text)?;
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();

        match trigger {
            ComposerTrigger::Mention { start_byte, query } => {
                let matches = fuzzy_match_files(&self.files_inspector.entries, &query, 7);
                if matches.is_empty() {
                    return None;
                }
                let sel_idx = self.composer_popup_idx.min(matches.len().saturating_sub(1));

                Some(
                    div()
                        .w_full()
                        .rounded_xl()
                        .border_1()
                        .border_color(p.hairline_strong)
                        .bg(p.surface_elevated)
                        .shadow_lg()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px_3()
                                .py_1p5()
                                .border_b_1()
                                .border_color(p.hairline)
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_xs()
                                .text_color(p.text_faint)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            Icon::new(Lucide::FileCode)
                                                .size(px(11.0))
                                                .text_color(p.accent),
                                        )
                                        .child(if query.is_empty() {
                                            "Mention & attach workspace file".to_string()
                                        } else {
                                            format!("Files matching \"@{query}\"")
                                        }),
                                )
                                .child("Tab or click to attach"),
                        )
                        .child(
                            div()
                                .p_1p5()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .children(matches.into_iter().enumerate().map(|(idx, entry)| {
                                    let is_sel = idx == sel_idx;
                                    let (badge_txt, badge_bg, badge_fg) =
                                        ext_badge(&entry.rel_path, p.is_dark);
                                    let entry_clone = entry.clone();
                                    div()
                                        .id(SharedString::from(format!("comp-mention-{idx}")))
                                        .px_2p5()
                                        .py_1p5()
                                        .rounded_lg()
                                        .cursor_pointer()
                                        .bg(if is_sel {
                                            p.surface_selected
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .hover(|s| s.bg(p.surface_hover))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            GlobalState::suppress_text_selection(cx);
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.apply_file_mention(
                                                start_byte,
                                                &entry_clone,
                                                window,
                                                cx,
                                            );
                                        }))
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
                                                .flex_1()
                                                .child(
                                                    div()
                                                        .w(px(38.0))
                                                        .px_1()
                                                        .py(px(1.0))
                                                        .rounded_sm()
                                                        .bg(badge_bg)
                                                        .text_color(badge_fg)
                                                        .text_xs()
                                                        .font_semibold()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .flex_shrink_0()
                                                        .child(badge_txt),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_family(mono_family.clone())
                                                        .text_color(p.text)
                                                        .truncate()
                                                        .child(entry.rel_path),
                                                ),
                                        )
                                        .when(is_sel, |el| {
                                            el.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.accent)
                                                    .child("Attach"),
                                            )
                                        })
                                })),
                        )
                        .into_any_element(),
                )
            }
            ComposerTrigger::Slash { query } => {
                let matches = self.matching_composer_slash_options(&query);
                if matches.is_empty() {
                    return None;
                }
                let has_repo_cmds = matches.iter().any(|m| m.is_project);
                let sel_idx = self.composer_popup_idx.min(matches.len().saturating_sub(1));

                Some(
                    div()
                        .w_full()
                        .rounded_xl()
                        .border_1()
                        .border_color(p.hairline_strong)
                        .bg(p.surface_elevated)
                        .shadow_lg()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px_3()
                                .py_1p5()
                                .border_b_1()
                                .border_color(p.hairline)
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_xs()
                                .text_color(p.text_faint)
                                .child(if has_repo_cmds {
                                    "Slash commands · .hasilanx.toml"
                                } else {
                                    "Slash commands"
                                })
                                .child("Tab or click to run"),
                        )
                        .child(
                            div()
                                .p_1p5()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .children(matches.into_iter().enumerate().map(|(idx, cmd)| {
                                    let is_sel = idx == sel_idx;
                                    let is_project = cmd.is_project;
                                    let cmd_exec = cmd.clone();
                                    div()
                                        .id(SharedString::from(format!("comp-slash-{idx}-{}", cmd.id)))
                                        .px_2p5()
                                        .py_1p5()
                                        .rounded_lg()
                                        .cursor_pointer()
                                        .bg(if is_sel {
                                            p.surface_selected
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .hover(|s| s.bg(p.surface_hover))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            GlobalState::suppress_text_selection(cx);
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.execute_slash_option(&cmd_exec, window, cx);
                                        }))
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
                                                .flex_1()
                                                .child(
                                                    Icon::new(cmd.icon)
                                                        .size(px(12.0))
                                                        .text_color(if is_sel || is_project {
                                                            p.accent
                                                        } else {
                                                            p.text_muted
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_family(mono_family.clone())
                                                        .font_semibold()
                                                        .text_color(p.text)
                                                        .flex_shrink_0()
                                                        .child(cmd.command),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(p.text_muted)
                                                        .truncate()
                                                        .child(cmd.description),
                                                ),
                                        )
                                        .when(is_project, |el| {
                                            el.child(
                                                div()
                                                    .px_1p5()
                                                    .py(px(1.0))
                                                    .rounded_sm()
                                                    .bg(p.accent_soft)
                                                    .text_color(p.accent)
                                                    .text_xs()
                                                    .font_medium()
                                                    .flex_shrink_0()
                                                    .child("Repo"),
                                            )
                                        })
                                })),
                        )
                        .into_any_element(),
                )
            }
        }
    }

    pub(crate) fn apply_selected_composer_autocomplete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.prompt_input.read(cx).value().to_string();
        let Some(trigger) = detect_composer_trigger(&text) else {
            return;
        };
        match trigger {
            ComposerTrigger::Mention { start_byte, query } => {
                let matches = fuzzy_match_files(&self.files_inspector.entries, &query, 7);
                if let Some(entry) = matches
                    .get(self.composer_popup_idx.min(matches.len().saturating_sub(1)))
                    .cloned()
                {
                    self.apply_file_mention(start_byte, &entry, window, cx);
                }
            }
            ComposerTrigger::Slash { query } => {
                let matches = self.matching_composer_slash_options(&query);
                if let Some(cmd) =
                    matches.get(self.composer_popup_idx.min(matches.len().saturating_sub(1)))
                {
                    self.execute_slash_option(cmd, window, cx);
                }
            }
        }
    }

    pub(crate) fn apply_file_mention(
        &mut self,
        start_byte: usize,
        entry: &WorkspaceFileEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.prompt_input.read(cx).value().to_string();
        let prefix = if start_byte <= current.len() && current.is_char_boundary(start_byte) {
            &current[..start_byte]
        } else {
            ""
        };
        let next = format!("{prefix}@{} ", entry.rel_path);
        self.prompt_input.update(cx, |s, cx| {
            s.set_value(next, window, cx);
            s.focus(window, cx);
        });
        let abs_str = entry.abs_path.to_string_lossy().to_string();
        if !self.pending_attachments.iter().any(|a| a.path == abs_str) {
            self.pending_attachments.push(Attachment {
                label: entry.rel_path.clone(),
                path: abs_str,
            });
        }
        self.composer_popup_idx = 0;
        cx.notify();
    }

    pub(crate) fn try_execute_exact_slash_command(
        &mut self,
        raw_prompt: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let trimmed = raw_prompt.trim();
        let Some(rest) = trimmed.strip_prefix('/') else {
            return false;
        };
        if rest.chars().any(char::is_whitespace) {
            return false;
        }
        let lower = rest.to_ascii_lowercase();
        if let Some(cfg) = self.active_project_workflow() {
            if let Some(pcmd) = cfg
                .commands
                .iter()
                .find(|c| c.name.trim().trim_start_matches('/').eq_ignore_ascii_case(&lower))
            {
                let prompt = pcmd.prompt.clone();
                self.prompt_input.update(cx, |s, cx| {
                    s.set_value("", window, cx);
                    s.focus(window, cx);
                });
                self.composer_popup_idx = 0;
                self.send_direct_prompt(prompt, window, cx);
                return true;
            }
        }
        if let Some(cmd) = SLASH_COMMANDS.iter().find(|c| c.id == lower) {
            return self.execute_slash_command(cmd.id, window, cx);
        }
        false
    }

    pub(crate) fn execute_slash_option(
        &mut self,
        opt: &ComposerSlashOption,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(prompt) = &opt.project_prompt {
            self.prompt_input.update(cx, |s, cx| {
                s.set_value("", window, cx);
                s.focus(window, cx);
            });
            self.composer_popup_idx = 0;
            self.send_direct_prompt(prompt.clone(), window, cx);
            return true;
        }
        self.execute_slash_command(&opt.id, window, cx)
    }

    pub(crate) fn execute_slash_command(
        &mut self,
        cmd_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.prompt_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        self.composer_popup_idx = 0;

        match cmd_id {
            "check" => {
                if self.active_project_workflow().is_some() {
                    self.run_selected_project_checks(None, cx);
                } else {
                    self.show_checks_drawer = true;
                    cx.notify();
                }
            }
            "init-workflow" => self.init_active_project_workflow(cx),
            "review" => {
                self.send_direct_prompt(
                    "Review the current uncommitted git diff carefully. Highlight any bugs, regressions, edge cases, or missing tests.".to_string(),
                    window,
                    cx,
                );
            }
            "test" => {
                self.send_direct_prompt(
                    "Run the project's test suite and summarize any failures, panics, or compiler warnings.".to_string(),
                    window,
                    cx,
                );
            }
            "explain" => {
                self.send_direct_prompt(
                    "Give a concise architectural overview of this repository, its crate/module boundaries, and main entry points.".to_string(),
                    window,
                    cx,
                );
            }
            "plan" => self.set_thread_mode(ExecutionMode::Plan, cx),
            "auto" => self.set_thread_mode(ExecutionMode::Auto, cx),
            "ask" => self.set_thread_mode(ExecutionMode::Ask, cx),
            "worktree" => {
                self.new_thread_workspace_mode = match self.new_thread_workspace_mode {
                    WorkspaceMode::Local => WorkspaceMode::Worktree,
                    WorkspaceMode::Worktree => WorkspaceMode::Local,
                };
                cx.notify();
            }
            "diff" => self.toggle_diff_pane(cx),
            "files" => self.toggle_files_pane(cx),
            "terminal" => self.toggle_terminal_dock(Some(window), cx),
            "split" => self.toggle_split_pane(cx),
            "parallel" => self.start_parallel_worktree_compare(cx),
            _ => return false,
        }
        true
    }

    pub(crate) fn active_agent_id_for_picker(&self, fallback: &str) -> String {
        self.selected_thread()
            .map(|(_, t)| t.active_agent_id.clone())
            .unwrap_or_else(|| {
                if self.draft_agent_id.is_empty() {
                    fallback.to_string()
                } else {
                    self.draft_agent_id.clone()
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[::core::prelude::v1::test]
    fn detects_at_mentions_and_slash_commands() {
        assert_eq!(detect_composer_trigger(""), None);
        assert_eq!(detect_composer_trigger("hello world"), None);
        assert_eq!(
            detect_composer_trigger("/rev"),
            Some(ComposerTrigger::Slash {
                query: "rev".to_string()
            })
        );
        assert_eq!(detect_composer_trigger("/review extra words"), None);
        assert_eq!(
            detect_composer_trigger("check @src/main"),
            Some(ComposerTrigger::Mention {
                start_byte: 6,
                query: "src/main".to_string()
            })
        );
        assert_eq!(
            detect_composer_trigger("@"),
            Some(ComposerTrigger::Mention {
                start_byte: 0,
                query: "".to_string()
            })
        );
        // Email address should NOT trigger mention because '@' is not preceded by whitespace
        assert_eq!(detect_composer_trigger("email foo@bar.com"), None);
    }

    #[::core::prelude::v1::test]
    fn fuzzy_matches_prioritize_filename_hits() {
        let files = vec![
            WorkspaceFileEntry {
                rel_path: "docs/main_guide.md".into(),
                abs_path: PathBuf::from("/repo/docs/main_guide.md"),
                size_bytes: 100,
            },
            WorkspaceFileEntry {
                rel_path: "crates/app/src/main.rs".into(),
                abs_path: PathBuf::from("/repo/crates/app/src/main.rs"),
                size_bytes: 200,
            },
            WorkspaceFileEntry {
                rel_path: "Cargo.toml".into(),
                abs_path: PathBuf::from("/repo/Cargo.toml"),
                size_bytes: 300,
            },
        ];
        let hits = fuzzy_match_files(&files, "main.rs", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel_path, "crates/app/src/main.rs");

        let sub = fuzzy_match_files(&files, "ctoml", 5);
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].rel_path, "Cargo.toml");
    }
}
