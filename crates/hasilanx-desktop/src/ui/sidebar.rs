use super::{ActiveModal, HasilanxApp, SidebarFilter};
use crate::client::LOCAL_HOST_ID;
use crate::state::ThreadKey;
use crate::theme::{SidebarDensity, agent_brand_colors, project_tile_colors};
use crate::time_fmt::{agent_display_name, relative_time};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::GlobalState;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable, StyledExt, Theme};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use hasilanx_protocol::{ThreadRecord, ThreadStatus, WorkspaceMode};

pub(crate) fn agent_mark_icon(agent_id: &str) -> Lucide {
    let lower = agent_id.to_ascii_lowercase();
    if lower.contains("codex") {
        Lucide::Code
    } else if lower.contains("agy") || lower.contains("antigravity") {
        Lucide::Sparkles
    } else if lower.contains("opencode") {
        Lucide::Terminal
    } else {
        Lucide::Bot
    }
}

impl HasilanxApp {
    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let online_hosts = self.host_states.values().filter(|h| h.connected).count();
        let total_hosts = self.host_states.len();
        let target_project = self.target_project_for_new_thread();
        let density = self.sidebar_density;
        let filter = self.sidebar_filter;

        // Gather global filter counts across all hosts
        let mut needs_count = 0usize;
        let mut working_count = 0usize;
        let mut pinned_existing_count = 0usize;
        let mut pinned_threads: Vec<(String, String, ThreadRecord)> = Vec::new();
        for hid in &self.host_order {
            if let Some(hs) = self.host_states.get(hid) {
                for th in &hs.threads {
                    if matches!(th.status, ThreadStatus::WaitingApproval | ThreadStatus::Error) {
                        needs_count += 1;
                    }
                    if th.status == ThreadStatus::Running {
                        working_count += 1;
                    }
                    if self.pinned_threads.contains(&th.id) {
                        pinned_existing_count += 1;
                        if self.matches_sidebar_filter(th) {
                            let pname = hs
                                .projects
                                .iter()
                                .find(|pr| pr.id == th.project_id)
                                .map(|pr| pr.name.clone())
                                .unwrap_or_default();
                            pinned_threads.push((hs.host_id.clone(), pname, th.clone()));
                        }
                    }
                }
            }
        }
        pinned_threads.sort_by(|a, b| b.2.updated_at.cmp(&a.2.updated_at));
        let pinned_count = pinned_existing_count;

        // Build collapsible project groups with colored initial tiles & Zeron tree connectors
        let mut project_sections: Vec<AnyElement> = Vec::new();
        let mut total_projects = 0usize;
        let mut matched_any = !pinned_threads.is_empty();

        for hid in &self.host_order {
            let Some(hs) = self.host_states.get(hid) else {
                continue;
            };
            for proj in &hs.projects {
                total_projects += 1;
                let mut threads: Vec<ThreadRecord> = hs
                    .threads
                    .iter()
                    .filter(|t| {
                        t.project_id == proj.id
                            && self.matches_sidebar_filter(t)
                            && !(filter == SidebarFilter::All && self.pinned_threads.contains(&t.id))
                    })
                    .cloned()
                    .collect();
                threads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

                if filter != SidebarFilter::All && threads.is_empty() {
                    continue;
                }
                if !threads.is_empty() {
                    matched_any = true;
                }

                let is_folded = self.collapsed_projects.contains(&proj.id);
                let proj_id_fold = proj.id.clone();
                let hid_new = hs.host_id.clone();
                let pid_new = proj.id.clone();
                let show_host = total_hosts > 1 && !hs.is_local;
                let host_label = hs.label.clone();
                let (tile_bg, tile_fg) = project_tile_colors(&proj.name, p.is_dark);
                let initial: String = proj
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "P".to_string());
                let count = threads.len();

                let thread_rows: Vec<AnyElement> = threads
                    .iter()
                    .map(|th| {
                        self.render_sidebar_thread_row(
                            &hs.host_id,
                            &proj.name,
                            th,
                            density,
                            false,
                            cx,
                        )
                    })
                    .collect();

                project_sections.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        // Project header row (foldable)
                        .child(
                            div()
                                .id(SharedString::from(format!("sb-proj-hdr-{}", proj.id)))
                                .h(px(28.0))
                                .px_1p5()
                                .rounded_md()
                                .hover(|s| s.bg(p.surface_hover))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.collapsed_projects.contains(&proj_id_fold) {
                                        this.collapsed_projects.remove(&proj_id_fold);
                                    } else {
                                        this.collapsed_projects.insert(proj_id_fold.clone());
                                    }
                                    cx.notify();
                                }))
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
                                        .flex_1()
                                        .child(
                                            div()
                                                .w(px(18.0))
                                                .h(px(18.0))
                                                .rounded_sm()
                                                .bg(tile_bg)
                                                .text_color(tile_fg)
                                                .text_xs()
                                                .font_semibold()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .flex_shrink_0()
                                                .child(initial),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(p.text)
                                                .truncate()
                                                .child(proj.name.clone()),
                                        )
                                        .when(count > 0, |el| {
                                            el.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_faint)
                                                    .flex_shrink_0()
                                                    .child(format!("{count}")),
                                            )
                                        })
                                        .when(show_host, |el| {
                                            el.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_faint)
                                                    .flex_shrink_0()
                                                    .child(format!("· {host_label}")),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_0p5()
                                        .flex_shrink_0()
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "sb-proj-new-{}",
                                                proj.id
                                            )))
                                            .xsmall()
                                            .ghost()
                                            .icon(Icon::new(Lucide::Plus))
                                            .tooltip("New session in project")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.create_thread(
                                                    hid_new.clone(),
                                                    pid_new.clone(),
                                                    Some(window),
                                                    cx,
                                                );
                                            })),
                                        )
                                        .child(
                                            Icon::new(if is_folded {
                                                Lucide::ChevronRight
                                            } else {
                                                Lucide::ChevronDown
                                            })
                                            .size(px(11.0))
                                            .text_color(p.text_faint),
                                        ),
                                ),
                        )
                        // Zeron tree-line indented session rows
                        .when(!is_folded, |el| {
                            el.child(
                                div()
                                    .ml(px(10.0))
                                    .pl_2()
                                    .border_l_1()
                                    .border_color(p.tree_line)
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .children(if thread_rows.is_empty() {
                                        vec![
                                            div()
                                                .px_2()
                                                .py_1()
                                                .text_xs()
                                                .text_color(p.text_faint)
                                                .child("No sessions yet")
                                                .into_any_element(),
                                        ]
                                    } else {
                                        thread_rows
                                    }),
                            )
                        })
                        .into_any_element(),
                );
            }
        }

        let target_project_footer = target_project.clone();

        div()
            .size_full()
            .rounded_xl()
            .border_1()
            .border_color(p.hairline)
            .bg(p.sidebar)
            .overflow_hidden()
            .flex()
            .flex_col()
            .text_color(p.text)
            // Top header bar
            .child(
                div()
                    .h(px(42.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(7.0))
                                    .h(px(7.0))
                                    .rounded_full()
                                    .bg(p.accent),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .child("Sessions"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_0p5()
                            .child(
                                Button::new("sb-cmd-k")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::Search))
                                    .tooltip("Search sessions & commands (Ctrl+K)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_command_palette(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("sb-density-toggle")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(match density {
                                        SidebarDensity::Regular => Lucide::Rows3,
                                        SidebarDensity::Compact => Lucide::List,
                                    }))
                                    .tooltip(match density {
                                        SidebarDensity::Regular => "Switch to compact density",
                                        SidebarDensity::Compact => "Switch to regular density",
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_sidebar_density(cx);
                                    })),
                            )
                            .child(
                                Button::new("sb-collapse")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::PanelLeftClose))
                                    .tooltip("Hide Sidebar (Ctrl+B)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_sidebar = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            // Zeron status filter pills strip (All · Needs you · Working · Pinned)
            .child(
                div()
                    .px_2p5()
                    .pb_2()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(self.render_filter_pill(SidebarFilter::All, "All", None, cx))
                    .child(self.render_filter_pill(
                        SidebarFilter::NeedsAttention,
                        "Needs you",
                        (needs_count > 0).then_some(needs_count),
                        cx,
                    ))
                    .child(self.render_filter_pill(
                        SidebarFilter::Working,
                        "Working",
                        (working_count > 0).then_some(working_count),
                        cx,
                    ))
                    .when(pinned_count > 0 || filter == SidebarFilter::Pinned, |el| {
                        el.child(self.render_filter_pill(
                            SidebarFilter::Pinned,
                            "Pinned",
                            (pinned_count > 0).then_some(pinned_count),
                            cx,
                        ))
                    }),
            )
            // Scrollable session tree
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .py_1()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .overflow_y_scrollbar()
                    // Pinned sessions group
                    .when(!pinned_threads.is_empty(), |el| {
                        let rows: Vec<AnyElement> = pinned_threads
                            .iter()
                            .map(|(hid, pname, th)| {
                                self.render_sidebar_thread_row(hid, pname, th, density, true, cx)
                            })
                            .collect();
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .text_xs()
                                        .font_medium()
                                        .text_color(p.accent)
                                        .child(Icon::new(Lucide::Pin).size(px(10.0)))
                                        .child(format!("Pinned · {}", rows.len())),
                                )
                                .children(rows),
                        )
                    })
                    // Project groups
                    .children(project_sections)
                    // Empty filter state
                    .when(!matched_any && filter != SidebarFilter::All, |el| {
                        el.child(
                            div()
                                .px_3()
                                .py_6()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(p.text_faint)
                                .child("No matching sessions"),
                        )
                    })
                    // Quiet Add project action
                    .child(
                        div()
                            .id("sb-add-project-row")
                            .mt_1()
                            .h(px(28.0))
                            .px_2()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.surface_hover).text_color(p.text))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                GlobalState::suppress_text_selection(cx);
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_add_project_modal(LOCAL_HOST_ID.to_string(), window, cx);
                            }))
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(p.text_faint)
                            .child(Icon::new(Lucide::FolderPlus).size(px(12.0)))
                            .child(if total_projects == 0 {
                                "Add project folder"
                            } else {
                                "Add project"
                            }),
                    ),
            )
            // Zeron-style bottom footer (Settings + Online status + Primary Compose button)
            .child(
                div()
                    .h(px(44.0))
                    .px_2p5()
                    .border_t_1()
                    .border_color(p.hairline)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .child(
                                Button::new("sb-settings")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::Settings))
                                    .tooltip("Settings (Ctrl+,)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_modal = ActiveModal::Settings;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .flex_shrink_0()
                                            .rounded_full()
                                            .bg(if online_hosts > 0 {
                                                p.success
                                            } else {
                                                p.approval
                                            }),
                                    )
                                    .child(
                                        div().truncate().child(if total_hosts == 1 {
                                            if online_hosts == 1 {
                                                "Local".to_string()
                                            } else {
                                                "Connecting...".to_string()
                                            }
                                        } else {
                                            format!("{online_hosts}/{total_hosts} hosts")
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("sb-new-thread")
                            .xsmall()
                            .primary()
                            .icon(Icon::new(Lucide::SquarePen))
                            .label("New")
                            .tooltip("New session (Ctrl+N)")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some((hid, pid, _)) = target_project_footer.clone() {
                                    this.create_thread(hid, pid, Some(window), cx);
                                } else {
                                    this.open_add_project_modal(
                                        LOCAL_HOST_ID.to_string(),
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    fn matches_sidebar_filter(&self, thread: &ThreadRecord) -> bool {
        match self.sidebar_filter {
            SidebarFilter::All => true,
            SidebarFilter::NeedsAttention => {
                matches!(
                    thread.status,
                    ThreadStatus::WaitingApproval | ThreadStatus::Error
                )
            }
            SidebarFilter::Working => thread.status == ThreadStatus::Running,
            SidebarFilter::Pinned => self.pinned_threads.contains(&thread.id),
        }
    }

    fn render_filter_pill(
        &self,
        filter: SidebarFilter,
        label: &'static str,
        count: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let active = self.sidebar_filter == filter;
        div()
            .id(SharedString::from(format!("sb-flt-{label}")))
            .px_2()
            .py_0p5()
            .rounded_full()
            .cursor_pointer()
            .bg(if active { p.accent } else { p.surface })
            .hover(|s| {
                if active {
                    s.bg(p.accent_hover)
                } else {
                    s.bg(p.surface_hover)
                }
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                GlobalState::suppress_text_selection(cx);
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sidebar_filter = filter;
                cx.notify();
            }))
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .font_medium()
            .text_color(if active { p.accent_fg } else { p.text_muted })
            .child(label)
            .when_some(count, |el, n| {
                el.child(
                    div()
                        .px_1()
                        .rounded_full()
                        .bg(if active {
                            rgba(0xffffff28)
                        } else {
                            p.accent_soft
                        })
                        .text_xs()
                        .text_color(if active { p.accent_fg } else { p.text })
                        .child(format!("{n}")),
                )
            })
            .into_any_element()
    }

    fn render_sidebar_thread_row(
        &self,
        host_id: &str,
        project_name: &str,
        thread: &ThreadRecord,
        density: SidebarDensity,
        is_pinned_section: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let key = ThreadKey::new(host_id, &thread.id);
        let is_selected = self.selected.as_ref() == Some(&key);
        let is_split = self.split_thread.as_ref() == Some(&key);
        let is_pinned = self.pinned_threads.contains(&thread.id);
        let rel = relative_time(thread.updated_at);
        let (mark_bg, mark_fg) = agent_brand_colors(&thread.active_agent_id, p.is_dark);
        let mark_icon = agent_mark_icon(&thread.active_agent_id);
        let is_regular = density == SidebarDensity::Regular;

        let standing_el: AnyElement = match thread.status {
            ThreadStatus::Running => Spinner::new()
                .xsmall()
                .color(p.running_hsla())
                .into_any_element(),
            ThreadStatus::WaitingApproval => div()
                .px_1p5()
                .py_0p5()
                .rounded_full()
                .bg(p.approval_soft)
                .text_xs()
                .font_medium()
                .text_color(p.approval)
                .child("Input")
                .into_any_element(),
            ThreadStatus::Error => div()
                .px_1p5()
                .py_0p5()
                .rounded_full()
                .bg(p.danger_soft)
                .text_xs()
                .font_medium()
                .text_color(p.danger)
                .child("Failed")
                .into_any_element(),
            ThreadStatus::Idle => div()
                .text_xs()
                .text_color(p.text_faint)
                .child(rel)
                .into_any_element(),
        };

        let subtitle = if is_pinned_section && !project_name.is_empty() {
            if let Some(ref br) = thread.branch_name {
                format!("{project_name} · {br}")
            } else {
                format!("{project_name} · {}", agent_display_name(&thread.active_agent_id))
            }
        } else if let Some(ref br) = thread.branch_name {
            format!("{} · {br}", agent_display_name(&thread.active_agent_id))
        } else {
            agent_display_name(&thread.active_agent_id).to_string()
        };

        let key_click = key.clone();
        let tid_pin = thread.id.clone();
        let mono_family = Theme::global(cx).mono_font_family.clone();
        let diff_counts: Option<(u32, u32)> = self
            .host_states
            .get(host_id)
            .and_then(|hs| hs.diffs.get(&thread.id))
            .filter(|d| !d.files.is_empty())
            .map(|d| {
                (
                    d.files.iter().map(|f| f.additions).sum(),
                    d.files.iter().map(|f| f.deletions).sum(),
                )
            });
        let is_worktree = thread.workspace_mode == WorkspaceMode::Worktree;

        div()
            .id(SharedString::from(format!(
                "sb-th-{}-{}-{}",
                if is_pinned_section { "pin" } else { "prj" },
                host_id,
                thread.id
            )))
            .px_2()
            .py(if is_regular { px(6.0) } else { px(5.0) })
            .rounded_lg()
            .cursor_pointer()
            .bg(if is_selected {
                p.surface_selected
            } else {
                rgba(0x00000000)
            })
            .hover(|s| {
                if is_selected {
                    s.bg(p.surface_selected)
                } else {
                    s.bg(p.surface_hover)
                }
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                GlobalState::suppress_text_selection(cx);
            })
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                if ev.modifiers().shift {
                    this.open_in_split_pane(key_click.clone(), cx);
                } else {
                    this.select_thread(key_click.clone(), window, cx);
                }
            }))
            .flex()
            .items_center()
            .gap_2()
            // Agent brand mark slot
            .child(
                div()
                    .w(if is_regular { px(24.0) } else { px(18.0) })
                    .h(if is_regular { px(24.0) } else { px(18.0) })
                    .rounded_md()
                    .bg(mark_bg)
                    .text_color(mark_fg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .child(
                        Icon::new(mark_icon).size(if is_regular { px(12.0) } else { px(10.0) }),
                    ),
            )
            // Title + optional 2nd line
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_1p5()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .when(
                                        is_selected
                                            || matches!(
                                                thread.status,
                                                ThreadStatus::Running
                                                    | ThreadStatus::WaitingApproval
                                                    | ThreadStatus::Error
                                            ),
                                        |el| el.font_medium(),
                                    )
                                    .text_color(if is_selected
                                        || thread.status != ThreadStatus::Idle
                                    {
                                        p.text
                                    } else {
                                        p.text_muted
                                    })
                                    .truncate()
                                    .child(thread.title.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .flex_shrink_0()
                                    .when(is_split, |el| {
                                        el.child(
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded_full()
                                                .bg(p.accent_soft)
                                                .text_xs()
                                                .font_medium()
                                                .text_color(p.accent)
                                                .child("Split"),
                                        )
                                    })
                                    .child(standing_el)
                                    .when(is_selected || is_pinned, |el| {
                                        el.child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "sb-pin-{}-{}",
                                                    if is_pinned_section { "p" } else { "n" },
                                                    thread.id
                                                )))
                                                .p_0p5()
                                                .rounded_sm()
                                                .hover(|s| s.bg(p.surface_hover))
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    GlobalState::suppress_text_selection(cx);
                                                    cx.stop_propagation();
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_pin_thread(&tid_pin, cx);
                                                    cx.stop_propagation();
                                                }))
                                                .child(
                                                    Icon::new(if is_pinned {
                                                        Lucide::PinOff
                                                    } else {
                                                        Lucide::Pin
                                                    })
                                                    .size(px(10.0))
                                                    .text_color(if is_pinned {
                                                        p.accent
                                                    } else {
                                                        p.text_faint
                                                    }),
                                                ),
                                        )
                                    }),
                            ),
                    )
                    .when(is_regular, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_1p5()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .min_w_0()
                                        .flex_1()
                                        .when(is_worktree, |e| {
                                            e.child(
                                                Icon::new(Lucide::GitBranch)
                                                    .size(px(10.0))
                                                    .text_color(p.accent)
                                                    .flex_shrink_0(),
                                            )
                                        })
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(p.text_faint)
                                                .truncate()
                                                .child(subtitle),
                                        ),
                                )
                                .when_some(diff_counts, |e, (add, del)| {
                                    e.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_0p5()
                                            .flex_shrink_0()
                                            .font_family(mono_family.clone())
                                            .text_xs()
                                            .when(add > 0, |c| {
                                                c.child(
                                                    div()
                                                        .text_color(p.success)
                                                        .child(format!("+{add}")),
                                                )
                                            })
                                            .when(del > 0, |c| {
                                                c.child(
                                                    div()
                                                        .text_color(p.danger)
                                                        .child(format!("-{del}")),
                                                )
                                            }),
                                    )
                                }),
                        )
                    }),
            )
            .into_any_element()
    }
}
