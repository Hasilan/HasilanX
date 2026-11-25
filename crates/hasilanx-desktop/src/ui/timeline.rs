use super::HasilanxApp;
use super::backdrop::render_hero_backdrop;
use super::sidebar::agent_mark_icon;
use crate::client::LOCAL_HOST_ID;
use crate::state::ThreadKey;
use crate::theme::agent_brand_colors;
use crate::time_fmt::{agent_display_name, relative_time, shorten_path};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{GlobalState, Icon, Sizable, StyledExt, Theme};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use hasilanx_protocol::{
    ApprovalDecision, CheckResultRecord, CheckRunStatus, ExecutionMode, NoticeLevel,
    PendingApproval, RpcCommand, ThreadStatus, TimelineItem, TimelineItemKind, ToolCallStatus,
    WorkspaceMode,
};
use std::collections::{HashMap, HashSet};

enum TimelineSegment<'a> {
    Single(&'a TimelineItem),
    ActivityGroup {
        group_id: String,
        items: Vec<&'a TimelineItem>,
    },
}

fn group_timeline_segments(items: &[TimelineItem]) -> Vec<TimelineSegment<'_>> {
    let mut out = Vec::new();
    let mut idx = 0;
    while idx < items.len() {
        if matches!(
            items[idx].kind,
            TimelineItemKind::Reasoning { .. } | TimelineItemKind::ToolCall { .. }
        ) {
            let start = idx;
            let mut batch = Vec::new();
            while idx < items.len()
                && matches!(
                    items[idx].kind,
                    TimelineItemKind::Reasoning { .. } | TimelineItemKind::ToolCall { .. }
                )
            {
                batch.push(&items[idx]);
                idx += 1;
            }
            out.push(TimelineSegment::ActivityGroup {
                group_id: format!("act-{}", items[start].id),
                items: batch,
            });
        } else {
            out.push(TimelineSegment::Single(&items[idx]));
            idx += 1;
        }
    }
    out
}

fn summarize_activity_group(items: &[&TimelineItem]) -> String {
    let mut commands = 0usize;
    let mut reads = 0usize;
    let mut edits = 0usize;
    let mut searches = 0usize;
    let mut web = 0usize;
    let mut other_tools = 0usize;
    let mut thoughts = 0usize;

    for item in items {
        match &item.kind {
            TimelineItemKind::Reasoning { .. } => thoughts += 1,
            TimelineItemKind::ToolCall { name, .. } => {
                let lower = name.to_ascii_lowercase();
                if lower.contains("shell")
                    || lower.contains("bash")
                    || lower.contains("command")
                    || lower.contains("exec")
                {
                    commands += 1;
                } else if lower.contains("web")
                    || lower.contains("fetch")
                    || lower.contains("open page")
                {
                    web += 1;
                } else if lower.contains("change")
                    || lower.contains("edit")
                    || lower.contains("write")
                    || lower.contains("patch")
                {
                    edits += 1;
                } else if lower.contains("read") || lower.contains("file") || lower.contains("list") {
                    reads += 1;
                } else if lower.contains("search") || lower.contains("grep") || lower.contains("find")
                {
                    searches += 1;
                } else {
                    other_tools += 1;
                }
            }
            _ => {}
        }
    }

    let mut parts = Vec::new();
    if commands > 0 {
        parts.push(if commands == 1 {
            "Ran 1 command".to_string()
        } else {
            format!("Ran {commands} commands")
        });
    }
    if reads > 0 {
        parts.push(if reads == 1 {
            "read 1 file".to_string()
        } else {
            format!("read {reads} files")
        });
    }
    if edits > 0 {
        parts.push(if edits == 1 {
            "edited 1 file".to_string()
        } else {
            format!("edited {edits} files")
        });
    }
    if web > 0 {
        parts.push(if web == 1 {
            "1 web search".to_string()
        } else {
            format!("{web} web searches")
        });
    }
    if searches > 0 {
        parts.push(if searches == 1 {
            "1 search".to_string()
        } else {
            format!("{searches} searches")
        });
    }
    if other_tools > 0 {
        parts.push(if other_tools == 1 {
            "1 tool".to_string()
        } else {
            format!("{other_tools} tools")
        });
    }
    if parts.is_empty() && thoughts > 0 {
        return if thoughts == 1 {
            "Thought".to_string()
        } else {
            format!("{thoughts} thoughts")
        };
    }
    if thoughts > 0 {
        parts.push("thought".to_string());
    }
    if let Some(first) = parts.first_mut() {
        let mut chs = first.chars();
        if let Some(c) = chs.next() {
            *first = format!("{}{}", c.to_uppercase(), chs.as_str());
        }
    }
    parts.join(" · ")
}

impl HasilanxApp {
    pub(crate) fn toggle_all_tool_calls(&mut self, cx: &mut Context<Self>) {
        let Some((hs, thread)) = self.selected_thread() else {
            return;
        };
        let tool_ids: Vec<String> = hs
            .timeline_items
            .iter()
            .filter(|i| {
                i.thread_id == thread.id && matches!(i.kind, TimelineItemKind::ToolCall { .. })
            })
            .map(|i| i.id.clone())
            .collect();
        if tool_ids.is_empty() {
            return;
        }
        let all_expanded = tool_ids.iter().all(|id| self.expanded_tools.contains(id));
        if all_expanded {
            for id in tool_ids {
                self.expanded_tools.remove(&id);
            }
        } else {
            for id in tool_ids {
                self.expanded_tools.insert(id);
            }
        }
        cx.notify();
    }

    pub(crate) fn step_session_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        let query = self
            .session_search_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        if query.is_empty() {
            return;
        }
        let Some((hs, thread)) = self.selected_thread() else {
            return;
        };
        let items: Vec<TimelineItem> = hs
            .timeline_items
            .iter()
            .filter(|i| i.thread_id == thread.id)
            .cloned()
            .collect();
        let segments = group_timeline_segments(&items);
        let match_count = segments
            .iter()
            .filter(|seg| segment_matches_query(seg, &query))
            .count();
        if match_count == 0 {
            self.session_search_match_idx = 0;
        } else if forward {
            self.session_search_match_idx = (self.session_search_match_idx + 1) % match_count;
        } else {
            self.session_search_match_idx = if self.session_search_match_idx == 0 {
                match_count - 1
            } else {
                self.session_search_match_idx - 1
            };
        }
        cx.notify();
    }

    pub(crate) fn render_stage(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();

        let Some((hs, thread)) = self.selected_thread() else {
            return self.render_hero_empty_state(None, cx);
        };

        let host_id = hs.host_id.clone();
        let thread = thread.clone();
        let project_info = hs
            .projects
            .iter()
            .find(|pr| pr.id == thread.project_id)
            .map(|pr| (pr.name.clone(), pr.root_path.clone()));

        let items: Vec<TimelineItem> = hs
            .timeline_items
            .iter()
            .filter(|i| i.thread_id == thread.id)
            .cloned()
            .collect();
        let is_running = thread.status == ThreadStatus::Running;

        // Empty thread -> Zeron Atmospheric Hero Screen with centered floating composer!
        if items.is_empty() && !is_running {
            return self.render_hero_empty_state(project_info, cx);
        }

        let checkpoint_turns: HashSet<String> = hs
            .turns
            .iter()
            .filter(|tr| tr.thread_id == thread.id && tr.checkpoint_before_ref.is_some())
            .map(|tr| tr.id.clone())
            .collect();
        let turn_durations: HashMap<String, i64> = hs
            .turns
            .iter()
            .filter(|tr| tr.thread_id == thread.id)
            .filter_map(|tr| {
                let end = tr.finished_at?;
                Some((tr.id.clone(), (end - tr.started_at).max(0)))
            })
            .collect();
        let approvals: Vec<PendingApproval> = hs
            .pending_approvals
            .iter()
            .filter(|a| a.thread_id == thread.id)
            .cloned()
            .collect();

        let search_query = if self.show_session_search {
            self.session_search_input
                .read(cx)
                .value()
                .trim()
                .to_lowercase()
        } else {
            String::new()
        };

        let segments = group_timeline_segments(&items);
        let matched_seg_indices: Vec<usize> = if !search_query.is_empty() {
            segments
                .iter()
                .enumerate()
                .filter_map(|(idx, seg)| segment_matches_query(seg, &search_query).then_some(idx))
                .collect()
        } else {
            Vec::new()
        };
        let active_match_seg: Option<usize> = if !matched_seg_indices.is_empty() {
            Some(matched_seg_indices[self.session_search_match_idx % matched_seg_indices.len()])
        } else {
            None
        };

        let timeline_elements: Vec<AnyElement> = segments
            .into_iter()
            .enumerate()
            .map(|(seg_idx, seg)| {
                let is_hit = !search_query.is_empty() && matched_seg_indices.contains(&seg_idx);
                let is_cur_hit = active_match_seg == Some(seg_idx);
                let inner = match seg {
                    TimelineSegment::Single(item) => {
                        let has_cp = item
                            .turn_id
                            .as_ref()
                            .map(|tid| checkpoint_turns.contains(tid))
                            .unwrap_or(false);
                        self.render_timeline_item("", &host_id, item, has_cp, cx)
                    }
                    TimelineSegment::ActivityGroup { group_id, items } => {
                        let dur = items
                            .iter()
                            .find_map(|i| i.turn_id.as_ref())
                            .and_then(|tid| turn_durations.get(tid))
                            .copied();
                        self.render_activity_group("", &group_id, &items, dur, cx)
                    }
                };
                if is_cur_hit {
                    div()
                        .w_full()
                        .pl_2p5()
                        .pr_1p5()
                        .py_1()
                        .rounded_lg()
                        .border_l_2()
                        .border_color(p.accent)
                        .bg(p.accent_soft)
                        .child(inner)
                        .into_any_element()
                } else if is_hit {
                    div()
                        .w_full()
                        .pl_2p5()
                        .border_l_2()
                        .border_color(p.hairline_strong)
                        .child(inner)
                        .into_any_element()
                } else {
                    inner
                }
            })
            .collect();

        let approval_elements: Vec<AnyElement> = approvals
            .iter()
            .map(|appr| self.render_approval_card("", &host_id, appr, cx))
            .collect();

        let composer_el = self.render_composer(false, cx);
        let checks_drawer = self.render_checks_drawer(cx);
        let running_tray = self.render_running_agents_tray(false, cx);
        let show_jump_latest = !self.is_timeline_near_bottom();
        let search_bar = if self.show_session_search {
            Some(self.render_session_search_bar(matched_seg_indices.len(), &search_query, cx))
        } else {
            None
        };

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            // Dismissable error banner
            .when_some(self.last_error_toast.clone(), |el, err| {
                el.child(
                    div()
                        .w_full()
                        .max_w(px(760.0))
                        .mx_auto()
                        .mt_2()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(p.danger_soft)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .text_color(p.danger)
                                .child(Icon::new(Lucide::CircleAlert).size(px(13.0)))
                                .child(err),
                        )
                        .child(
                            Button::new("dismiss-err-toast")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(Lucide::X))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.last_error_toast = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            // Floating Ctrl+F in-session search bar
            .when_some(search_bar, |el, bar| el.child(bar))
            // Scrollable chat transcript
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .id("stage-timeline-scroll")
                            .track_scroll(&self.timeline_scroll)
                            .size_full()
                            .overflow_y_scroll()
                            .px_5()
                            .pt_5()
                            .pb_8()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(760.0))
                                    .mx_auto()
                                    .flex()
                                    .flex_col()
                                    .gap_4()
                                    .children(timeline_elements)
                                    .when(is_running, |el| {
                                        el.child(
                                            div()
                                                .pt_1()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .text_xs()
                                                .text_color(p.text_muted)
                                                .child(
                                                    Spinner::new()
                                                        .xsmall()
                                                        .color(p.running_hsla()),
                                                )
                                                .child(format!(
                                                    "{} is working...",
                                                    agent_display_name(&thread.active_agent_id)
                                                )),
                                        )
                                    }),
                            ),
                    )
                    .vertical_scrollbar(&self.timeline_scroll),
            )
            // Sticky approval banners centered above composer
            .when(!approval_elements.is_empty(), |el| {
                el.child(
                    div()
                        .px_5()
                        .pb_2()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(760.0))
                                .mx_auto()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(approval_elements),
                        ),
                )
            })
            // Subtle floating controls right above composer: Jump to latest + live activity pill
            .when(show_jump_latest || running_tray.is_some(), |el| {
                el.child(
                    div()
                        .w_full()
                        .max_w(px(760.0))
                        .mx_auto()
                        .px_5()
                        .pt_1p5()
                        .pb_1()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .when(show_jump_latest, |row| {
                            row.child(
                                div()
                                    .id("stage-jump-latest-pill")
                                    .px_2p5()
                                    .py_1()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(p.hairline_strong)
                                    .bg(p.surface_elevated)
                                    .shadow_sm()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(p.surface_hover).text_color(p.text))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        GlobalState::suppress_text_selection(cx);
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.timeline_scroll.scroll_to_bottom();
                                        cx.notify();
                                    }))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(p.text_muted)
                                    .child(Icon::new(Lucide::ArrowDown).size(px(11.0)))
                                    .child("Latest"),
                            )
                        })
                        .when_some(running_tray, |row, tray| row.child(tray)),
                )
            })
            // Quality Gates (.hasilanx.toml) live verification drawer
            .when_some(checks_drawer, |el, drawer| el.child(drawer))
            // Docked floating pill composer
            .child(composer_el)
            .into_any_element()
    }

    fn render_checks_drawer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.show_checks_drawer {
            return None;
        }
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();
        let wf = self.active_project_workflow().cloned();
        let results: Vec<CheckResultRecord> = self.selected_thread_checks().to_vec();

        let body: AnyElement = if let Some(cfg) = wf {
            let auto_after = cfg.checks.run_after_turn;
            let mut rows: Vec<AnyElement> = Vec::new();
            // Render configured checks (with latest result if available) + any extra results
            for (idx, spec) in cfg.checks.commands.iter().enumerate() {
                let res_opt = results.iter().find(|r| r.name == spec.name);
                let check_key = format!("chk-{}", spec.name);
                let is_expanded = self.expanded_check_logs.contains(&check_key);
                let name_run = spec.name.clone();
                let key_toggle = check_key.clone();

                let (status_icon, status_text, status_color, is_failed, out_text, res_for_fix) =
                    if let Some(res) = res_opt {
                        let dur = format_elapsed_ms(res.duration_ms as i64);
                        match res.status {
                            CheckRunStatus::Running => (
                                Spinner::new()
                                    .xsmall()
                                    .color(p.running_hsla())
                                    .into_any_element(),
                                "Running...".to_string(),
                                p.running,
                                false,
                                res.output.clone(),
                                None,
                            ),
                            CheckRunStatus::Passed => (
                                Icon::new(Lucide::Check)
                                    .size(px(12.0))
                                    .text_color(p.success)
                                    .into_any_element(),
                                format!("Passed · {dur}"),
                                p.success,
                                false,
                                res.output.clone(),
                                None,
                            ),
                            CheckRunStatus::Failed => (
                                Icon::new(Lucide::CircleX)
                                    .size(px(12.0))
                                    .text_color(p.danger)
                                    .into_any_element(),
                                format!(
                                    "Failed (exit {}) · {dur}",
                                    res.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                                ),
                                p.danger,
                                true,
                                res.output.clone(),
                                Some(res.clone()),
                            ),
                            CheckRunStatus::TimedOut => (
                                Icon::new(Lucide::CircleAlert)
                                    .size(px(12.0))
                                    .text_color(p.approval)
                                    .into_any_element(),
                                format!("Timed out · {dur}"),
                                p.approval,
                                true,
                                res.output.clone(),
                                Some(res.clone()),
                            ),
                        }
                    } else {
                        (
                            div()
                                .w(px(6.0))
                                .h(px(6.0))
                                .rounded_full()
                                .bg(p.text_faint)
                                .into_any_element(),
                            "Not run".to_string(),
                            p.text_faint,
                            false,
                            String::new(),
                            None,
                        )
                    };

                let has_log = !out_text.trim().is_empty();
                let show_log = has_log && (is_expanded || is_failed);
                let out_copy = out_text.clone();
                let copy_id = format!("chk-copy-{idx}");
                let copied = self.is_copied(&copy_id);

                rows.push(
                    div()
                        .w_full()
                        .rounded_lg()
                        .border_1()
                        .border_color(if is_failed { p.danger } else { p.hairline })
                        .bg(p.surface)
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id(SharedString::from(format!("chk-row-{idx}")))
                                .px_2p5()
                                .py_1p5()
                                .cursor_pointer()
                                .hover(|s| s.bg(p.surface_hover))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.expanded_check_logs.contains(&key_toggle) {
                                        this.expanded_check_logs.remove(&key_toggle);
                                    } else {
                                        this.expanded_check_logs.insert(key_toggle.clone());
                                    }
                                    cx.notify();
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
                                        .child(status_icon)
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(p.text)
                                                .flex_shrink_0()
                                                .child(spec.name.clone()),
                                        )
                                        .child({
                                            let cmd_oneline = spec
                                                .command
                                                .split_whitespace()
                                                .collect::<Vec<_>>()
                                                .join(" ");
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_xs()
                                                .font_family(mono_family.clone())
                                                .text_color(p.text_muted)
                                                .whitespace_nowrap()
                                                .truncate()
                                                .child(cmd_oneline)
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .flex_shrink_0()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(status_color)
                                                .child(status_text),
                                        )
                                        .when_some(res_for_fix, |el, failed_rec| {
                                            el.child(
                                                Button::new(SharedString::from(format!(
                                                    "chk-autofix-{idx}"
                                                )))
                                                .xsmall()
                                                .primary()
                                                .icon(Icon::new(Lucide::Sparkles))
                                                .label("Auto-Fix")
                                                .tooltip("Send this failing check & log to AI to fix automatically")
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.auto_fix_failed_check(
                                                        &failed_rec,
                                                        window,
                                                        cx,
                                                    );
                                                })),
                                            )
                                        })
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "chk-rerun-{idx}"
                                            )))
                                            .xsmall()
                                            .ghost()
                                            .icon(Icon::new(Lucide::Play))
                                            .tooltip("Run this check")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.run_selected_project_checks(
                                                    Some(name_run.clone()),
                                                    cx,
                                                );
                                            })),
                                        ),
                                ),
                        )
                        .when(show_log, |el| {
                            let (clipped, _) = clip_with_notice(&out_text, 2400);
                            let md = markdown_code_fence("text", &clipped);
                            let copy_id_click = copy_id.clone();
                            el.child(
                                div()
                                    .border_t_1()
                                    .border_color(p.hairline)
                                    .bg(p.canvas)
                                    .p_2()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.text_faint)
                                                    .child("Output"),
                                            )
                                            .child(
                                                Button::new(SharedString::from(copy_id))
                                                    .xsmall()
                                                    .ghost()
                                                    .icon(Icon::new(if copied {
                                                        Lucide::Check
                                                    } else {
                                                        Lucide::Copy
                                                    }))
                                                    .label(if copied { "Copied" } else { "Copy" })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.copy_with_feedback(
                                                            copy_id_click.clone(),
                                                            out_copy.clone(),
                                                            cx,
                                                        );
                                                    })),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("chk-log-scr-{idx}")))
                                            .max_h(px(150.0))
                                            .overflow_y_scroll()
                                            .text_xs()
                                            .child(
                                                TextView::markdown(
                                                    SharedString::from(format!(
                                                        "chk-log-md-{idx}"
                                                    )),
                                                    md,
                                                )
                                                .selectable(true),
                                            ),
                                    ),
                            )
                        })
                        .into_any_element(),
                );
            }

            div()
                .flex()
                .flex_col()
                .gap_1p5()
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
                                .child(
                                    Icon::new(Lucide::ShieldCheck)
                                        .size(px(13.0))
                                        .text_color(p.accent),
                                )
                                .child(
                                    div()
                                        .font_semibold()
                                        .text_color(p.text)
                                        .child("Quality Gates"),
                                )
                                .child(
                                    div()
                                        .font_family(mono_family.clone())
                                        .text_color(p.text_faint)
                                        .child(".hasilanx.toml"),
                                )
                                .when(auto_after, |e| {
                                    e.child(
                                        div()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded_full()
                                            .bg(p.accent_soft)
                                            .text_xs()
                                            .text_color(p.accent)
                                            .child("Auto after turn"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    Button::new("chk-run-all-btn")
                                        .xsmall()
                                        .ghost()
                                        .icon(Icon::new(Lucide::Play))
                                        .label("Run all")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.run_selected_project_checks(None, cx);
                                        })),
                                )
                                .child(
                                    Button::new("chk-open-cfg-btn")
                                        .xsmall()
                                        .ghost()
                                        .icon(Icon::new(Lucide::FileCode))
                                        .label("Config")
                                        .tooltip("Open .hasilanx.toml in Files inspector")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.open_file_in_inspector(".hasilanx.toml", cx);
                                        })),
                                )
                                .child(
                                    Button::new("chk-reload-cfg-btn")
                                        .xsmall()
                                        .ghost()
                                        .icon(Icon::new(Lucide::RefreshCw))
                                        .tooltip("Reload .hasilanx.toml")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reload_active_project_workflow(cx);
                                        })),
                                )
                                .child(
                                    Button::new("chk-close-drawer-btn")
                                        .xsmall()
                                        .ghost()
                                        .icon(Icon::new(Lucide::X))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.show_checks_drawer = false;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .children(rows)
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .justify_between()
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
                                .child("No .hasilanx.toml in this repository yet"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.text_muted)
                                .child("Auto-generate Quality Gates, Worktree hooks, Agent roles & custom `/` commands."),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new("chk-init-wf-btn")
                                .xsmall()
                                .primary()
                                .icon(Icon::new(Lucide::Sparkles))
                                .label("Generate .hasilanx.toml")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.init_active_project_workflow(cx);
                                })),
                        )
                        .child(
                            Button::new("chk-close-empty-btn")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(Lucide::X))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_checks_drawer = false;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element()
        };

        Some(
            div()
                .w_full()
                .px_5()
                .pb_1()
                .child(
                    div()
                        .w_full()
                        .max_w(px(760.0))
                        .mx_auto()
                        .p_2p5()
                        .rounded_xl()
                        .border_1()
                        .border_color(p.hairline_strong)
                        .bg(p.surface_elevated)
                        .shadow_sm()
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn render_session_search_bar(
        &self,
        match_count: usize,
        query: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let counter = if query.is_empty() {
            "Find in session".to_string()
        } else if match_count == 0 {
            "0 matches".to_string()
        } else {
            let cur = (self.session_search_match_idx % match_count) + 1;
            format!("{cur} / {match_count}")
        };

        div()
            .w_full()
            .px_5()
            .pt_2()
            .child(
                div()
                    .w_full()
                    .max_w(px(760.0))
                    .mx_auto()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .w(px(340.0))
                            .px_2p5()
                            .py_1()
                            .rounded_lg()
                            .border_1()
                            .border_color(p.hairline_strong)
                            .bg(p.surface_elevated)
                            .shadow_md()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Icon::new(Lucide::Search)
                                    .size(px(12.0))
                                    .text_color(p.text_faint),
                            )
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.session_search_input)
                                        .xsmall()
                                        .appearance(false)
                                        .bordered(false),
                                ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if !query.is_empty() && match_count == 0 {
                                        p.danger
                                    } else {
                                        p.text_faint
                                    })
                                    .flex_shrink_0()
                                    .child(counter),
                            )
                            .child(
                                Button::new("search-prev-btn")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::ArrowUp))
                                    .tooltip("Previous match (Shift+Enter)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.step_session_search(false, cx);
                                    })),
                            )
                            .child(
                                Button::new("search-next-btn")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::ArrowDown))
                                    .tooltip("Next match (Enter)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.step_session_search(true, cx);
                                    })),
                            )
                            .child(
                                Button::new("search-close-btn")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::X))
                                    .tooltip("Close search (Esc)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_session_search = false;
                                        this.prompt_input.update(cx, |s, cx| s.focus(window, cx));
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Side-by-side Split Session Arena pane: streams a second agent/worktree session live,
    /// supports inline tool inspection & approvals, one-click "Run last prompt" A/B testing,
    /// and instant Focus swap with the primary pane.
    pub(crate) fn render_split_stage(
        &self,
        split_key: &ThreadKey,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let Some(hs) = self.host_states.get(&split_key.host_id) else {
            return div().into_any_element();
        };
        let Some(thread) = hs.threads.iter().find(|t| t.id == split_key.thread_id) else {
            return div().into_any_element();
        };

        let host_id = hs.host_id.clone();
        let thread = thread.clone();
        let (brand_bg, brand_fg) = agent_brand_colors(&thread.active_agent_id, p.is_dark);
        let brand_icon = agent_mark_icon(&thread.active_agent_id);
        let is_running = thread.status == ThreadStatus::Running;
        let is_approval = thread.status == ThreadStatus::WaitingApproval;

        let items: Vec<TimelineItem> = hs
            .timeline_items
            .iter()
            .filter(|i| i.thread_id == thread.id)
            .cloned()
            .collect();
        let checkpoint_turns: HashSet<String> = hs
            .turns
            .iter()
            .filter(|tr| tr.thread_id == thread.id && tr.checkpoint_before_ref.is_some())
            .map(|tr| tr.id.clone())
            .collect();
        let turn_durations: HashMap<String, i64> = hs
            .turns
            .iter()
            .filter(|tr| tr.thread_id == thread.id)
            .filter_map(|tr| {
                let end = tr.finished_at?;
                Some((tr.id.clone(), (end - tr.started_at).max(0)))
            })
            .collect();
        let approvals: Vec<PendingApproval> = hs
            .pending_approvals
            .iter()
            .filter(|a| a.thread_id == thread.id)
            .cloned()
            .collect();

        // Grab latest user prompt from the primary session so user can A/B compare in 1 click!
        let primary_last_prompt: Option<String> = self.selected_thread().and_then(|(phs, pth)| {
            phs.timeline_items
                .iter()
                .rev()
                .filter(|i| i.thread_id == pth.id)
                .find_map(|i| match &i.kind {
                    TimelineItemKind::UserMessage { text, .. } => Some(text.clone()),
                    _ => None,
                })
        });

        let segments = group_timeline_segments(&items);
        let timeline_elements: Vec<AnyElement> = segments
            .into_iter()
            .map(|seg| match seg {
                TimelineSegment::Single(item) => {
                    let has_cp = item
                        .turn_id
                        .as_ref()
                        .map(|tid| checkpoint_turns.contains(tid))
                        .unwrap_or(false);
                    self.render_timeline_item("split-", &host_id, item, has_cp, cx)
                }
                TimelineSegment::ActivityGroup { group_id, items } => {
                    let dur = items
                        .iter()
                        .find_map(|i| i.turn_id.as_ref())
                        .and_then(|tid| turn_durations.get(tid))
                        .copied();
                    self.render_activity_group("split-", &group_id, &items, dur, cx)
                }
            })
            .collect();

        let approval_elements: Vec<AnyElement> = approvals
            .iter()
            .map(|appr| self.render_approval_card("split-", &host_id, appr, cx))
            .collect();

        let split_hid_send = split_key.host_id.clone();
        let split_tid_send = split_key.thread_id.clone();
        let split_hid_cancel = split_key.host_id.clone();
        let split_tid_cancel = split_key.thread_id.clone();

        div()
            .size_full()
            .bg(p.canvas)
            .border_l_1()
            .border_color(p.hairline)
            .flex()
            .flex_col()
            // Split Session Header
            .child(
                div()
                    .h(px(38.0))
                    .px_3()
                    .border_b_1()
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
                            .flex_1()
                            .child(
                                div()
                                    .w(px(20.0))
                                    .h(px(20.0))
                                    .rounded_md()
                                    .bg(brand_bg)
                                    .text_color(brand_fg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .flex_shrink_0()
                                    .child(Icon::new(brand_icon).size(px(11.0))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .truncate()
                                    .child(thread.title.clone()),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(p.surface_elevated)
                                    .border_1()
                                    .border_color(p.hairline)
                                    .text_xs()
                                    .text_color(p.text_muted)
                                    .flex_shrink_0()
                                    .child(agent_display_name(&thread.active_agent_id)),
                            )
                            .when(thread.workspace_mode == WorkspaceMode::Worktree, |el| {
                                el.child(
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded_full()
                                        .bg(p.accent_soft)
                                        .text_xs()
                                        .text_color(p.accent)
                                        .flex_shrink_0()
                                        .child("Worktree"),
                                )
                            })
                            .when(is_running, |el| {
                                el.child(Spinner::new().xsmall().color(p.running_hsla()))
                            })
                            .when(is_approval, |el| {
                                el.child(
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded_full()
                                        .bg(p.approval_soft)
                                        .text_xs()
                                        .text_color(p.approval)
                                        .flex_shrink_0()
                                        .child("Approval"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                Button::new("split-focus-swap-btn")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::ArrowLeftRight))
                                    .label("Focus")
                                    .tooltip("Swap into primary composer pane")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.focus_split_thread(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("split-close-btn")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::X))
                                    .tooltip("Close split view (Ctrl+\\)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.split_thread = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            // Scrollable transcript
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .id("split-timeline-scroll")
                            .track_scroll(&self.split_timeline_scroll)
                            .size_full()
                            .overflow_y_scroll()
                            .px_4()
                            .pt_4()
                            .pb_6()
                            .child(if timeline_elements.is_empty() && !is_running {
                                div()
                                    .py_12()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(p.text_muted)
                                    .child(
                                        Icon::new(Lucide::Columns2)
                                            .size(px(20.0))
                                            .text_color(p.text_faint),
                                    )
                                    .child(format!(
                                        "{} is ready for side-by-side comparison.",
                                        agent_display_name(&thread.active_agent_id)
                                    ))
                                    .child(
                                        div()
                                            .text_color(p.text_faint)
                                            .child("Click \"Run last prompt\" below or Focus to write a new prompt."),
                                    )
                                    .into_any_element()
                            } else {
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_3p5()
                                    .children(timeline_elements)
                                    .when(is_running, |el| {
                                        el.child(
                                            div()
                                                .pt_1()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .text_xs()
                                                .text_color(p.text_muted)
                                                .child(
                                                    Spinner::new()
                                                        .xsmall()
                                                        .color(p.running_hsla()),
                                                )
                                                .child(format!(
                                                    "{} is working...",
                                                    agent_display_name(&thread.active_agent_id)
                                                )),
                                        )
                                    })
                                    .into_any_element()
                            }),
                    )
                    .vertical_scrollbar(&self.split_timeline_scroll),
            )
            // Inline approvals right inside split pane
            .when(!approval_elements.is_empty(), |el| {
                el.child(
                    div()
                        .px_4()
                        .pb_2()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(approval_elements),
                )
            })
            // Split Action Footer (One-click A/B Arena "Run last prompt" + Stop + Focus)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(p.hairline)
                    .bg(p.surface)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.text_faint)
                            .truncate()
                            .child(if let Some(ref br) = thread.branch_name {
                                format!("Branch: {br}")
                            } else {
                                "Side-by-side live view".to_string()
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .flex_shrink_0()
                            .when(is_running, |el| {
                                el.child(
                                    Button::new("split-stop-btn")
                                        .xsmall()
                                        .danger()
                                        .icon(Icon::new(Lucide::Square))
                                        .label("Stop")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.hosts_mgr.send(
                                                &split_hid_cancel,
                                                RpcCommand::CancelTurn {
                                                    thread_id: split_tid_cancel.clone(),
                                                },
                                            );
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(!is_running, |el| {
                                el.when_some(primary_last_prompt, |e, prompt_to_mirror| {
                                    e.child(
                                        Button::new("split-mirror-prompt-btn")
                                            .xsmall()
                                            .primary()
                                            .icon(Icon::new(Lucide::Sparkles))
                                            .label("Run last prompt")
                                            .tooltip("Send the primary session's last prompt to this agent for A/B comparison")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.hosts_mgr.send(
                                                    &split_hid_send,
                                                    RpcCommand::SendPrompt {
                                                        thread_id: split_tid_send.clone(),
                                                        prompt: prompt_to_mirror.clone(),
                                                        attachments: Vec::new(),
                                                    },
                                                );
                                                this.split_timeline_scroll.scroll_to_bottom();
                                                cx.notify();
                                            })),
                                    )
                                })
                            }),
                    ),
            )
            .into_any_element()
    }

    /// Zeron-inspired atmospheric New Session landing screen with halftone sky backdrop,
    /// centered floating composer, quick prompt starters, and bottom live activity pill.
    fn render_hero_empty_state(
        &self,
        active_project: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let has_projects = self.host_states.values().any(|h| !h.projects.is_empty());
        let active_agent = self.active_agent_id_for_picker("codex");
        let (brand_bg, brand_fg) = agent_brand_colors(&active_agent, p.is_dark);
        let brand_icon = agent_mark_icon(&active_agent);

        let subtitle = if let Some((name, path)) = active_project {
            format!("{} · {}", name, shorten_path(&path))
        } else if let Some((_, _, name)) = self.target_project_for_new_thread() {
            name
        } else {
            "Add a local project folder to begin.".to_string()
        };

        let starters = [
            (
                "Explain codebase",
                "Give me a concise overview of this repository's architecture and main entry points.",
            ),
            (
                "Review current diff",
                "Inspect the current uncommitted git diff and flag any bugs or edge cases.",
            ),
            (
                "Run tests",
                "Run the project's test suite and summarize any failures or warnings.",
            ),
        ];

        let hero_composer = self.render_composer(true, cx);
        let bottom_tray = self.render_running_agents_tray(true, cx);

        let needs_local_backdrop = self.split_thread.is_some()
            && self.split_thread.as_ref() != self.selected.as_ref();

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .when(needs_local_backdrop, |el| {
                el.bg(p.canvas)
                    .child(render_hero_backdrop(self.backdrop_style, p))
            })
            .overflow_hidden()
            // Centered hero composer + quick actions + bottom status pill
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .px_6()
                    .py_6()
                    .child(div().h(px(12.0)))
                    // Center column
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_4()
                            // Brand mark + serene headline
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .w(px(34.0))
                                            .h(px(34.0))
                                            .rounded_xl()
                                            .bg(brand_bg)
                                            .border_1()
                                            .border_color(p.hairline_strong)
                                            .text_color(brand_fg)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(Icon::new(brand_icon).size(px(17.0))),
                                    )
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_semibold()
                                            .text_color(p.text)
                                            .child("What are we building?"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.text_muted)
                                            .child(subtitle),
                                    ),
                            )
                            // Centered floating Zeron Composer
                            .child(hero_composer)
                            // Starter pills or Add Project CTA
                            .child(if !has_projects {
                                Button::new("empty-add-project")
                                    .small()
                                    .primary()
                                    .icon(Icon::new(Lucide::FolderPlus))
                                    .label("Add project folder")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_add_project_modal(
                                            LOCAL_HOST_ID.to_string(),
                                            window,
                                            cx,
                                        );
                                    }))
                                    .into_any_element()
                            } else {
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .justify_center()
                                    .gap_1p5()
                                    .children(starters.into_iter().enumerate().map(
                                        |(i, (title, prompt))| {
                                            let prompt_str = prompt.to_string();
                                            div()
                                                .id(SharedString::from(format!("starter-chip-{i}")))
                                                .px_3()
                                                .py_1()
                                                .rounded_full()
                                                .border_1()
                                                .border_color(p.hairline)
                                                .bg(p.surface)
                                                .hover(|s| {
                                                    s.bg(p.surface_hover).text_color(p.text)
                                                })
                                                .cursor_pointer()
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    GlobalState::suppress_text_selection(cx);
                                                })
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.send_direct_prompt(
                                                        prompt_str.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                }))
                                                .text_xs()
                                                .text_color(p.text_muted)
                                                .child(title)
                                        },
                                    ))
                                    .into_any_element()
                            }),
                    )
                    // Bottom floating Live Agents Pill (just like Zeron app-screenshot.jpg!)
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .min_h(px(30.0))
                            .when_some(bottom_tray, |el, tray| el.child(tray)),
                    ),
            )
            .into_any_element()
    }

    /// Zeron-style floating status pill showing active/waiting sessions across all hosts.
    fn render_running_agents_tray(
        &self,
        always_show_summary: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = self.palette();
        let mut working = 0usize;
        let mut needs_input = 0usize;
        let mut total_sessions = 0usize;
        let mut jump_target: Option<ThreadKey> = None;

        for hid in &self.host_order {
            if let Some(hs) = self.host_states.get(hid) {
                for th in &hs.threads {
                    total_sessions += 1;
                    let is_current = self
                        .selected
                        .as_ref()
                        .is_some_and(|k| k.host_id == hs.host_id && k.thread_id == th.id);
                    if th.status == ThreadStatus::WaitingApproval {
                        needs_input += 1;
                        if !is_current {
                            jump_target = Some(ThreadKey::new(&hs.host_id, &th.id));
                        }
                    } else if th.status == ThreadStatus::Running {
                        working += 1;
                        if jump_target.is_none() && !is_current {
                            jump_target = Some(ThreadKey::new(&hs.host_id, &th.id));
                        }
                    }
                }
            }
        }

        if working == 0 && needs_input == 0 {
            if !always_show_summary || total_sessions == 0 {
                return None;
            }
            let hosts = self.host_states.len();
            let label = if hosts > 1 {
                format!("{total_sessions} sessions · {hosts} hosts")
            } else {
                format!("{total_sessions} sessions · ready")
            };
            return Some(
                div()
                    .id("hero-status-pill")
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .border_1()
                    .border_color(p.hairline)
                    .bg(p.surface_elevated)
                    .shadow_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(p.surface_hover))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        GlobalState::suppress_text_selection(cx);
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_command_palette(window, cx);
                    }))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(p.text_muted)
                    .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(p.success))
                    .child(label)
                    .child(
                        Icon::new(Lucide::ChevronUp)
                            .size(px(11.0))
                            .text_color(p.text_faint),
                    )
                    .into_any_element(),
            );
        }

        let mut parts = Vec::new();
        if working > 0 {
            parts.push(format!("{working} working"));
        }
        if needs_input > 0 {
            parts.push(format!("{needs_input} needs approval"));
        }
        let label = parts.join(" · ");
        let is_urgent = needs_input > 0;

        Some(
            div()
                .id("running-agents-tray-pill")
                .px_3()
                .py_1()
                .rounded_full()
                .border_1()
                .border_color(if is_urgent { p.approval } else { p.hairline_strong })
                .bg(if is_urgent {
                    p.approval_soft
                } else {
                    p.surface_elevated
                })
                .shadow_sm()
                .cursor_pointer()
                .hover(|s| s.bg(p.surface_hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    GlobalState::suppress_text_selection(cx);
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(target) = jump_target.clone() {
                        this.select_thread(target, window, cx);
                    } else {
                        this.open_command_palette(window, cx);
                    }
                }))
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .font_medium()
                .text_color(if is_urgent { p.approval } else { p.text })
                .when(working > 0 && !is_urgent, |el| {
                    el.child(Spinner::new().xsmall().color(p.running_hsla()))
                })
                .when(is_urgent, |el| {
                    el.child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(p.approval))
                })
                .child(label)
                .into_any_element(),
        )
    }

    /// Zeron-style Grouped Activity Rail: summary header + vertical hairline trunk connecting
    /// consecutive thoughts & tool calls cleanly.
    fn render_activity_group(
        &self,
        id_prefix: &str,
        group_id: &str,
        items: &[&TimelineItem],
        elapsed_ms: Option<i64>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let any_running = items.iter().any(|it| {
            matches!(
                it.kind,
                TimelineItemKind::ToolCall {
                    status: ToolCallStatus::Running | ToolCallStatus::Pending,
                    ..
                }
            )
        });
        let any_failed = items.iter().any(|it| {
            matches!(
                it.kind,
                TimelineItemKind::ToolCall {
                    status: ToolCallStatus::Failed,
                    ..
                }
            )
        });

        // Always keep single reasoning or running group open; allow folding multi-item groups cleanly
        let is_collapsed = !any_running && self.collapsed_activity_groups.contains(group_id);
        let summary = summarize_activity_group(items);
        let elapsed_label = elapsed_ms.map(format_elapsed_ms);
        let gid = group_id.to_string();

        // For a single standalone thought with no tool calls, render directly without redundant wrapper
        if items.len() == 1 && matches!(items[0].kind, TimelineItemKind::Reasoning { .. }) {
            return self.render_activity_child_row(id_prefix, items[0], cx);
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            // Quiet Zeron Activity Summary Header ("Ran 2 commands · read 1 file · 4s")
            .child(
                div()
                    .id(SharedString::from(format!("{id_prefix}act-hdr-{group_id}")))
                    .px_1p5()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(p.surface_hover).text_color(p.text))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        GlobalState::suppress_text_selection(cx);
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.collapsed_activity_groups.contains(&gid) {
                            this.collapsed_activity_groups.remove(&gid);
                        } else {
                            this.collapsed_activity_groups.insert(gid.clone());
                        }
                        cx.notify();
                    }))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_xs()
                    .text_color(if any_failed { p.danger } else { p.text_muted })
                    .child(if any_running {
                        Spinner::new()
                            .xsmall()
                            .color(p.running_hsla())
                            .into_any_element()
                    } else {
                        Icon::new(if is_collapsed {
                            Lucide::ChevronRight
                        } else {
                            Lucide::ChevronDown
                        })
                        .size(px(12.0))
                        .text_color(p.text_faint)
                        .into_any_element()
                    })
                    .child(div().font_medium().child(summary))
                    .when_some(elapsed_label, |el, dur| {
                        el.child(div().text_color(p.text_faint).child(format!("· {dur}")))
                    }),
            )
            // Zeron vertical hairline trunk + connected tool/thought rows
            .when(!is_collapsed, |el| {
                el.child(
                    div()
                        .ml(px(9.0))
                        .pl_3()
                        .border_l_1()
                        .border_color(p.tree_line)
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .children(
                            items
                                .iter()
                                .map(|item| self.render_activity_child_row(id_prefix, item, cx)),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_activity_child_row(
        &self,
        id_prefix: &str,
        item: &TimelineItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();

        match &item.kind {
            TimelineItemKind::Reasoning { text } => {
                let is_expanded = self.expanded_reasoning.contains(&item.id);
                let item_id = item.id.clone();
                let line_count = text.lines().count().max(1);

                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "{id_prefix}reason-hdr-{}",
                                item.id
                            )))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.surface_hover).text_color(p.text_muted))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                GlobalState::suppress_text_selection(cx);
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.expanded_reasoning.contains(&item_id) {
                                    this.expanded_reasoning.remove(&item_id);
                                } else {
                                    this.expanded_reasoning.insert(item_id.clone());
                                }
                                cx.notify();
                            }))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_xs()
                            .text_color(p.text_faint)
                            .child(
                                Icon::new(if is_expanded {
                                    Lucide::ChevronDown
                                } else {
                                    Lucide::ChevronRight
                                })
                                .size(px(11.0)),
                            )
                            .child(format!("Thought · {line_count} lines")),
                    )
                    .when(is_expanded, |el| {
                        el.child(
                            div()
                                .ml_2()
                                .pl_3()
                                .py_1()
                                .border_l_2()
                                .border_color(p.hairline_strong)
                                .text_xs()
                                .text_color(p.text_muted)
                                .child(
                                    TextView::markdown(
                                        SharedString::from(format!(
                                            "{id_prefix}reason-md-{}",
                                            item.id
                                        )),
                                        text.clone(),
                                    )
                                    .selectable(true),
                                ),
                        )
                    })
                    .into_any_element()
            }

            TimelineItemKind::ToolCall {
                name,
                status,
                input,
                output,
                ..
            } => {
                let is_expanded = self.expanded_tools.contains(&item.id);
                let item_id = item.id.clone();

                let inline_preview = {
                    let raw_prev = extract_tool_inline_preview(name, input.as_deref());
                    if let Some(root_str) = self.active_workspace_root_str() {
                        let stripped_root = root_str.trim_end_matches('/');
                        let with_slash = format!("{stripped_root}/");
                        if let Some(rel) = raw_prev.strip_prefix(&with_slash) {
                            rel.to_string()
                        } else if let Some(rel) = raw_prev
                            .strip_prefix(&shorten_path(stripped_root))
                            .and_then(|s| s.strip_prefix('/'))
                        {
                            rel.to_string()
                        } else {
                            raw_prev
                        }
                    } else {
                        raw_prev
                    }
                };
                let tool_icon = tool_category_icon(name);
                let openable_file: Option<String> = if id_prefix.is_empty() {
                    self.active_workspace_root_str().and_then(|root_str| {
                        extract_tool_file_path(name, input.as_deref()).filter(|raw| {
                            if raw.starts_with('/') {
                                raw.starts_with(root_str)
                            } else {
                                !raw.starts_with("..")
                            }
                        })
                    })
                } else {
                    None
                };

                let (border_color, icon_color) = match status {
                    ToolCallStatus::Failed => (p.danger, p.danger),
                    ToolCallStatus::Running => (p.hairline_strong, p.running),
                    ToolCallStatus::Pending => (p.hairline, p.text_faint),
                    ToolCallStatus::Completed => (
                        if is_expanded {
                            p.hairline_strong
                        } else {
                            p.hairline
                        },
                        p.text_muted,
                    ),
                };

                let status_hint: Option<(String, Rgba)> = match status {
                    ToolCallStatus::Running => Some(("Running".to_string(), p.running)),
                    ToolCallStatus::Pending => Some(("Pending".to_string(), p.text_faint)),
                    ToolCallStatus::Failed => Some(("Failed".to_string(), p.danger)),
                    ToolCallStatus::Completed => output.as_deref().and_then(|o| {
                        let lines = o.lines().count();
                        (lines > 0).then(|| {
                            if lines == 1 {
                                ("1 line".to_string(), p.text_faint)
                            } else {
                                (format!("{lines} lines"), p.text_faint)
                            }
                        })
                    }),
                };

                let status_el: AnyElement = match status {
                    ToolCallStatus::Pending => div()
                        .w(px(6.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(p.text_faint)
                        .into_any_element(),
                    ToolCallStatus::Running => Spinner::new()
                        .xsmall()
                        .color(p.running_hsla())
                        .into_any_element(),
                    ToolCallStatus::Completed => Icon::new(Lucide::Check)
                        .size(px(12.0))
                        .text_color(p.success)
                        .into_any_element(),
                    ToolCallStatus::Failed => Icon::new(Lucide::CircleX)
                        .size(px(12.0))
                        .text_color(p.danger)
                        .into_any_element(),
                };

                let input_clean = input
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned);
                let output_clean = output
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .map(ToOwned::to_owned);

                div()
                    .w_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(border_color)
                    .bg(p.surface)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "{id_prefix}tool-hdr-{}",
                                item.id
                            )))
                            .w_full()
                            .px_2p5()
                            .py_1p5()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.surface_hover))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                GlobalState::suppress_text_selection(cx);
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.expanded_tools.contains(&item_id) {
                                    this.expanded_tools.remove(&item_id);
                                } else {
                                    this.expanded_tools.insert(item_id.clone());
                                }
                                cx.notify();
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
                                        Icon::new(tool_icon)
                                            .size(px(12.0))
                                            .text_color(icon_color)
                                            .flex_shrink_0(),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(p.text)
                                            .when(!inline_preview.is_empty(), |el| {
                                                el.max_w(px(220.0))
                                            })
                                            .truncate()
                                            .child(name.clone()),
                                    )
                                    .when(!inline_preview.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_xs()
                                                .font_family(mono_family.clone())
                                                .text_color(p.text_muted)
                                                .truncate()
                                                .child(inline_preview),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .flex_shrink_0()
                                    .when_some(openable_file, |el, file_to_open| {
                                        let open_btn_id =
                                            format!("{id_prefix}tool-open-file-{}", item.id);
                                        el.child(
                                            div()
                                                .id(SharedString::from(open_btn_id))
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded_sm()
                                                .bg(p.surface_elevated)
                                                .border_1()
                                                .border_color(p.hairline)
                                                .hover(|s| {
                                                    s.bg(p.surface_selected).text_color(p.text)
                                                })
                                                .cursor_pointer()
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    GlobalState::suppress_text_selection(cx);
                                                    cx.stop_propagation();
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.open_file_in_inspector(&file_to_open, cx);
                                                    cx.stop_propagation();
                                                }))
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .text_xs()
                                                .text_color(p.text_muted)
                                                .child(
                                                    Icon::new(Lucide::ExternalLink).size(px(10.0)),
                                                )
                                                .child("Open"),
                                        )
                                    })
                                    .when_some(status_hint, |el, (hint, color)| {
                                        el.child(div().text_xs().text_color(color).child(hint))
                                    })
                                    .child(status_el)
                                    .child(
                                        Icon::new(if is_expanded {
                                            Lucide::ChevronDown
                                        } else {
                                            Lucide::ChevronRight
                                        })
                                        .size(px(12.0))
                                        .text_color(p.text_faint),
                                    ),
                            ),
                    )
                    .when(is_expanded, |el| {
                        let has_output = output_clean.is_some();
                        let status_copy = *status;
                        el.child(
                            div()
                                .w_full()
                                .border_t_1()
                                .border_color(p.hairline)
                                .bg(p.canvas)
                                .p_2p5()
                                .flex()
                                .flex_col()
                                .gap_2p5()
                                .when_some(input_clean, |e, inp| {
                                    let (clipped, truncated) = clip_with_notice(&inp, 4000);
                                    let lang = detect_code_lang(name, &inp, true);
                                    let md = markdown_code_fence(lang, &clipped);
                                    let inp_copy = inp.clone();
                                    let item_id_cp = item.id.clone();
                                    e.child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_medium()
                                                            .text_color(p.text_faint)
                                                            .child(if truncated {
                                                                "Input (truncated)"
                                                            } else {
                                                                "Input"
                                                            }),
                                                    )
                                                    .child({
                                                        let cp_key = format!(
                                                            "{id_prefix}copy-tool-in-{item_id_cp}"
                                                        );
                                                        let copied = self.is_copied(&cp_key);
                                                        Button::new(SharedString::from(cp_key.clone()))
                                                            .xsmall()
                                                            .ghost()
                                                            .icon(Icon::new(if copied {
                                                                Lucide::Check
                                                            } else {
                                                                Lucide::Copy
                                                            }))
                                                            .label(if copied { "Copied" } else { "Copy" })
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    this.copy_with_feedback(
                                                                        cp_key.clone(),
                                                                        inp_copy.clone(),
                                                                        cx,
                                                                    );
                                                                },
                                                            ))
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "{id_prefix}tool-in-scroll-{}",
                                                        item.id
                                                    )))
                                                    .max_h(px(220.0))
                                                    .overflow_y_scroll()
                                                    .text_xs()
                                                    .child(
                                                        TextView::markdown(
                                                            SharedString::from(format!(
                                                                "{id_prefix}tool-in-{}",
                                                                item.id
                                                            )),
                                                            md,
                                                        )
                                                        .selectable(true),
                                                    ),
                                            ),
                                    )
                                })
                                .when_some(output_clean, |e, out| {
                                    let line_count = out.lines().count();
                                    let (clipped, truncated) = clip_with_notice(&out, 8000);
                                    let lang = detect_code_lang(name, &out, false);
                                    let md = markdown_code_fence(lang, &clipped);
                                    let out_copy = out.clone();
                                    let item_id_cp = item.id.clone();
                                    let label = if truncated {
                                        format!("Output · {line_count} lines (truncated)")
                                    } else {
                                        format!("Output · {line_count} lines")
                                    };
                                    e.child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_medium()
                                                            .text_color(p.text_faint)
                                                            .child(label),
                                                    )
                                                    .child({
                                                        let cp_key = format!(
                                                            "{id_prefix}copy-tool-out-{item_id_cp}"
                                                        );
                                                        let copied = self.is_copied(&cp_key);
                                                        Button::new(SharedString::from(cp_key.clone()))
                                                            .xsmall()
                                                            .ghost()
                                                            .icon(Icon::new(if copied {
                                                                Lucide::Check
                                                            } else {
                                                                Lucide::Copy
                                                            }))
                                                            .label(if copied { "Copied" } else { "Copy" })
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    this.copy_with_feedback(
                                                                        cp_key.clone(),
                                                                        out_copy.clone(),
                                                                        cx,
                                                                    );
                                                                },
                                                            ))
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "{id_prefix}tool-out-scroll-{}",
                                                        item.id
                                                    )))
                                                    .max_h(px(320.0))
                                                    .overflow_y_scroll()
                                                    .text_xs()
                                                    .child(
                                                        TextView::markdown(
                                                            SharedString::from(format!(
                                                                "{id_prefix}tool-out-{}",
                                                                item.id
                                                            )),
                                                            md,
                                                        )
                                                        .selectable(true),
                                                    ),
                                            ),
                                    )
                                })
                                .when(!has_output, |e| {
                                    let (msg, color) = match status_copy {
                                        ToolCallStatus::Running | ToolCallStatus::Pending => {
                                            ("Running — waiting for tool output...", p.text_muted)
                                        }
                                        ToolCallStatus::Failed => {
                                            ("Tool call failed without returning output.", p.danger)
                                        }
                                        ToolCallStatus::Completed => (
                                            "Completed (no output payload returned by agent)",
                                            p.text_faint,
                                        ),
                                    };
                                    e.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .text_xs()
                                            .text_color(color)
                                            .when(
                                                matches!(
                                                    status_copy,
                                                    ToolCallStatus::Running
                                                        | ToolCallStatus::Pending
                                                ),
                                                |row| {
                                                    row.child(
                                                        Spinner::new()
                                                            .xsmall()
                                                            .color(p.running_hsla()),
                                                    )
                                                },
                                            )
                                            .child(msg),
                                    )
                                }),
                        )
                    })
                    .into_any_element()
            }

            _ => div().into_any_element(),
        }
    }

    fn render_timeline_item(
        &self,
        id_prefix: &str,
        host_id: &str,
        item: &TimelineItem,
        has_checkpoint: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();

        match &item.kind {
            // Zeron-style right-aligned rounded user message pill!
            TimelineItemKind::UserMessage { text, attachments } => {
                let turn_id_opt = item.turn_id.clone();
                let hid_rev = host_id.to_string();
                let tid_rev = item.thread_id.clone();
                let rel = relative_time(item.created_at);
                let copy_text = text.clone();
                let resend_text = text.clone();

                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap_1()
                    .child(
                        div()
                            .max_w(px(580.0))
                            .px_4()
                            .py_2p5()
                            .rounded(px(18.0))
                            .border_1()
                            .border_color(p.hairline)
                            .bg(p.user_pill)
                            .text_sm()
                            .text_color(p.text)
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .child(
                                TextView::markdown(
                                    SharedString::from(format!("{id_prefix}usr-md-{}", item.id)),
                                    text.clone(),
                                )
                                .selectable(true),
                            )
                            .when(!attachments.is_empty(), |el| {
                                el.child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_1p5()
                                        .pt_1()
                                        .children(attachments.iter().map(|a| {
                                            div()
                                                .px_2()
                                                .py_0p5()
                                                .rounded_full()
                                                .bg(p.surface_elevated)
                                                .border_1()
                                                .border_color(p.hairline)
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .text_xs()
                                                .text_color(p.text_muted)
                                                .child(Icon::new(Lucide::Paperclip).size(px(11.0)))
                                                .child(a.label.clone())
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .px_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(p.text_faint).child(rel))
                            .child({
                                let copy_fb_id = format!("{id_prefix}copy-usr-{}", item.id);
                                let copied = self.is_copied(&copy_fb_id);
                                let copy_fb_click = copy_fb_id.clone();
                                div()
                                    .id(SharedString::from(copy_fb_id))
                                    .px_1()
                                    .py_0p5()
                                    .rounded_sm()
                                    .hover(|s| s.bg(p.surface_hover).text_color(p.text_muted))
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        GlobalState::suppress_text_selection(cx);
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.copy_with_feedback(
                                            copy_fb_click.clone(),
                                            copy_text.clone(),
                                            cx,
                                        );
                                    }))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(if copied { p.success } else { p.text_faint })
                                    .child(
                                        Icon::new(if copied { Lucide::Check } else { Lucide::Copy })
                                            .size(px(10.0)),
                                    )
                                    .child(if copied { "Copied" } else { "Copy" })
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "{id_prefix}resend-usr-{}",
                                        item.id
                                    )))
                                    .px_1()
                                    .py_0p5()
                                    .rounded_sm()
                                    .hover(|s| s.bg(p.surface_hover).text_color(p.text_muted))
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        GlobalState::suppress_text_selection(cx);
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.prompt_input.update(cx, |s, cx| {
                                            s.set_value(resend_text.clone(), window, cx);
                                            s.focus(window, cx);
                                        });
                                        cx.notify();
                                    }))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child(Icon::new(Lucide::RotateCw).size(px(10.0)))
                                    .child("Reuse"),
                            )
                            .when(has_checkpoint, |el| {
                                if let Some(turn_id) = turn_id_opt {
                                    el.child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "{id_prefix}rev-cp-{turn_id}"
                                            )))
                                            .px_1()
                                            .py_0p5()
                                            .rounded_sm()
                                            .hover(|s| {
                                                s.bg(p.surface_hover).text_color(p.text_muted)
                                            })
                                            .cursor_pointer()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                GlobalState::suppress_text_selection(cx);
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.hosts_mgr.send(
                                                    &hid_rev,
                                                    RpcCommand::RevertToTurn {
                                                        thread_id: tid_rev.clone(),
                                                        turn_id: turn_id.clone(),
                                                        use_before: true,
                                                    },
                                                );
                                                cx.notify();
                                            }))
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_xs()
                                            .text_color(p.text_faint)
                                            .child(Icon::new(Lucide::RotateCcw).size(px(10.0)))
                                            .child("Restore"),
                                    )
                                } else {
                                    el
                                }
                            }),
                    )
                    .into_any_element()
            }

            TimelineItemKind::AssistantMessage { text } => {
                let copy_text = text.clone();
                div()
                    .w_full()
                    .px_1()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .text_sm()
                    .text_color(p.text)
                    .child(
                        TextView::markdown(
                            SharedString::from(format!("{id_prefix}md-{}", item.id)),
                            text.clone(),
                        )
                        .selectable(true),
                    )
                    .child({
                        let copy_fb_id = format!("{id_prefix}copy-asst-{}", item.id);
                        let copied = self.is_copied(&copy_fb_id);
                        let copy_fb_click = copy_fb_id.clone();
                        div().flex().items_center().child(
                            div()
                                .id(SharedString::from(copy_fb_id))
                                .px_1p5()
                                .py_0p5()
                                .rounded_sm()
                                .hover(|s| s.bg(p.surface_hover).text_color(p.text_muted))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.copy_with_feedback(
                                        copy_fb_click.clone(),
                                        copy_text.clone(),
                                        cx,
                                    );
                                }))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(if copied { p.success } else { p.text_faint })
                                .child(
                                    Icon::new(if copied { Lucide::Check } else { Lucide::Copy })
                                        .size(px(10.0)),
                                )
                                .child(if copied { "Copied" } else { "Copy" }),
                        )
                    })
                    .into_any_element()
            }

            TimelineItemKind::Reasoning { .. } | TimelineItemKind::ToolCall { .. } => {
                self.render_activity_child_row(id_prefix, item, cx)
            }

            TimelineItemKind::Plan { text } => {
                let hid_plan = host_id.to_string();
                let tid_plan = item.thread_id.clone();
                div()
                    .w_full()
                    .rounded_xl()
                    .border_1()
                    .border_color(p.hairline_strong)
                    .bg(p.surface)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_3p5()
                            .py_2()
                            .border_b_1()
                            .border_color(p.hairline)
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(p.accent)
                                    .child(Icon::new(Lucide::Layers).size(px(12.0)))
                                    .child("Plan"),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "{id_prefix}approve-plan-{}",
                                    item.id
                                )))
                                .xsmall()
                                .primary()
                                .icon(Icon::new(Lucide::Play))
                                .label("Approve & Execute")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.hosts_mgr.send(
                                        &hid_plan,
                                        RpcCommand::ApprovePlan {
                                            thread_id: tid_plan.clone(),
                                            execution_mode: ExecutionMode::AcceptEdits,
                                        },
                                    );
                                    cx.notify();
                                })),
                            ),
                    )
                    .child(
                        div().p_3p5().text_sm().child(
                            TextView::markdown(
                                SharedString::from(format!("{id_prefix}plan-md-{}", item.id)),
                                text.clone(),
                            )
                            .selectable(true),
                        ),
                    )
                    .into_any_element()
            }

            TimelineItemKind::SystemNotice { level, message } => {
                let (accent, soft, icon) = match level {
                    NoticeLevel::Info => (p.text_muted, p.surface, Lucide::Info),
                    NoticeLevel::Warning => (p.approval, p.approval_soft, Lucide::TriangleAlert),
                    NoticeLevel::Error => (p.danger, p.danger_soft, Lucide::CircleAlert),
                };
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(soft)
                    .flex()
                    .items_start()
                    .gap_2()
                    .text_xs()
                    .text_color(p.text_muted)
                    .child(Icon::new(icon).size(px(13.0)).text_color(accent))
                    .child(
                        div().flex_1().child(
                            TextView::markdown(
                                SharedString::from(format!("{id_prefix}notice-md-{}", item.id)),
                                message.clone(),
                            )
                            .selectable(true),
                        ),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_approval_card(
        &self,
        id_prefix: &str,
        host_id: &str,
        approval: &PendingApproval,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let appr_id = approval.id.clone();

        div()
            .w_full()
            .rounded_xl()
            .border_1()
            .border_color(p.approval)
            .bg(p.surface_elevated)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3p5()
                    .py_2()
                    .bg(p.approval_soft)
                    .border_b_1()
                    .border_color(p.hairline)
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .font_medium()
                    .text_color(p.approval)
                    .child(Icon::new(Lucide::ShieldAlert).size(px(13.0)))
                    .child(format!("Approval required · {}", approval.tool_name)),
            )
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.text)
                            .child(approval.description.clone()),
                    )
                    .when_some(approval.args_json.clone(), |el, args| {
                        let pretty = format_approval_args(&args);
                        let (clipped, _) = clip_with_notice(&pretty, 600);
                        let md = markdown_code_fence("", &clipped);
                        el.child(
                            div().rounded_md().bg(p.canvas).p_2().child(
                                TextView::markdown(
                                    SharedString::from(format!(
                                        "{id_prefix}appr-args-{}",
                                        approval.id
                                    )),
                                    md,
                                )
                                .selectable(true),
                            ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1p5()
                            .children(approval.options.iter().map(|opt| {
                                let hid = host_id.to_string();
                                let aid = appr_id.clone();
                                let decision = opt.decision;
                                let oid = Some(opt.id.clone());
                                let btn = Button::new(SharedString::from(format!(
                                    "{id_prefix}appr-{}-{}",
                                    approval.id, opt.id
                                )))
                                .xsmall()
                                .label(opt.label.clone());
                                let btn = match decision {
                                    ApprovalDecision::Accept => {
                                        btn.primary().icon(Icon::new(Lucide::Check))
                                    }
                                    ApprovalDecision::AcceptForSession => {
                                        btn.outline().icon(Icon::new(Lucide::Shield))
                                    }
                                    ApprovalDecision::Decline | ApprovalDecision::Cancel => {
                                        btn.danger().icon(Icon::new(Lucide::X))
                                    }
                                };
                                btn.on_click(cx.listener(move |this, _, _, cx| {
                                    this.resolve_approval(
                                        hid.clone(),
                                        aid.clone(),
                                        decision,
                                        oid.clone(),
                                        cx,
                                    );
                                }))
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn format_approval_args(raw: &str) -> String {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.to_string();
    };
    if let Some(obj) = val.as_object() {
        if let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) {
            return format!("$ {cmd}");
        }
        if let Some(cmd) = obj.get("cmd").and_then(|v| v.as_str()) {
            return format!("$ {cmd}");
        }
        let lines: Vec<String> = obj
            .iter()
            .filter_map(|(k, v)| match v {
                serde_json::Value::String(s) => Some(format!("{k}: {s}")),
                serde_json::Value::Null => None,
                other => Some(format!("{k}: {other}")),
            })
            .collect();
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }
    raw.to_string()
}

fn tool_category_icon(name: &str) -> Lucide {
    let lower = name.to_ascii_lowercase();
    if lower.contains("shell")
        || lower.contains("bash")
        || lower.contains("command")
        || lower.contains("exec")
        || lower.contains("terminal")
    {
        Lucide::Terminal
    } else if lower.contains("web")
        || lower.contains("fetch")
        || lower.contains("open page")
        || lower.contains("find in page")
        || lower.contains("http")
    {
        Lucide::Globe
    } else if lower.contains("change")
        || lower.contains("edit")
        || lower.contains("patch")
        || lower.contains("write")
        || lower.contains("diff")
    {
        Lucide::FileDiff
    } else if lower.contains("read") || lower.contains("file") || lower.contains("list") {
        Lucide::FileText
    } else if lower.contains("search") || lower.contains("grep") || lower.contains("find") {
        Lucide::Search
    } else if lower.contains("image") {
        Lucide::Image
    } else if lower.contains("sub-agent") || lower.contains("agent") || lower.contains("collab") {
        Lucide::Bot
    } else {
        Lucide::Wrench
    }
}

fn unwrap_shell_wrapper(cmd: &str) -> &str {
    let trimmed = cmd.trim();
    for prefix in [
        "/usr/bin/bash -lc ",
        "/bin/bash -lc ",
        "bash -lc ",
        "/bin/sh -c ",
        "sh -c ",
        "/usr/bin/zsh -lc ",
        "zsh -lc ",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let r = rest.trim();
            if (r.starts_with('\'') && r.ends_with('\'') && r.len() >= 2)
                || (r.starts_with('"') && r.ends_with('"') && r.len() >= 2)
            {
                return &r[1..r.len() - 1];
            }
            return r;
        }
    }
    trimmed
}

fn extract_tool_inline_preview(name: &str, input: Option<&str>) -> String {
    let Some(raw) = input.map(str::trim).filter(|s| !s.is_empty()) else {
        return String::new();
    };

    let candidate = if raw.starts_with('{') || raw.starts_with('[') {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
            extract_json_preview(&val)
        } else {
            raw.lines().next().unwrap_or("").trim().to_string()
        }
    } else {
        let unwrapped = unwrap_shell_wrapper(raw);
        let first = unwrapped.lines().next().unwrap_or("").trim();
        let extra_lines = unwrapped.lines().count().saturating_sub(1);
        if extra_lines > 0 {
            format!("{first} (+{extra_lines} lines)")
        } else {
            first.to_string()
        }
    };

    let clean = candidate.trim();
    if clean.is_empty() {
        return String::new();
    }
    if name.contains(clean) {
        return String::new();
    }
    let shortened = if clean.starts_with('/') {
        shorten_path(clean)
    } else {
        clean.to_string()
    };
    if shortened.chars().count() > 72 {
        let truncated: String = shortened.chars().take(71).collect();
        format!("{truncated}…")
    } else {
        shortened
    }
}

fn extract_json_preview(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::Object(map) => {
            const PRIORITY_KEYS: &[&str] = &[
                "command",
                "CommandLine",
                "cmd",
                "query",
                "Query",
                "url",
                "Url",
                "path",
                "Path",
                "file_path",
                "filePath",
                "AbsolutePath",
                "TargetFile",
                "pattern",
                "Pattern",
                "prompt",
                "Prompt",
                "name",
            ];
            for key in PRIORITY_KEYS {
                if let Some(s) = map.get(*key).and_then(|v| v.as_str()) {
                    let t = unwrap_shell_wrapper(s);
                    if !t.is_empty() {
                        return t.lines().next().unwrap_or("").trim().to_string();
                    }
                }
            }
            let mut parts = Vec::new();
            for (k, v) in map.iter().take(3) {
                match v {
                    serde_json::Value::String(s) if !s.trim().is_empty() => {
                        let first = s.lines().next().unwrap_or("").trim();
                        parts.push(format!("{k}={first}"));
                    }
                    serde_json::Value::Number(n) => parts.push(format!("{k}={n}")),
                    serde_json::Value::Bool(b) => parts.push(format!("{k}={b}")),
                    _ => {}
                }
            }
            parts.join(" ")
        }
        serde_json::Value::Array(arr) => {
            if let Some(first) = arr.first() {
                let p = extract_json_preview(first);
                if arr.len() > 1 && !p.is_empty() {
                    format!("{p} (+{})", arr.len() - 1)
                } else {
                    p
                }
            } else {
                String::new()
            }
        }
        serde_json::Value::String(s) => s.lines().next().unwrap_or("").trim().to_string(),
        other => other.to_string(),
    }
}

fn detect_code_lang(tool_name: &str, text: &str, is_input: bool) -> &'static str {
    let trimmed = text.trim();
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            return "json";
        }
    }
    if trimmed.starts_with("--- ")
        || trimmed.starts_with("@@ ")
        || trimmed.starts_with("diff --git ")
    {
        return "diff";
    }
    if is_input {
        let lower = tool_name.to_ascii_lowercase();
        if lower.contains("shell") || lower.contains("bash") || lower.contains("command") {
            return "bash";
        }
    }
    ""
}

fn clip_with_notice(text: &str, max_chars: usize) -> (String, bool) {
    let total = text.chars().count();
    if total <= max_chars {
        (text.to_string(), false)
    } else {
        let mut clipped: String = text.chars().take(max_chars).collect();
        clipped.push_str("\n… (truncated — use Copy for full content)");
        (clipped, true)
    }
}

fn markdown_code_fence(lang: &str, content: &str) -> String {
    let mut max_run = 0usize;
    let mut current = 0usize;
    for ch in content.chars() {
        if ch == '`' {
            current += 1;
            if current > max_run {
                max_run = current;
            }
        } else {
            current = 0;
        }
    }
    let fence_len = (max_run + 1).max(3);
    let fence: String = std::iter::repeat_n('`', fence_len).collect();
    if lang.is_empty() {
        format!("{fence}\n{content}\n{fence}")
    } else {
        format!("{fence}{lang}\n{content}\n{fence}")
    }
}

fn format_elapsed_ms(ms: i64) -> String {
    if ms < 1_000 {
        if ms <= 0 {
            "<1s".to_string()
        } else {
            format!("{ms}ms")
        }
    } else if ms < 10_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else if ms < 60_000 {
        format!("{}s", ms / 1000)
    } else {
        let total_secs = ms / 1000;
        let mins = total_secs / 60;
        let rem = total_secs % 60;
        if rem == 0 {
            format!("{mins}m")
        } else {
            format!("{mins}m {rem}s")
        }
    }
}

fn segment_matches_query(seg: &TimelineSegment<'_>, q_lower: &str) -> bool {
    match seg {
        TimelineSegment::Single(item) => timeline_item_matches_query(item, q_lower),
        TimelineSegment::ActivityGroup { items, .. } => {
            items.iter().any(|it| timeline_item_matches_query(it, q_lower))
        }
    }
}

fn timeline_item_matches_query(item: &TimelineItem, q_lower: &str) -> bool {
    match &item.kind {
        TimelineItemKind::UserMessage { text, attachments } => {
            text.to_lowercase().contains(q_lower)
                || attachments
                    .iter()
                    .any(|a| a.label.to_lowercase().contains(q_lower))
        }
        TimelineItemKind::AssistantMessage { text }
        | TimelineItemKind::Reasoning { text }
        | TimelineItemKind::Plan { text } => text.to_lowercase().contains(q_lower),
        TimelineItemKind::ToolCall {
            name,
            input,
            output,
            ..
        } => {
            name.to_lowercase().contains(q_lower)
                || input
                    .as_deref()
                    .map(|s| s.to_lowercase().contains(q_lower))
                    .unwrap_or(false)
                || output
                    .as_deref()
                    .map(|s| s.to_lowercase().contains(q_lower))
                    .unwrap_or(false)
        }
        TimelineItemKind::SystemNotice { message, .. } => message.to_lowercase().contains(q_lower),
    }
}

pub(crate) fn extract_tool_file_path(name: &str, input: Option<&str>) -> Option<String> {
    if let Some(raw) = input.map(str::trim).filter(|s| !s.is_empty()) {
        // 1. JSON tool payload keys
        if raw.starts_with('{') {
            if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) {
                for key in [
                    "path",
                    "Path",
                    "file_path",
                    "filePath",
                    "AbsolutePath",
                    "TargetFile",
                ] {
                    if let Some(s) = map.get(key).and_then(|v| v.as_str()).map(str::trim) {
                        if !s.is_empty() && !s.contains('\n') {
                            return Some(s.to_string());
                        }
                    }
                }
            }
        }
        // 2. Shell wrappers / file-change summaries ("update src/main.rs", "head -n 20 Cargo.toml")
        let first_line = unwrap_shell_wrapper(raw).lines().next().unwrap_or("").trim();
        for prefix in ["update ", "add ", "delete ", "modify "] {
            if let Some(rest) = first_line.strip_prefix(prefix) {
                let p = rest.split(" -> ").last().unwrap_or(rest).trim();
                if looks_like_file_path(p) {
                    return Some(p.to_string());
                }
            }
        }
        let tokens: Vec<&str> = first_line.split_whitespace().collect();
        if let Some(&cmd) = tokens.first() {
            if matches!(cmd, "cat" | "head" | "tail" | "sed" | "nl" | "bat" | "less") {
                for tok in tokens.iter().skip(1).rev() {
                    let clean = tok.trim_matches(|c| c == '\'' || c == '"');
                    if !clean.starts_with('-') && looks_like_file_path(clean) {
                        return Some(clean.to_string());
                    }
                }
            }
        }
        if !first_line.contains(' ') && looks_like_file_path(first_line) {
            return Some(first_line.to_string());
        }
    }

    // 3. Fallback: tool title like "File Cargo.toml" or "Read src/main.rs"
    for tok in name.split_whitespace().rev() {
        let clean = tok.trim_matches(|c| matches!(c, '\'' | '"' | '`' | ',' | ':'));
        if looks_like_file_path(clean) {
            return Some(clean.to_string());
        }
    }
    None
}

fn looks_like_file_path(s: &str) -> bool {
    if s.is_empty() || s.starts_with('-') || s.starts_with("http://") || s.starts_with("https://") {
        return false;
    }
    let has_slash = s.contains('/');
    let has_ext = s
        .rsplit('/')
        .next()
        .is_some_and(|base| base.contains('.') && !base.starts_with('.') && !base.ends_with('.'));
    has_slash || has_ext
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn extracts_tool_file_paths_cleanly() {
        assert_eq!(
            extract_tool_file_path("Read File", Some("{\"Path\": \"/repo/Cargo.toml\"}")),
            Some("/repo/Cargo.toml".to_string())
        );
        assert_eq!(
            extract_tool_file_path("File Change", Some("update crates/app/src/main.rs")),
            Some("crates/app/src/main.rs".to_string())
        );
        assert_eq!(
            extract_tool_file_path(
                "Shell Command",
                Some("/usr/bin/bash -lc 'head -n 40 Cargo.toml'")
            ),
            Some("Cargo.toml".to_string())
        );
        assert_eq!(
            extract_tool_file_path("Web Search", Some("{\"query\": \"rust gpui\"}")),
            None
        );
    }

    #[::core::prelude::v1::test]
    fn formats_elapsed_durations_cleanly() {
        assert_eq!(format_elapsed_ms(0), "<1s");
        assert_eq!(format_elapsed_ms(450), "450ms");
        assert_eq!(format_elapsed_ms(2_400), "2.4s");
        assert_eq!(format_elapsed_ms(14_000), "14s");
        assert_eq!(format_elapsed_ms(60_000), "1m");
        assert_eq!(format_elapsed_ms(125_000), "2m 5s");
    }

    #[::core::prelude::v1::test]
    fn inline_preview_handles_pretty_json_and_shell_wrappers() {
        let agy_json = "{\n  \"query\": \"名古屋 今日の天気 2026年11月22日\"\n}";
        assert_eq!(
            extract_tool_inline_preview("\"名古屋 今日の天気 2026年11月22日\" on Web", Some(agy_json)),
            ""
        );
        assert_eq!(
            extract_tool_inline_preview("Web Search", Some(agy_json)),
            "名古屋 今日の天気 2026年11月22日"
        );
        assert_eq!(
            extract_tool_inline_preview(
                "Read File",
                Some("/usr/bin/bash -lc 'head -n 5 Cargo.toml'")
            ),
            "head -n 5 Cargo.toml"
        );
        let read_json = "{\n  \"Path\": \"/tmp/foo/Cargo.toml\"\n}";
        assert_eq!(
            extract_tool_inline_preview("File Cargo.toml", Some(read_json)),
            "/tmp/foo/Cargo.toml"
        );
    }

    #[::core::prelude::v1::test]
    fn markdown_fence_escapes_embedded_backticks() {
        let body = "before\n```rust\nfn main() {}\n```\nafter";
        let fenced = markdown_code_fence("text", body);
        assert!(fenced.starts_with("````text\n"));
        assert!(fenced.ends_with("\n````"));
    }

    #[::core::prelude::v1::test]
    fn summarizes_activity_groups_cleanly() {
        let items = vec![
            TimelineItem {
                id: "1".into(),
                thread_id: "t".into(),
                turn_id: None,
                kind: TimelineItemKind::ToolCall {
                    call_id: "c1".into(),
                    name: "Shell Command".into(),
                    status: ToolCallStatus::Completed,
                    input: None,
                    output: None,
                },
                created_at: 1,
                updated_at: 1,
            },
            TimelineItem {
                id: "2".into(),
                thread_id: "t".into(),
                turn_id: None,
                kind: TimelineItemKind::ToolCall {
                    call_id: "c2".into(),
                    name: "Read File".into(),
                    status: ToolCallStatus::Completed,
                    input: None,
                    output: None,
                },
                created_at: 2,
                updated_at: 2,
            },
            TimelineItem {
                id: "3".into(),
                thread_id: "t".into(),
                turn_id: None,
                kind: TimelineItemKind::ToolCall {
                    call_id: "c3".into(),
                    name: "Web Search".into(),
                    status: ToolCallStatus::Completed,
                    input: None,
                    output: None,
                },
                created_at: 3,
                updated_at: 3,
            },
        ];
        let refs: Vec<&TimelineItem> = items.iter().collect();
        assert_eq!(
            summarize_activity_group(&refs),
            "Ran 1 command · read 1 file · 1 web search"
        );
    }
}
