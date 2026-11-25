use super::{ActiveModal, HasilanxApp, InspectorTab, status_color};
use crate::client::LOCAL_HOST_ID;
use crate::state::ThreadKey;
use crate::time_fmt::{agent_display_name, shorten_path};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::GlobalState;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::diff::{Diff, DiffMode};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{Icon, Sizable, StyledExt, Theme};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use hasilanx_protocol::{RpcCommand, ThreadStatus};

#[derive(Debug, Clone)]
enum PaletteEntry {
    Session {
        key: ThreadKey,
        title: String,
        meta: String,
        status: ThreadStatus,
    },
    Command {
        id: &'static str,
        label: &'static str,
        icon: Lucide,
        shortcut: &'static str,
    },
}

impl HasilanxApp {
    pub(crate) fn render_diff_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();
        let Some(key) = self.selected.clone() else {
            return div()
                .size_full()
                .rounded_xl()
                .border_1()
                .border_color(p.hairline)
                .bg(p.sidebar)
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(p.text_faint)
                .child("Select a session to inspect changes & files.")
                .into_any_element();
        };

        let empty_diff = hasilanx_protocol::GitDiffSnapshot::default();
        let diff = self.selected_diff_snapshot().unwrap_or(&empty_diff);
        let has_parsed_files = !self.diff_state.read(cx).files().is_empty();
        let total_add: u32 = diff.files.iter().map(|f| f.additions).sum();
        let total_del: u32 = diff.files.iter().map(|f| f.deletions).sum();
        let active_filter = self.selected_diff_file.clone();
        let tab = self.inspector_tab;

        let hid_ref = key.host_id.clone();
        let tid_ref = key.thread_id.clone();
        let hid_push = key.host_id.clone();
        let tid_push = key.thread_id.clone();
        let hid_pr = key.host_id.clone();
        let tid_pr = key.thread_id.clone();

        let changes_tab_active = tab == InspectorTab::Changes;
        let files_tab_active = tab == InspectorTab::Files;

        let header = div()
            .h(px(42.0))
            .px_2p5()
            .border_b_1()
            .border_color(p.hairline)
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            // Zeron tabbed inspector strip (Changes | Files)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .id("insp-tab-changes")
                            .h(px(26.0))
                            .px_2p5()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(if changes_tab_active {
                                p.surface_selected
                            } else {
                                rgba(0x00000000)
                            })
                            .hover(|s| {
                                if changes_tab_active {
                                    s.bg(p.surface_selected)
                                } else {
                                    s.bg(p.surface_hover)
                                }
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                GlobalState::suppress_text_selection(cx);
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inspector_tab = InspectorTab::Changes;
                                this.sync_diff_state(cx);
                                cx.notify();
                            }))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_xs()
                            .when(changes_tab_active, |el| el.font_medium())
                            .text_color(if changes_tab_active {
                                p.text
                            } else {
                                p.text_muted
                            })
                            .child(Icon::new(Lucide::FileDiff).size(px(12.0)))
                            .child("Changes")
                            .when(!diff.files.is_empty(), |el| {
                                el.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_0p5()
                                        .font_family(mono_family.clone())
                                        .text_xs()
                                        .child(
                                            div()
                                                .text_color(p.success)
                                                .child(format!("+{total_add}")),
                                        )
                                        .child(
                                            div()
                                                .text_color(p.danger)
                                                .child(format!("-{total_del}")),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("insp-tab-files")
                            .h(px(26.0))
                            .px_2p5()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(if files_tab_active {
                                p.surface_selected
                            } else {
                                rgba(0x00000000)
                            })
                            .hover(|s| {
                                if files_tab_active {
                                    s.bg(p.surface_selected)
                                } else {
                                    s.bg(p.surface_hover)
                                }
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                GlobalState::suppress_text_selection(cx);
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inspector_tab = InspectorTab::Files;
                                this.sync_files_inspector(false);
                                cx.notify();
                            }))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_xs()
                            .when(files_tab_active, |el| el.font_medium())
                            .text_color(if files_tab_active {
                                p.text
                            } else {
                                p.text_muted
                            })
                            .child(Icon::new(Lucide::FileCode).size(px(12.0)))
                            .child("Files"),
                    ),
            )
            // Right tab actions
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .flex_shrink_0()
                    .when(changes_tab_active, |el| {
                        let has_files = !diff.files.is_empty();
                        el.when(has_files, |e| {
                            e.child(
                                Button::new("diff-ask-ai-review")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::Sparkles))
                                    .label("Review")
                                    .tooltip("Ask AI to review current git diff")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send_direct_prompt(
                                            "Review the current uncommitted git diff carefully. Flag any bugs, regressions, edge cases, or missing tests.".to_string(),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                        })
                        .child(
                            Button::new("diff-mode-toggle")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(match self.diff_mode {
                                    DiffMode::Split => Lucide::Rows2,
                                    _ => Lucide::Columns2,
                                }))
                                .tooltip("Switch Unified / Split")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_diff_mode(cx);
                                })),
                        )
                        .child(
                            Button::new("diff-refresh-btn")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(Lucide::RefreshCw))
                                .tooltip("Refresh Diff")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.hosts_mgr.send(
                                        &hid_ref,
                                        RpcCommand::GetThreadDiff {
                                            thread_id: tid_ref.clone(),
                                        },
                                    );
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("diff-close-btn")
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(Lucide::X))
                            .tooltip("Close Inspector (Ctrl+Shift+D)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_diff_pane = false;
                                cx.notify();
                            })),
                    ),
            );

        if files_tab_active {
            return div()
                .size_full()
                .rounded_xl()
                .border_1()
                .border_color(p.hairline)
                .bg(p.sidebar)
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(header)
                .child(self.render_files_tab(cx))
                .into_any_element();
        }

        div()
            .size_full()
            .rounded_xl()
            .border_1()
            .border_color(p.hairline)
            .bg(p.sidebar)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(header)
            // File filter strip (hugs content tightly; scrolls only when many files changed)
            .when(!diff.files.is_empty(), |el| {
                let all_active = active_filter.is_none();
                let file_count = diff.files.len();
                let chips_el = div()
                    .px_2p5()
                    .py_1p5()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id("diff-filter-all")
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(if all_active {
                                p.surface_selected
                            } else {
                                rgba(0x00000000)
                            })
                            .hover(|s| s.bg(p.surface_hover))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected_diff_file = None;
                                this.sync_diff_state(cx);
                                cx.notify();
                            }))
                            .text_xs()
                            .text_color(if all_active { p.text } else { p.text_muted })
                            .child(format!("All ({file_count})")),
                    )
                    .children(diff.files.iter().enumerate().map(|(idx, f)| {
                        let fpath = f.path.clone();
                        let is_active = active_filter.as_deref() == Some(fpath.as_str());
                        div()
                            .id(SharedString::from(format!("diff-flt-file-{idx}")))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(if is_active {
                                p.surface_selected
                            } else {
                                rgba(0x00000000)
                            })
                            .hover(|s| s.bg(p.surface_hover))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.selected_diff_file.as_deref() == Some(fpath.as_str()) {
                                    this.selected_diff_file = None;
                                } else {
                                    this.selected_diff_file = Some(fpath.clone());
                                }
                                this.sync_diff_state(cx);
                                cx.notify();
                            }))
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .child(
                                div()
                                    .font_family(mono_family.clone())
                                    .text_color(p.approval)
                                    .child(f.kind.code()),
                            )
                            .child(
                                div()
                                    .text_color(if is_active { p.text } else { p.text_muted })
                                    .child(f.path.clone()),
                            )
                            .child(
                                div()
                                    .font_family(mono_family.clone())
                                    .text_color(p.success)
                                    .child(format!("+{}", f.additions)),
                            )
                            .child(
                                div()
                                    .font_family(mono_family.clone())
                                    .text_color(p.danger)
                                    .child(format!("-{}", f.deletions)),
                            )
                    }))
                    .when_some(active_filter.clone(), |el, open_path| {
                        el.child(
                            Button::new("diff-open-in-files")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(Lucide::ExternalLink))
                                .label("Open file")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_file_in_inspector(&open_path, cx);
                                })),
                        )
                    });

                el.child(if file_count > 6 {
                    div()
                        .h(px(84.0))
                        .border_b_1()
                        .border_color(p.hairline)
                        .overflow_y_scrollbar()
                        .child(chips_el)
                        .into_any_element()
                } else {
                    div()
                        .border_b_1()
                        .border_color(p.hairline)
                        .child(chips_el)
                        .into_any_element()
                })
            })
            // Pierre virtualized Diff viewer
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .bg(p.canvas)
                    .child(if has_parsed_files {
                        Diff::new(&self.diff_state)
                            .size_full()
                            .into_any_element()
                    } else if !diff.unified_diff.trim().is_empty() {
                        let lines: Vec<AnyElement> = diff
                            .unified_diff
                            .lines()
                            .take(400)
                            .map(|line| {
                                let (col, bg) =
                                    if line.starts_with('+') && !line.starts_with("+++") {
                                        (p.success, p.success_soft)
                                    } else if line.starts_with('-') && !line.starts_with("---") {
                                        (p.danger, p.danger_soft)
                                    } else if line.starts_with("@@") {
                                        (p.running, p.running_soft)
                                    } else {
                                        (p.text_muted, rgba(0x00000000))
                                    };
                                div()
                                    .px_3()
                                    .bg(bg)
                                    .text_xs()
                                    .font_family(mono_family.clone())
                                    .text_color(col)
                                    .child(line.to_string())
                                    .into_any_element()
                            })
                            .collect();
                        div()
                            .size_full()
                            .py_2()
                            .flex()
                            .flex_col()
                            .overflow_y_scrollbar()
                            .children(lines)
                            .into_any_element()
                    } else {
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .child(
                                Icon::new(Lucide::CircleCheck)
                                    .size(px(20.0))
                                    .text_color(p.text_faint),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child(if diff.is_git_repo {
                                        "Working tree is clean."
                                    } else {
                                        "Not a Git repository."
                                    }),
                            )
                            .into_any_element()
                    }),
            )
            // Bottom Git Commit / Push / PR footer
            .child(
                div()
                    .p_2p5()
                    .border_t_1()
                    .border_color(p.hairline)
                    .bg(p.sidebar)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Input::new(&self.commit_msg_input).xsmall())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Button::new("git-commit-btn")
                                    .xsmall()
                                    .primary()
                                    .icon(Icon::new(Lucide::GitCommitHorizontal))
                                    .label("Commit")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_git_commit(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("git-push-btn")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::Upload))
                                    .label("Push")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.hosts_mgr.send(
                                            &hid_push,
                                            RpcCommand::GitPush {
                                                thread_id: tid_push.clone(),
                                            },
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("git-pr-btn")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::GitPullRequest))
                                    .label("PR")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let title = this
                                            .commit_msg_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_string();
                                        let title = if title.is_empty() {
                                            "Updates from HasilanX".to_string()
                                        } else {
                                            title
                                        };
                                        this.hosts_mgr.send(
                                            &hid_pr,
                                            RpcCommand::GitCreatePr {
                                                thread_id: tid_pr.clone(),
                                                title,
                                                body: "Created via HasilanX".to_string(),
                                            },
                                        );
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_active_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.active_modal.clone() {
            ActiveModal::None => None,
            ActiveModal::CommandPalette => Some(self.render_command_palette_modal(cx)),
            ActiveModal::Settings => Some(self.render_settings_modal(cx)),
            ActiveModal::AddProjectPath(host_id) => {
                Some(self.render_add_project_path_modal(host_id, cx))
            }
        }
    }

    pub(crate) fn execute_selected_palette_item(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.palette_input.read(cx).value().trim().to_lowercase();
        let entries = self.collect_palette_entries(&query);
        if entries.is_empty() {
            return;
        }
        let idx = self.palette_selected_idx.min(entries.len() - 1);
        let chosen = entries[idx].clone();
        self.execute_palette_entry(chosen, window, cx);
    }

    fn collect_palette_entries(&self, query: &str) -> Vec<PaletteEntry> {
        let commands: &[(&'static str, &'static str, Lucide, &'static str)] = &[
            ("new-session", "New session", Lucide::Plus, "Ctrl+N"),
            (
                "parallel",
                "Parallel compare in isolated worktree",
                Lucide::GitFork,
                "",
            ),
            (
                "split",
                "Toggle side-by-side Split view",
                Lucide::Columns2,
                "Ctrl+\\",
            ),
            (
                "run-checks",
                "Run project checks (.hasilanx.toml)",
                Lucide::Play,
                "",
            ),
            (
                "init-workflow",
                "Create / open .hasilanx.toml workflow",
                Lucide::FileCode,
                "",
            ),
            (
                "reload-workflow",
                "Reload .hasilanx.toml",
                Lucide::RefreshCw,
                "",
            ),
            ("search", "Find in current session", Lucide::Search, "Ctrl+F"),
            ("diff", "Toggle Changes (Git diff)", Lucide::FileDiff, "Ctrl+Shift+D"),
            ("files", "Toggle Files explorer", Lucide::FileCode, "Ctrl+Shift+F"),
            (
                "terminal",
                "Toggle integrated Terminal",
                Lucide::SquareTerminal,
                "Ctrl+Shift+J",
            ),
            ("tools", "Expand / collapse all tools", Lucide::Wrench, "Ctrl+Shift+O"),
            ("add-project", "Add project folder", Lucide::FolderPlus, ""),
            ("theme", "Switch theme", Lucide::Palette, "Ctrl+Shift+T"),
            ("settings", "Open Settings", Lucide::Settings, "Ctrl+,"),
        ];

        let mut out = Vec::new();
        for &(id, label, icon, shortcut) in commands {
            if query.is_empty()
                || label.to_lowercase().contains(query)
                || id.contains(query)
                || shortcut.to_lowercase().contains(query)
            {
                out.push(PaletteEntry::Command {
                    id,
                    label,
                    icon,
                    shortcut,
                });
            }
        }

        let mut sessions = Vec::new();
        for hid in &self.host_order {
            if let Some(hs) = self.host_states.get(hid) {
                for t in &hs.threads {
                    let pname = hs
                        .projects
                        .iter()
                        .find(|pr| pr.id == t.project_id)
                        .map(|pr| pr.name.clone())
                        .unwrap_or_default();
                    if query.is_empty()
                        || t.title.to_lowercase().contains(query)
                        || pname.to_lowercase().contains(query)
                        || hs.label.to_lowercase().contains(query)
                        || t.active_agent_id.to_lowercase().contains(query)
                    {
                        sessions.push((
                            t.updated_at,
                            PaletteEntry::Session {
                                key: ThreadKey::new(hid, &t.id),
                                title: t.title.clone(),
                                meta: format!(
                                    "{} · {}",
                                    pname,
                                    agent_display_name(&t.active_agent_id)
                                ),
                                status: t.status,
                            },
                        ));
                    }
                }
            }
        }
        sessions.sort_by(|a, b| b.0.cmp(&a.0));
        // When user typed a query, prioritize matching sessions first if query is non-empty,
        // or keep concise commands + sessions cleanly separated.
        if !query.is_empty() {
            let mut merged: Vec<PaletteEntry> = sessions.into_iter().take(18).map(|(_, e)| e).collect();
            merged.extend(out);
            merged
        } else {
            // Default view: recent sessions first (fastest switching), then commands
            let mut merged: Vec<PaletteEntry> = sessions.into_iter().take(12).map(|(_, e)| e).collect();
            merged.extend(out);
            merged
        }
    }

    fn execute_palette_entry(
        &mut self,
        entry: PaletteEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match entry {
            PaletteEntry::Session { key, .. } => {
                self.active_modal = ActiveModal::None;
                self.select_thread(key, window, cx);
            }
            PaletteEntry::Command { id, .. } => match id {
                "new-session" => {
                    self.active_modal = ActiveModal::None;
                    if let Some((hid, pid, _)) = self.target_project_for_new_thread() {
                        self.create_thread(hid, pid, Some(window), cx);
                    } else {
                        self.open_add_project_modal(LOCAL_HOST_ID.to_string(), window, cx);
                    }
                }
                "parallel" => {
                    self.active_modal = ActiveModal::None;
                    self.start_parallel_worktree_compare(cx);
                }
                "split" => {
                    self.active_modal = ActiveModal::None;
                    self.toggle_split_pane(cx);
                }
                "run-checks" => {
                    self.active_modal = ActiveModal::None;
                    if self.active_project_workflow().is_some() {
                        self.run_selected_project_checks(None, cx);
                    } else {
                        self.show_checks_drawer = true;
                        cx.notify();
                    }
                }
                "init-workflow" => {
                    self.active_modal = ActiveModal::None;
                    if self.active_project_workflow().is_some() {
                        self.open_file_in_inspector(".hasilanx.toml", cx);
                    } else {
                        self.init_active_project_workflow(cx);
                    }
                }
                "reload-workflow" => {
                    self.active_modal = ActiveModal::None;
                    self.reload_active_project_workflow(cx);
                }
                "search" => {
                    self.active_modal = ActiveModal::None;
                    self.toggle_session_search(window, cx);
                }
                "diff" => {
                    self.active_modal = ActiveModal::None;
                    self.toggle_diff_pane(cx);
                }
                "files" => {
                    self.active_modal = ActiveModal::None;
                    self.toggle_files_pane(cx);
                }
                "terminal" => {
                    self.active_modal = ActiveModal::None;
                    self.toggle_terminal_dock(Some(window), cx);
                }
                "tools" => {
                    self.active_modal = ActiveModal::None;
                    self.toggle_all_tool_calls(cx);
                }
                "add-project" => {
                    self.open_add_project_modal(LOCAL_HOST_ID.to_string(), window, cx);
                }
                "theme" => {
                    let next = self.theme_preset.next();
                    self.set_theme(next, cx);
                }
                "settings" => {
                    self.active_modal = ActiveModal::Settings;
                    cx.notify();
                }
                _ => {}
            },
        }
    }

    fn render_command_palette_modal(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();
        let query = self.palette_input.read(cx).value().trim().to_lowercase();
        let entries = self.collect_palette_entries(&query);
        let total = entries.len();
        let sel_idx = if total == 0 {
            0
        } else {
            self.palette_selected_idx.min(total - 1)
        };

        div()
            .id("modal-overlay-palette")
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(64.0))
            .px_4()
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                let q = this.palette_input.read(cx).value().trim().to_lowercase();
                let count = this.collect_palette_entries(&q).len();
                if count == 0 {
                    return;
                }
                let k = ev.keystroke.key.as_str();
                if k == "down" || (ev.keystroke.modifiers.control && k == "n") {
                    this.palette_selected_idx = (this.palette_selected_idx + 1) % count;
                    cx.notify();
                    cx.stop_propagation();
                } else if k == "up" || (ev.keystroke.modifiers.control && k == "p") {
                    this.palette_selected_idx = if this.palette_selected_idx == 0 {
                        count - 1
                    } else {
                        this.palette_selected_idx - 1
                    };
                    cx.notify();
                    cx.stop_propagation();
                } else if k == "enter" {
                    this.execute_selected_palette_item(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id("modal-backdrop-palette")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(p.overlay_backdrop)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        GlobalState::suppress_text_selection(cx);
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.active_modal = ActiveModal::None;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("modal-card-palette")
                    .relative()
                    .occlude()
                    .w(px(560.0))
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
                            .px_3p5()
                            .py_2p5()
                            .border_b_1()
                            .border_color(p.hairline)
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(Lucide::Search).size(px(14.0)).text_color(p.text_faint))
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.palette_input)
                                        .appearance(false)
                                        .bordered(false),
                                ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child("↑↓ · ↵ · ESC"),
                            ),
                    )
                    // Unified keyboard-navigable sessions + commands list
                    .child(
                        div()
                            .max_h(px(380.0))
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .overflow_y_scrollbar()
                            .children(if entries.is_empty() {
                                vec![
                                    div()
                                        .py_6()
                                        .flex()
                                        .justify_center()
                                        .text_xs()
                                        .text_color(p.text_faint)
                                        .child("No matching sessions or commands.")
                                        .into_any_element(),
                                ]
                            } else {
                                entries
                                    .into_iter()
                                    .enumerate()
                                    .map(|(idx, entry)| {
                                        let is_sel = idx == sel_idx;
                                        let entry_exec = entry.clone();
                                        match entry {
                                            PaletteEntry::Session {
                                                key,
                                                title,
                                                meta,
                                                status,
                                            } => {
                                                let key_split = key.clone();
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "pal-row-{idx}"
                                                    )))
                                                    .px_2p5()
                                                    .py_1p5()
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .bg(if is_sel {
                                                        p.surface_selected
                                                    } else {
                                                        rgba(0x00000000)
                                                    })
                                                    .when(is_sel, |el| {
                                                        el.border_l_2().border_color(p.accent)
                                                    })
                                                    .hover(|s| s.bg(p.surface_hover))
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.execute_palette_entry(
                                                                entry_exec.clone(),
                                                                window,
                                                                cx,
                                                            );
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
                                                            .gap_2()
                                                            .min_w_0()
                                                            .flex_1()
                                                            .child(
                                                                div()
                                                                    .w(px(6.0))
                                                                    .h(px(6.0))
                                                                    .rounded_full()
                                                                    .flex_shrink_0()
                                                                    .bg(status_color(&p, status)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .font_medium()
                                                                    .text_color(p.text)
                                                                    .truncate()
                                                                    .child(title),
                                                            ),
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
                                                                    .text_color(p.text_faint)
                                                                    .child(meta),
                                                            )
                                                            .child(
                                                                div()
                                                                    .id(SharedString::from(
                                                                        format!("pal-spl-{idx}"),
                                                                    ))
                                                                    .p_1()
                                                                    .rounded_sm()
                                                                    .hover(|s| {
                                                                        s.bg(p.surface_elevated)
                                                                            .text_color(p.accent)
                                                                    })
                                                                    .on_mouse_down(
                                                                        MouseButton::Left,
                                                                        |_, _, cx| {
                                                                            GlobalState::suppress_text_selection(cx);
                                                                            cx.stop_propagation();
                                                                        },
                                                                    )
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            this.active_modal =
                                                                                ActiveModal::None;
                                                                            this.open_in_split_pane(
                                                                                key_split.clone(),
                                                                                cx,
                                                                            );
                                                                            cx.stop_propagation();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(Lucide::Columns2)
                                                                            .size(px(11.0))
                                                                            .text_color(
                                                                                p.text_faint,
                                                                            ),
                                                                    ),
                                                            ),
                                                    )
                                                    .into_any_element()
                                            }
                                            PaletteEntry::Command {
                                                label,
                                                icon,
                                                shortcut,
                                                ..
                                            } => div()
                                                .id(SharedString::from(format!("pal-row-{idx}")))
                                                .px_2p5()
                                                .py_1p5()
                                                .rounded_md()
                                                .cursor_pointer()
                                                .bg(if is_sel {
                                                    p.surface_selected
                                                } else {
                                                    rgba(0x00000000)
                                                })
                                                .when(is_sel, |el| {
                                                    el.border_l_2().border_color(p.accent)
                                                })
                                                .hover(|s| s.bg(p.surface_hover))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.execute_palette_entry(
                                                            entry_exec.clone(),
                                                            window,
                                                            cx,
                                                        );
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
                                                        .gap_2()
                                                        .min_w_0()
                                                        .child(
                                                            Icon::new(icon)
                                                                .size(px(12.0))
                                                                .text_color(if is_sel {
                                                                    p.accent
                                                                } else {
                                                                    p.text_muted
                                                                }),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(p.text)
                                                                .truncate()
                                                                .child(label),
                                                        ),
                                                )
                                                .when(!shortcut.is_empty(), |el| {
                                                    el.child(
                                                        div()
                                                            .px_1p5()
                                                            .py_0p5()
                                                            .rounded_sm()
                                                            .bg(p.surface)
                                                            .border_1()
                                                            .border_color(p.hairline)
                                                            .text_xs()
                                                            .font_family(mono_family.clone())
                                                            .text_color(p.text_faint)
                                                            .child(shortcut),
                                                    )
                                                })
                                                .into_any_element(),
                                        }
                                    })
                                    .collect()
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_add_project_path_modal(&self, host_id: String, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let is_local = host_id == LOCAL_HOST_ID;
        let hid_sub = host_id.clone();

        let suggestions: Vec<(String, String)> = if is_local {
            dirs::home_dir()
                .map(|h| h.join("Projects"))
                .and_then(|proj_dir| std::fs::read_dir(proj_dir).ok())
                .map(|entries| {
                    let mut list: Vec<(String, String)> = entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .map(|e| {
                            let full = e.path().to_string_lossy().to_string();
                            let name = e.file_name().to_string_lossy().to_string();
                            (name, full)
                        })
                        .collect();
                    list.sort_by(|a, b| a.0.cmp(&b.0));
                    list
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        div()
            .id("modal-overlay-projpath")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .child(
                div()
                    .id("modal-backdrop-projpath")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(p.overlay_backdrop)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        GlobalState::suppress_text_selection(cx);
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.active_modal = ActiveModal::None;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("modal-card-projpath")
                    .relative()
                    .occlude()
                    .w(px(480.0))
                    .rounded_xl()
                    .border_1()
                    .border_color(p.hairline_strong)
                    .bg(p.surface_elevated)
                    .shadow_lg()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(p.text)
                                    .child("Add project folder"),
                            )
                            .child(
                                Button::new("close-proj-modal")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::X))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_modal = ActiveModal::None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(Input::new(&self.project_path_input).small())
                    .when(!suggestions.is_empty(), |el| {
                        let hid_chip = host_id.clone();
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(p.text_faint)
                                        .child("~/Projects"),
                                )
                                .child(
                                    div()
                                        .max_h(px(160.0))
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .overflow_y_scrollbar()
                                        .children(suggestions.into_iter().enumerate().map(
                                            |(idx, (name, full_path))| {
                                                let hid_c = hid_chip.clone();
                                                let short = shorten_path(&full_path);
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "sug-proj-{idx}"
                                                    )))
                                                    .px_2()
                                                    .py_1p5()
                                                    .rounded_md()
                                                    .hover(|s| s.bg(p.surface_hover))
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.hosts_mgr.send(
                                                                &hid_c,
                                                                RpcCommand::CreateProject {
                                                                    name: None,
                                                                    root_path: full_path.clone(),
                                                                },
                                                            );
                                                            this.active_modal = ActiveModal::None;
                                                            cx.notify();
                                                        },
                                                    ))
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap_2()
                                                    .text_xs()
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .text_color(p.text)
                                                            .child(
                                                                Icon::new(Lucide::Folder)
                                                                    .size(px(12.0))
                                                                    .text_color(p.text_muted),
                                                            )
                                                            .child(name),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_color(p.text_faint)
                                                            .truncate()
                                                            .child(short),
                                                    )
                                            },
                                        )),
                                ),
                        )
                    })
                    .child(
                        div()
                            .pt_1()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(if is_local {
                                Button::new("browse-os-proj-btn")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::FolderOpen))
                                    .label("Browse...")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pick_local_project_folder(cx);
                                    }))
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        Button::new("cancel-proj-modal")
                                            .xsmall()
                                            .ghost()
                                            .label("Cancel")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.active_modal = ActiveModal::None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("save-proj-modal")
                                            .xsmall()
                                            .primary()
                                            .label("Add")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.submit_project_path_modal(
                                                    hid_sub.clone(),
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
