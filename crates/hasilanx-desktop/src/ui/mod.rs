mod backdrop;
mod composer;
mod files_view;
mod panels;
mod settings;
mod sidebar;
mod terminal_view;
mod timeline;

use crate::client::{ClientUpdateKind, LOCAL_HOST_ID, MultiHostManager};
use crate::notify::send_notification;
use crate::state::{HostWorkspaceState, ThreadKey};
use crate::theme::{BackdropStyle, SidebarDensity, ThemePreset, UiPalette, UiPreferences};
use base64::Engine as _;
use files_view::FilesInspectorState;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::GlobalState;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::diff::{DiffFile, DiffMode, DiffState};
use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    Disableable, Icon, ResizableState, Sizable, StyledExt, Theme, h_resizable, resizable_panel,
    v_resizable,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use hasilanx_protocol::{
    ApprovalDecision, Attachment, CheckResultRecord, CheckRunStatus, DomainEvent, ExecutionMode,
    GitDiffSnapshot, ProjectWorkflowConfig, RpcCommand, ThreadRecord, ThreadStatus,
    TurnCompletionStatus, WorkspaceMode,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use terminal_view::{TerminalImeEvent, TerminalImeState, TerminalLayoutSnapshot};

gpui_kit::actions!(
    hasilanx,
    [
        ToggleCommandPalette,
        ToggleDiffPane,
        ToggleFilesPane,
        ToggleTerminalDock,
        ToggleSidebar,
        ToggleAllTools,
        ToggleSessionSearch,
        ToggleSplitPane,
        NavigateBack,
        NavigateForward,
        NewThreadAction,
        SendPromptAction,
        ComposerAcceptAutocomplete,
        ComposerPrevAutocomplete,
        ComposerNextAutocomplete,
        OpenSettings,
        CloseOverlay,
        CycleTheme,
        QuickJump1,
        QuickJump2,
        QuickJump3,
        QuickJump4,
        QuickJump5,
        QuickJump6,
        QuickJump7,
        QuickJump8,
        QuickJump9
    ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarFilter {
    All,
    NeedsAttention,
    Working,
    Pinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InspectorTab {
    Changes,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    Appearance,
    InputSession,
    Agents,
    Workflow,
    Daemons,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActiveModal {
    None,
    CommandPalette,
    Settings,
    AddProjectPath(String),
}

pub struct HasilanxApp {
    pub(crate) hosts_mgr: MultiHostManager,
    pub(crate) host_order: Vec<String>,
    pub(crate) host_states: HashMap<String, HostWorkspaceState>,
    pub(crate) selected: Option<ThreadKey>,
    pub(crate) split_thread: Option<ThreadKey>,
    pub(crate) pending_split_for_new_thread: bool,
    pub(crate) draft_agent_id: String,
    pub(crate) draft_model: Option<String>,
    pub(crate) draft_thinking: Option<String>,
    pub(crate) draft_mode: ExecutionMode,
    pub(crate) pending_auto_prompt: Option<(String, Vec<Attachment>)>,
    pub(crate) editing_thread_title: bool,
    pub(crate) selected_diff_file: Option<String>,
    pub(crate) theme_preset: ThemePreset,
    pub(crate) backdrop_style: BackdropStyle,
    pub(crate) sidebar_density: SidebarDensity,
    pub(crate) sidebar_filter: SidebarFilter,
    pub(crate) inspector_tab: InspectorTab,
    pub(crate) send_on_enter: bool,
    pub(crate) new_thread_workspace_mode: WorkspaceMode,
    pub(crate) show_sidebar: bool,
    pub(crate) show_diff_pane: bool,
    pub(crate) show_terminal_dock: bool,
    pub(crate) show_session_search: bool,
    pub(crate) show_checks_drawer: bool,
    pub(crate) expanded_check_logs: HashSet<String>,
    pub(crate) session_search_match_idx: usize,
    pub(crate) palette_selected_idx: usize,
    pub(crate) composer_popup_idx: usize,
    pub(crate) active_modal: ActiveModal,
    pub(crate) settings_tab: SettingsTab,
    pub(crate) settings_notice: Option<(bool, String)>,
    pub(crate) preferred_agent_id: Option<String>,
    pub(crate) expanded_reasoning: HashSet<String>,
    pub(crate) expanded_tools: HashSet<String>,
    pub(crate) collapsed_activity_groups: HashSet<String>,
    pub(crate) collapsed_projects: HashSet<String>,
    pub(crate) pinned_threads: HashSet<String>,
    pub(crate) nav_history: Vec<ThreadKey>,
    pub(crate) nav_index: usize,
    pub(crate) files_inspector: FilesInspectorState,
    pub(crate) pending_attachments: Vec<Attachment>,
    pub(crate) last_error_toast: Option<String>,
    pub(crate) copied_feedback: Option<(String, Instant)>,
    pub(crate) diff_mode: DiffMode,
    pub(crate) diff_state: Entity<DiffState>,
    diff_synced_sig: Option<(String, String, usize, usize, Option<String>)>,
    // Resizable panel states, focus & scroll handles
    h_resize_state: Entity<ResizableState>,
    v_resize_state: Entity<ResizableState>,
    split_resize_state: Entity<ResizableState>,
    pub(crate) terminal_focus: FocusHandle,
    pub(crate) terminal_ime: Entity<TerminalImeState>,
    pub(crate) terminal_layout: Rc<RefCell<TerminalLayoutSnapshot>>,
    pub(crate) terminal_selecting: bool,
    pub(crate) terminal_font_size: f32,
    pub(crate) terminal_maximized: bool,
    pub(crate) terminal_copied_until: Option<Instant>,
    pub(crate) terminal_hovered_url: Option<(usize, usize, usize, String)>,
    pub(crate) timeline_scroll: ScrollHandle,
    pub(crate) split_timeline_scroll: ScrollHandle,
    // Inputs
    pub(crate) project_path_input: Entity<InputState>,
    pub(crate) thread_title_input: Entity<InputState>,
    pub(crate) prompt_input: Entity<TextareaState>,
    pub(crate) model_input: Entity<InputState>,
    pub(crate) commit_msg_input: Entity<InputState>,
    pub(crate) remote_name_input: Entity<InputState>,
    pub(crate) remote_url_input: Entity<InputState>,
    pub(crate) remote_token_input: Entity<InputState>,
    pub(crate) palette_input: Entity<InputState>,
    pub(crate) files_filter_input: Entity<InputState>,
    pub(crate) session_search_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    _poll_task: Task<()>,
}

fn prompt_placeholder(send_on_enter: bool) -> &'static str {
    if send_on_enter {
        "Do anything... (@file, /cmd · Enter to send)"
    } else {
        "Do anything... (@file to mention, / for commands)"
    }
}

impl HasilanxApp {
    pub fn new(
        hosts_mgr: MultiHostManager,
        initial_hosts: Vec<(String, String, bool)>,
        prefs: UiPreferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut host_order = Vec::new();
        let mut host_states = HashMap::new();
        for (id, label, is_local) in initial_hosts {
            host_order.push(id.clone());
            host_states.insert(id.clone(), HostWorkspaceState::new(id, label, is_local));
        }

        let send_on_enter = prefs.send_on_enter;
        let h_resize_state = cx.new(|_| ResizableState::default());
        let v_resize_state = cx.new(|_| ResizableState::default());
        let split_resize_state = cx.new(|_| ResizableState::default());
        let diff_state = cx.new(|cx| DiffState::new(Vec::<DiffFile>::new(), cx));
        let terminal_focus = cx.focus_handle();
        let terminal_ime = cx.new(|_| TerminalImeState::new());

        let project_path_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("/home/user/Projects/my-repo")
        });
        let thread_title_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Thread title...")
        });
        let prompt_input = cx.new(|cx| {
            let s = TextareaState::new(window, cx)
                .placeholder(prompt_placeholder(send_on_enter))
                .submit_on_enter(send_on_enter)
                .auto_grow(2, 8);
            s.focus(window, cx);
            s
        });
        let model_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filter or custom model ID...")
        });
        let commit_msg_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Commit message or PR title..."));
        let remote_name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("gpu-workstation"));
        let remote_url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("http://100.64.0.12:4820")
        });
        let remote_token_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Bearer token from daemon.toml")
        });
        let palette_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search sessions or run command...")
        });
        let files_filter_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filter workspace files...")
        });
        let session_search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Find in session...")
        });

        let sub_prompt = cx.subscribe_in(
            &prompt_input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.composer_popup_idx = 0;
                    let text = this.prompt_input.read(cx).value().to_string();
                    if matches!(
                        composer::detect_composer_trigger(&text),
                        Some(composer::ComposerTrigger::Mention { .. })
                    ) {
                        this.sync_files_inspector(false);
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { secondary, shift } => {
                    let should_send = *secondary || (this.send_on_enter && !*shift);
                    if should_send {
                        this.send_or_queue_prompt(false, window, cx);
                    }
                }
                _ => {}
            },
        );
        let sub_proj = cx.subscribe_in(
            &project_path_input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => {
                    if let ActiveModal::AddProjectPath(host_id) = this.active_modal.clone() {
                        this.submit_project_path_modal(host_id, window, cx);
                    }
                }
                _ => {}
            },
        );
        let sub_title = cx.subscribe_in(
            &thread_title_input,
            window,
            |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_thread_title_edit(cx);
                }
            },
        );
        let sub_model = cx.subscribe_in(
            &model_input,
            window,
            |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => {
                    this.apply_custom_model_input(cx);
                }
                _ => {}
            },
        );
        let sub_commit = cx.subscribe_in(
            &commit_msg_input,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.submit_git_commit(window, cx);
                }
            },
        );
        let sub_palette = cx.subscribe_in(
            &palette_input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.palette_selected_idx = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    this.execute_selected_palette_item(window, cx);
                }
                _ => {}
            },
        );
        let sub_files_filter = cx.subscribe_in(
            &files_filter_input,
            window,
            |_, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            },
        );
        let sub_session_search = cx.subscribe_in(
            &session_search_input,
            window,
            |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => {
                    this.session_search_match_idx = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { shift, .. } => {
                    this.step_session_search(!*shift, cx);
                }
                _ => {}
            },
        );
        let sub_remote_token = cx.subscribe_in(
            &remote_token_input,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.add_remote_host(window, cx);
                }
            },
        );
        let sub_term_ime = cx.subscribe(
            &terminal_ime,
            |this, _, ev: &TerminalImeEvent, cx| match ev {
                TerminalImeEvent::Commit(text) => {
                    if !text.is_empty() {
                        if let Some(buf) = this.active_terminal_mut() {
                            buf.scroll_to_bottom();
                            buf.clear_selection();
                        }
                        this.send_terminal_bytes(text.as_bytes(), cx);
                    }
                }
                TerminalImeEvent::PreeditChanged => {
                    cx.notify();
                }
            },
        );

        let mgr_poll = hosts_mgr.clone();
        let poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(32))
                    .await;
                if !mgr_poll.has_pending_updates() {
                    if this.upgrade().is_none() {
                        break;
                    }
                    continue;
                }
                let keep_going = this
                    .update(cx, |app, cx| {
                        app.drain_host_updates(cx);
                    })
                    .is_ok();
                if !keep_going {
                    break;
                }
            }
        });

        let preferred_agent_id = prefs.default_agent_id.clone();
        let initial_agent_id = preferred_agent_id
            .clone()
            .unwrap_or_else(|| "codex".to_string());
        let initial_term_font = prefs.terminal_font_size.clamp(10.0, 20.0);

        Self {
            hosts_mgr,
            host_order,
            host_states,
            selected: None,
            split_thread: None,
            pending_split_for_new_thread: false,
            draft_agent_id: initial_agent_id,
            draft_model: None,
            draft_thinking: None,
            draft_mode: prefs.default_execution_mode,
            pending_auto_prompt: None,
            editing_thread_title: false,
            selected_diff_file: None,
            theme_preset: prefs.theme,
            backdrop_style: prefs.backdrop,
            sidebar_density: prefs.sidebar_density,
            sidebar_filter: SidebarFilter::All,
            inspector_tab: InspectorTab::Changes,
            send_on_enter,
            new_thread_workspace_mode: prefs.default_workspace_mode,
            show_sidebar: true,
            show_diff_pane: false,
            show_terminal_dock: false,
            show_session_search: false,
            show_checks_drawer: false,
            expanded_check_logs: HashSet::new(),
            session_search_match_idx: 0,
            palette_selected_idx: 0,
            composer_popup_idx: 0,
            active_modal: ActiveModal::None,
            settings_tab: SettingsTab::Appearance,
            settings_notice: None,
            preferred_agent_id,
            expanded_reasoning: HashSet::new(),
            expanded_tools: HashSet::new(),
            collapsed_activity_groups: HashSet::new(),
            collapsed_projects: HashSet::new(),
            pinned_threads: prefs.pinned_threads.into_iter().collect(),
            nav_history: Vec::new(),
            nav_index: 0,
            files_inspector: FilesInspectorState::default(),
            pending_attachments: Vec::new(),
            last_error_toast: None,
            copied_feedback: None,
            diff_mode: DiffMode::Unified,
            diff_state,
            diff_synced_sig: None,
            h_resize_state,
            v_resize_state,
            split_resize_state,
            terminal_focus,
            terminal_ime,
            terminal_layout: Rc::new(RefCell::new(TerminalLayoutSnapshot::default())),
            terminal_selecting: false,
            terminal_font_size: initial_term_font,
            terminal_maximized: false,
            terminal_copied_until: None,
            terminal_hovered_url: None,
            timeline_scroll: ScrollHandle::new(),
            split_timeline_scroll: ScrollHandle::new(),
            project_path_input,
            thread_title_input,
            prompt_input,
            model_input,
            commit_msg_input,
            remote_name_input,
            remote_url_input,
            remote_token_input,
            palette_input,
            files_filter_input,
            session_search_input,
            _subscriptions: vec![
                sub_prompt,
                sub_proj,
                sub_title,
                sub_model,
                sub_commit,
                sub_palette,
                sub_files_filter,
                sub_session_search,
                sub_remote_token,
                sub_term_ime,
            ],
            _poll_task: poll_task,
        }
    }

    pub(crate) fn palette(&self) -> UiPalette {
        self.theme_preset.palette()
    }

    pub(crate) fn set_theme(&mut self, preset: ThemePreset, cx: &mut Context<Self>) {
        self.theme_preset = preset;
        preset.apply(cx);
        self.persist_prefs();
        cx.notify();
    }

    pub(crate) fn set_send_on_enter(
        &mut self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_on_enter = enabled;
        self.prompt_input.update(cx, |s, cx| {
            s.set_submit_on_enter(enabled, cx);
            s.set_placeholder(prompt_placeholder(enabled), window, cx);
        });
        self.persist_prefs();
        cx.notify();
    }

    pub(crate) fn set_backdrop_style(&mut self, style: BackdropStyle, cx: &mut Context<Self>) {
        self.backdrop_style = style;
        self.persist_prefs();
        cx.notify();
    }

    pub(crate) fn set_sidebar_density(
        &mut self,
        density: SidebarDensity,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_density = density;
        self.persist_prefs();
        cx.notify();
    }

    pub(crate) fn toggle_sidebar_density(&mut self, cx: &mut Context<Self>) {
        let next = match self.sidebar_density {
            SidebarDensity::Regular => SidebarDensity::Compact,
            SidebarDensity::Compact => SidebarDensity::Regular,
        };
        self.set_sidebar_density(next, cx);
    }

    pub(crate) fn toggle_pin_thread(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        if self.pinned_threads.contains(thread_id) {
            self.pinned_threads.remove(thread_id);
        } else {
            self.pinned_threads.insert(thread_id.to_string());
        }
        self.persist_prefs();
        cx.notify();
    }

    pub(crate) fn set_terminal_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.terminal_font_size = size.clamp(10.0, 20.0);
        self.persist_prefs();
        cx.notify();
    }

    pub(crate) fn set_default_workspace_mode(
        &mut self,
        mode: WorkspaceMode,
        cx: &mut Context<Self>,
    ) {
        self.new_thread_workspace_mode = mode;
        self.persist_prefs();
        cx.notify();
    }

    pub(crate) fn set_default_execution_mode(
        &mut self,
        mode: ExecutionMode,
        cx: &mut Context<Self>,
    ) {
        self.draft_mode = mode;
        self.persist_prefs();
        cx.notify();
    }

    pub(crate) fn set_default_agent_pref(&mut self, agent_id: String, cx: &mut Context<Self>) {
        self.preferred_agent_id = Some(agent_id.clone());
        self.draft_agent_id = agent_id;
        self.draft_model = None;
        self.draft_thinking = None;
        self.persist_prefs();
        cx.notify();
    }

    fn persist_prefs(&self) {
        let mut pinned: Vec<String> = self.pinned_threads.iter().cloned().collect();
        pinned.sort();
        let prefs = UiPreferences {
            theme: self.theme_preset,
            send_on_enter: self.send_on_enter,
            backdrop: self.backdrop_style,
            sidebar_density: self.sidebar_density,
            pinned_threads: pinned,
            terminal_font_size: self.terminal_font_size,
            default_workspace_mode: self.new_thread_workspace_mode,
            default_execution_mode: self.draft_mode,
            default_agent_id: self.preferred_agent_id.clone(),
        };
        prefs.save();
    }

    pub(crate) fn copy_with_feedback(
        &mut self,
        feedback_id: impl Into<String>,
        text: String,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let id = feedback_id.into();
        let expires = Instant::now() + Duration::from_millis(1400);
        self.copied_feedback = Some((id.clone(), expires));
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(Duration::from_millis(1450))
                .await;
            let _ = this.update(cx, |app, cx| {
                if let Some((ref cur_id, until)) = app.copied_feedback {
                    if cur_id == &id && Instant::now() >= until {
                        app.copied_feedback = None;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(crate) fn is_copied(&self, feedback_id: &str) -> bool {
        self.copied_feedback
            .as_ref()
            .is_some_and(|(id, until)| id == feedback_id && Instant::now() < *until)
    }

    pub(crate) fn is_hero_stage(&self) -> bool {
        match self.selected_thread() {
            None => true,
            Some((hs, th)) => {
                th.status != ThreadStatus::Running
                    && !hs.timeline_items.iter().any(|i| i.thread_id == th.id)
            }
        }
    }

    pub(crate) fn is_timeline_near_bottom(&self) -> bool {
        let max_y = self.timeline_scroll.max_offset().y;
        let off_y = self.timeline_scroll.offset().y;
        max_y <= px(1.0) || (max_y + off_y) <= px(96.0)
    }

    fn drain_host_updates(&mut self, cx: &mut Context<Self>) {
        let updates = self.hosts_mgr.drain_updates();
        if updates.is_empty() {
            return;
        }
        let mut need_diff_sync = false;
        let mut need_persist_prefs = false;
        let was_near_bottom = self.is_timeline_near_bottom();

        for upd in updates {
            let host_id = upd.host_id;
            let Some(hstate) = self.host_states.get_mut(&host_id) else {
                continue;
            };
            match upd.kind {
                ClientUpdateKind::Connected {
                    hostname,
                    daemon_version,
                } => {
                    hstate.connected = true;
                    hstate.hostname = hostname;
                    hstate.daemon_version = daemon_version;
                    hstate.status_message = "Online".into();
                }
                ClientUpdateKind::Disconnected(msg) => {
                    hstate.connected = false;
                    hstate.status_message = msg;
                }
                ClientUpdateKind::FullSnapshot(snap) => {
                    hstate.apply_snapshot(snap);
                    if self.preferred_agent_id.is_none() && self.draft_agent_id == "codex" {
                        self.draft_agent_id = hstate.default_agent_id();
                    }
                    if self.selected.is_none() {
                        if let Some(first) = hstate.threads.first() {
                            let key = ThreadKey::new(&host_id, &first.id);
                            self.selected = Some(key.clone());
                            self.hosts_mgr.send(
                                &key.host_id,
                                RpcCommand::GetThreadDiff {
                                    thread_id: key.thread_id.clone(),
                                },
                            );
                            if self.show_terminal_dock {
                                let (cols, rows) = {
                                    let l = self.terminal_layout.borrow();
                                    (l.cols.clamp(20, 400) as u16, l.rows.clamp(4, 150) as u16)
                                };
                                self.hosts_mgr.send(
                                    &key.host_id,
                                    RpcCommand::TerminalOpen {
                                        thread_id: key.thread_id,
                                        cols,
                                        rows,
                                    },
                                );
                            }
                            self.timeline_scroll.scroll_to_bottom();
                            need_diff_sync = true;
                        }
                    }
                }
                ClientUpdateKind::DiffUpdated { thread_id, diff } => {
                    hstate.diffs.insert(thread_id, diff);
                    need_diff_sync = true;
                }
                ClientUpdateKind::AgentsUpdated(agents) => {
                    hstate.agents = agents;
                }
                ClientUpdateKind::Event(env) => {
                    let host_label = hstate.label.clone();
                    match &env.event {
                        DomainEvent::ApprovalRequested { approval } => {
                            send_notification(
                                &format!("Approval Needed ({host_label})"),
                                &format!("{}: {}", approval.tool_name, approval.description),
                                true,
                            );
                        }
                        DomainEvent::TurnCompleted {
                            thread_id,
                            status,
                            error,
                            ..
                        } => {
                            let is_current = self
                                .selected
                                .as_ref()
                                .map(|k| k.host_id == host_id && k.thread_id == *thread_id)
                                .unwrap_or(false);
                            let thread_title = hstate
                                .threads
                                .iter()
                                .find(|t| t.id == *thread_id)
                                .map(|t| t.title.clone())
                                .unwrap_or_else(|| "Thread".to_string());
                            match status {
                                TurnCompletionStatus::Completed if !is_current => {
                                    send_notification(
                                        &format!("Agent Finished ({host_label})"),
                                        &thread_title,
                                        true,
                                    );
                                }
                                TurnCompletionStatus::Failed => {
                                    let msg = error.as_deref().unwrap_or("Turn failed");
                                    send_notification(
                                        &format!("Agent Error ({host_label})"),
                                        &format!("{thread_title}: {msg}"),
                                        true,
                                    );
                                }
                                _ => {}
                            }
                        }
                        DomainEvent::TimelineItemUpserted { .. }
                        | DomainEvent::TimelineItemAppended { .. } => {
                            if was_near_bottom {
                                self.timeline_scroll.scroll_to_bottom();
                            }
                            if self.split_thread.is_some() {
                                let max_y = self.split_timeline_scroll.max_offset().y;
                                let off_y = self.split_timeline_scroll.offset().y;
                                if max_y <= px(1.0) || (max_y + off_y) <= px(96.0) {
                                    self.split_timeline_scroll.scroll_to_bottom();
                                }
                            }
                        }
                        DomainEvent::DiffUpdated { .. } => {
                            need_diff_sync = true;
                        }
                        _ => {}
                    }

                    let deleted_thread_id = match &env.event {
                        DomainEvent::ThreadDeleted { thread_id } => Some(thread_id.clone()),
                        _ => None,
                    };
                    let created_project_id = match &env.event {
                        DomainEvent::ProjectCreated { project } => Some(project.id.clone()),
                        _ => None,
                    };
                    let created_thread_id = match &env.event {
                        DomainEvent::ThreadCreated { thread } => Some(thread.id.clone()),
                        _ => None,
                    };
                    hstate.apply_event(env);

                    if let Some(dtid) = deleted_thread_id {
                        if self.pinned_threads.remove(&dtid) {
                            need_persist_prefs = true;
                        }
                    }

                    if let Some(pid) = created_project_id {
                        if self.pending_auto_prompt.is_some() && self.selected.is_none() {
                            self.hosts_mgr.send(
                                &host_id,
                                RpcCommand::CreateThread {
                                    project_id: pid,
                                    title: None,
                                    agent_id: self.draft_agent_id.clone(),
                                    model: self.draft_model.clone(),
                                    thinking: self.draft_thinking.clone(),
                                    execution_mode: self.draft_mode,
                                    workspace_mode: self.new_thread_workspace_mode,
                                },
                            );
                        }
                    }

                    if let Some(tid) = created_thread_id {
                        let key = ThreadKey::new(&host_id, tid.clone());
                        if self.pending_split_for_new_thread && self.selected.is_some() {
                            self.pending_split_for_new_thread = false;
                            self.split_thread = Some(key.clone());
                            self.hosts_mgr.send(
                                &key.host_id,
                                RpcCommand::GetThreadDiff {
                                    thread_id: key.thread_id.clone(),
                                },
                            );
                            self.split_timeline_scroll.scroll_to_bottom();
                        } else {
                            self.pending_split_for_new_thread = false;
                            self.selected = Some(key.clone());
                            self.editing_thread_title = false;
                            self.selected_diff_file = None;
                            self.hosts_mgr.send(
                                &key.host_id,
                                RpcCommand::GetThreadDiff {
                                    thread_id: key.thread_id.clone(),
                                },
                            );
                            if let Some((prompt, attachments)) = self.pending_auto_prompt.take() {
                                self.hosts_mgr.send(
                                    &key.host_id,
                                    RpcCommand::SendPrompt {
                                        thread_id: tid,
                                        prompt,
                                        attachments,
                                    },
                                );
                            }
                            self.timeline_scroll.scroll_to_bottom();
                            need_diff_sync = true;
                        }
                    }

                    if let Some(ref sel) = self.selected {
                        if sel.host_id == host_id
                            && !hstate.threads.iter().any(|t| t.id == sel.thread_id)
                        {
                            self.selected = hstate
                                .threads
                                .first()
                                .map(|t| ThreadKey::new(&host_id, &t.id));
                            need_diff_sync = true;
                        }
                    }
                    if let Some(ref spl) = self.split_thread {
                        if spl.host_id == host_id
                            && !hstate.threads.iter().any(|t| t.id == spl.thread_id)
                        {
                            self.split_thread = None;
                        }
                    }
                }
                ClientUpdateKind::TerminalOutput { thread_id, data } => {
                    let replies = {
                        let buf = hstate.terminals.entry(thread_id.clone()).or_default();
                        buf.feed(&data);
                        buf.take_replies()
                    };
                    for reply in replies {
                        let data_b64 = base64::engine::general_purpose::STANDARD.encode(reply);
                        self.hosts_mgr.send(
                            &host_id,
                            RpcCommand::TerminalWrite {
                                thread_id: thread_id.clone(),
                                data_b64,
                            },
                        );
                    }
                }
                ClientUpdateKind::TerminalExited { thread_id, code } => {
                    let buf = hstate.terminals.entry(thread_id).or_default();
                    buf.opened = false;
                    buf.exited_code = code;
                }
                ClientUpdateKind::ChecksUpdated { thread_id, checks } => {
                    let any_failed = checks.iter().any(|c| {
                        matches!(c.status, CheckRunStatus::Failed | CheckRunStatus::TimedOut)
                    });
                    let is_current = self
                        .selected
                        .as_ref()
                        .is_some_and(|k| k.host_id == host_id && k.thread_id == thread_id);
                    hstate.thread_checks.insert(thread_id, checks);
                    if is_current && any_failed {
                        self.show_checks_drawer = true;
                    }
                }
                ClientUpdateKind::ProjectConfigUpdated { project_id, config } => {
                    if let Some(cfg) = config {
                        hstate.project_configs.insert(project_id, cfg);
                    } else {
                        hstate.project_configs.remove(&project_id);
                    }
                }
                ClientUpdateKind::CommandError(err) => {
                    self.last_error_toast = Some(err);
                }
            }
        }
        if need_persist_prefs {
            self.persist_prefs();
        }
        if need_diff_sync {
            self.sync_diff_state(cx);
        }
        cx.notify();
    }

    pub(crate) fn sync_diff_state(&mut self, cx: &mut Context<Self>) {
        if !self.show_diff_pane || self.inspector_tab != InspectorTab::Changes {
            return;
        }
        let Some(key) = self.selected.clone() else {
            if self.diff_synced_sig.is_some() {
                self.diff_synced_sig = None;
                self.diff_state.update(cx, |s, cx| {
                    s.set_files(Vec::<DiffFile>::new(), cx);
                });
            }
            return;
        };

        let (diff_len, files_len, sel_valid) = self
            .host_states
            .get(&key.host_id)
            .and_then(|h| h.diffs.get(&key.thread_id))
            .map(|d| {
                let valid = self
                    .selected_diff_file
                    .as_ref()
                    .map(|sel| d.files.iter().any(|f| &f.path == sel))
                    .unwrap_or(true);
                (d.unified_diff.len(), d.files.len(), valid)
            })
            .unwrap_or((0, 0, false));

        if !sel_valid {
            self.selected_diff_file = None;
        }

        let sig = (
            key.host_id.clone(),
            key.thread_id.clone(),
            diff_len,
            files_len,
            self.selected_diff_file.clone(),
        );
        if self.diff_synced_sig.as_ref() == Some(&sig) {
            return;
        }
        self.diff_synced_sig = Some(sig);

        let mut files = self
            .host_states
            .get(&key.host_id)
            .and_then(|h| h.diffs.get(&key.thread_id))
            .filter(|d| !d.unified_diff.trim().is_empty())
            .and_then(|d| DiffFile::parse(&d.unified_diff).ok())
            .unwrap_or_default();

        if let Some(ref filter_path) = self.selected_diff_file {
            files.retain(|f| f.path().as_ref() == filter_path.as_str());
        }

        self.diff_state.update(cx, |s, cx| {
            s.set_files(files, cx);
        });
    }

    pub(crate) fn selected_thread(&self) -> Option<(&HostWorkspaceState, &ThreadRecord)> {
        let key = self.selected.as_ref()?;
        let hs = self.host_states.get(&key.host_id)?;
        let thread = hs.threads.iter().find(|t| t.id == key.thread_id)?;
        Some((hs, thread))
    }

    pub(crate) fn selected_diff_snapshot(&self) -> Option<&GitDiffSnapshot> {
        let key = self.selected.as_ref()?;
        self.host_states
            .get(&key.host_id)
            .and_then(|h| h.diffs.get(&key.thread_id))
    }

    pub(crate) fn pick_local_project_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select Project Repository Folder".into()),
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(dir) = paths.into_iter().next() {
                    let path_str = dir.to_string_lossy().to_string();
                    let _ = this.update(cx, |app, cx| {
                        app.hosts_mgr.send(
                            LOCAL_HOST_ID,
                            RpcCommand::CreateProject {
                                name: None,
                                root_path: path_str,
                            },
                        );
                        app.active_modal = ActiveModal::None;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(crate) fn pick_attachment_files(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach Files to Prompt".into()),
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |app, cx| {
                    for p in paths {
                        let raw = p.to_string_lossy().to_string();
                        let label = Path::new(&raw)
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| raw.clone());
                        app.pending_attachments.push(Attachment { path: raw, label });
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn open_add_project_modal(
        &mut self,
        host_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default_dir = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "~".to_string());
        self.project_path_input.update(cx, |s, cx| {
            s.set_value(default_dir, window, cx);
            s.focus(window, cx);
        });
        self.active_modal = ActiveModal::AddProjectPath(host_id);
        cx.notify();
    }

    pub(crate) fn submit_project_path_modal(
        &mut self,
        host_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.project_path_input.read(cx).value().trim().to_string();
        if path.is_empty() {
            return;
        }
        self.hosts_mgr.send(
            &host_id,
            RpcCommand::CreateProject {
                name: None,
                root_path: path,
            },
        );
        self.project_path_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.active_modal = ActiveModal::None;
        cx.notify();
    }

    pub(crate) fn add_remote_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.remote_name_input.read(cx).value().trim().to_string();
        let url = self.remote_url_input.read(cx).value().trim().to_string();
        let token = self.remote_token_input.read(cx).value().trim().to_string();
        match self.hosts_mgr.add_remote(name, url, token) {
            Ok(remote) => {
                let remote_name = remote.name.clone();
                self.host_order.push(remote.id.clone());
                self.host_states.insert(
                    remote.id.clone(),
                    HostWorkspaceState::new(remote.id, remote.name, false),
                );
                self.last_error_toast = None;
                self.settings_notice = Some((
                    false,
                    format!("Connected remote host \"{remote_name}\"."),
                ));
                self.remote_name_input
                    .update(cx, |s, cx| s.set_value("", window, cx));
                self.remote_url_input
                    .update(cx, |s, cx| s.set_value("", window, cx));
                self.remote_token_input
                    .update(cx, |s, cx| s.set_value("", window, cx));
            }
            Err(err) => {
                let msg = err.to_string();
                self.last_error_toast = Some(msg.clone());
                self.settings_notice = Some((true, msg));
            }
        }
        cx.notify();
    }

    pub(crate) fn remove_remote_host(&mut self, host_id: String, cx: &mut Context<Self>) {
        if host_id == LOCAL_HOST_ID {
            return;
        }
        self.hosts_mgr.remove_remote(&host_id);
        self.host_order.retain(|id| id != &host_id);
        self.host_states.remove(&host_id);
        if self
            .selected
            .as_ref()
            .map(|k| k.host_id == host_id)
            .unwrap_or(false)
        {
            self.selected = None;
        }
        cx.notify();
    }

    pub(crate) fn target_project_for_new_thread(&self) -> Option<(String, String, String)> {
        self.selected_thread()
            .and_then(|(hs, th)| {
                hs.projects
                    .iter()
                    .find(|pr| pr.id == th.project_id)
                    .map(|pr| (hs.host_id.clone(), pr.id.clone(), pr.name.clone()))
            })
            .or_else(|| {
                self.host_order
                    .iter()
                    .filter_map(|hid| self.host_states.get(hid))
                    .find_map(|hs| {
                        hs.projects
                            .first()
                            .map(|pr| (hs.host_id.clone(), pr.id.clone(), pr.name.clone()))
                    })
            })
    }

    pub(crate) fn create_thread(
        &mut self,
        host_id: String,
        project_id: String,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let proj_defaults = self
            .host_states
            .get(&host_id)
            .and_then(|h| h.project_configs.get(&project_id))
            .map(|c| c.defaults.clone());

        let agent_id = proj_defaults
            .as_ref()
            .and_then(|d| d.agent.clone())
            .filter(|aid| {
                self.host_states
                    .get(&host_id)
                    .is_some_and(|h| h.agents.iter().any(|a| a.id == *aid && a.available))
            })
            .or_else(|| self.selected_thread().map(|(_, t)| t.active_agent_id.clone()))
            .unwrap_or_else(|| {
                self.host_states
                    .get(&host_id)
                    .map(|h| h.default_agent_id())
                    .unwrap_or_else(|| self.draft_agent_id.clone())
            });
        let model = proj_defaults
            .as_ref()
            .and_then(|d| d.model.clone())
            .or_else(|| self.selected_thread().and_then(|(_, t)| t.active_model.clone()))
            .or_else(|| self.draft_model.clone());
        let thinking = proj_defaults
            .as_ref()
            .and_then(|d| d.thinking.clone())
            .or_else(|| self.selected_thread().and_then(|(_, t)| t.active_thinking.clone()))
            .or_else(|| self.draft_thinking.clone());
        let execution_mode = proj_defaults
            .as_ref()
            .and_then(|d| d.execution_mode)
            .or_else(|| self.selected_thread().map(|(_, t)| t.execution_mode))
            .unwrap_or(self.draft_mode);
        let workspace_mode = proj_defaults
            .as_ref()
            .and_then(|d| d.workspace_mode)
            .unwrap_or(self.new_thread_workspace_mode);

        self.last_error_toast = None;
        self.hosts_mgr.send(
            &host_id,
            RpcCommand::CreateThread {
                project_id,
                title: None,
                agent_id,
                model,
                thinking,
                execution_mode,
                workspace_mode,
            },
        );
        if let Some(w) = window {
            self.prompt_input.update(cx, |s, cx| s.focus(w, cx));
        }
        cx.notify();
    }

    pub(crate) fn select_thread(
        &mut self,
        key: ThreadKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_thread_inner(key, true, window, cx);
    }

    fn select_thread_inner(
        &mut self,
        key: ThreadKey,
        record_history: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if record_history && self.selected.as_ref() != Some(&key) {
            if self.nav_history.is_empty() {
                if let Some(prev) = self.selected.clone() {
                    self.nav_history.push(prev);
                }
            } else if self.nav_index + 1 < self.nav_history.len() {
                self.nav_history.truncate(self.nav_index + 1);
            }
            if self.nav_history.last() != Some(&key) {
                self.nav_history.push(key.clone());
                if self.nav_history.len() > 64 {
                    self.nav_history.remove(0);
                }
            }
            self.nav_index = self.nav_history.len().saturating_sub(1);
        }

        self.selected = Some(key.clone());
        self.editing_thread_title = false;
        self.show_session_search = false;
        self.show_checks_drawer = false;
        self.selected_diff_file = None;
        self.model_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
        });
        self.hosts_mgr.send(
            &key.host_id,
            RpcCommand::GetThreadDiff {
                thread_id: key.thread_id.clone(),
            },
        );
        if self.show_terminal_dock {
            let (cols, rows) = {
                let l = self.terminal_layout.borrow();
                (l.cols.clamp(20, 400) as u16, l.rows.clamp(4, 150) as u16)
            };
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::TerminalOpen {
                    thread_id: key.thread_id,
                    cols,
                    rows,
                },
            );
        }
        if self.show_diff_pane && self.inspector_tab == InspectorTab::Files {
            self.sync_files_inspector(false);
        }
        self.sync_diff_state(cx);
        self.timeline_scroll.scroll_to_bottom();
        self.prompt_input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn can_navigate_back(&self) -> bool {
        self.nav_index > 0 && self.nav_history.len() > 1
    }

    pub(crate) fn can_navigate_forward(&self) -> bool {
        self.nav_index + 1 < self.nav_history.len()
    }

    pub(crate) fn navigate_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_navigate_back() {
            self.nav_index -= 1;
            if let Some(key) = self.nav_history.get(self.nav_index).cloned() {
                self.select_thread_inner(key, false, window, cx);
            }
        }
    }

    pub(crate) fn navigate_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_navigate_forward() {
            self.nav_index += 1;
            if let Some(key) = self.nav_history.get(self.nav_index).cloned() {
                self.select_thread_inner(key, false, window, cx);
            }
        }
    }

    pub(crate) fn start_thread_title_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, th)) = self.selected_thread() else {
            return;
        };
        let current = th.title.clone();
        self.editing_thread_title = true;
        self.thread_title_input.update(cx, |s, cx| {
            s.set_value(current, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    pub(crate) fn commit_thread_title_edit(&mut self, cx: &mut Context<Self>) {
        if !self.editing_thread_title {
            return;
        }
        self.editing_thread_title = false;
        let Some(key) = self.selected.clone() else {
            cx.notify();
            return;
        };
        let new_title = self.thread_title_input.read(cx).value().trim().to_string();
        if !new_title.is_empty() {
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::UpdateThreadSettings {
                    thread_id: key.thread_id,
                    title: Some(new_title),
                    agent_id: None,
                    model: None,
                    thinking: None,
                    execution_mode: None,
                },
            );
        }
        cx.notify();
    }

    pub(crate) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_selected_idx = 0;
        self.palette_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        self.active_modal = ActiveModal::CommandPalette;
        cx.notify();
    }

    pub(crate) fn ordered_session_keys(&self) -> Vec<ThreadKey> {
        let mut pinned: Vec<(i64, ThreadKey)> = Vec::new();
        let mut rest: Vec<(i64, ThreadKey)> = Vec::new();
        for hid in &self.host_order {
            if let Some(hs) = self.host_states.get(hid) {
                for t in &hs.threads {
                    let key = ThreadKey::new(hid, &t.id);
                    if self.pinned_threads.contains(&t.id) {
                        pinned.push((t.updated_at, key));
                    } else {
                        rest.push((t.updated_at, key));
                    }
                }
            }
        }
        pinned.sort_by(|a, b| b.0.cmp(&a.0));
        rest.sort_by(|a, b| b.0.cmp(&a.0));
        pinned
            .into_iter()
            .chain(rest)
            .map(|(_, k)| k)
            .collect()
    }

    pub(crate) fn quick_jump_session(
        &mut self,
        index_1_based: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_modal == ActiveModal::Settings {
            let tabs = [
                SettingsTab::Appearance,
                SettingsTab::InputSession,
                SettingsTab::Agents,
                SettingsTab::Workflow,
                SettingsTab::Daemons,
            ];
            if let Some(&tab) = tabs.get(index_1_based.saturating_sub(1)) {
                self.settings_tab = tab;
                self.settings_notice = None;
                cx.notify();
            }
            return;
        }
        let keys = self.ordered_session_keys();
        if let Some(target) = keys.get(index_1_based.saturating_sub(1)).cloned() {
            self.select_thread(target, window, cx);
        }
    }

    pub(crate) fn toggle_session_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_session_search = !self.show_session_search;
        if self.show_session_search {
            self.session_search_match_idx = 0;
            self.session_search_input.update(cx, |s, cx| {
                s.focus(window, cx);
            });
        } else {
            self.prompt_input.update(cx, |s, cx| {
                s.focus(window, cx);
            });
        }
        cx.notify();
    }

    pub(crate) fn toggle_split_pane(&mut self, cx: &mut Context<Self>) {
        if self.split_thread.is_some() {
            self.split_thread = None;
            cx.notify();
            return;
        }
        let next = self
            .ordered_session_keys()
            .into_iter()
            .find(|k| self.selected.as_ref() != Some(k));
        if let Some(key) = next {
            self.open_in_split_pane(key, cx);
        } else {
            self.last_error_toast =
                Some("Only 1 session — click Parallel to fork a second agent in a worktree.".into());
            cx.notify();
        }
    }

    pub(crate) fn open_in_split_pane(&mut self, key: ThreadKey, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&key) {
            return;
        }
        self.split_thread = Some(key.clone());
        self.hosts_mgr.send(
            &key.host_id,
            RpcCommand::GetThreadDiff {
                thread_id: key.thread_id,
            },
        );
        self.split_timeline_scroll.scroll_to_bottom();
        cx.notify();
    }

    pub(crate) fn focus_split_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(split_key) = self.split_thread.clone() else {
            return;
        };
        let prev_primary = self.selected.clone();
        self.split_thread = prev_primary;
        self.select_thread(split_key, window, cx);
    }

    pub(crate) fn active_project_workflow(&self) -> Option<&ProjectWorkflowConfig> {
        if let Some((hs, th)) = self.selected_thread() {
            return hs.project_configs.get(&th.project_id);
        }
        let (hid, pid, _) = self.target_project_for_new_thread()?;
        self.host_states
            .get(&hid)
            .and_then(|hs| hs.project_configs.get(&pid))
    }

    pub(crate) fn selected_thread_checks(&self) -> &[CheckResultRecord] {
        let Some(key) = self.selected.as_ref() else {
            return &[];
        };
        self.host_states
            .get(&key.host_id)
            .and_then(|hs| hs.thread_checks.get(&key.thread_id))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub(crate) fn run_selected_project_checks(
        &mut self,
        check_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        self.last_error_toast = None;
        self.show_checks_drawer = true;
        self.hosts_mgr.send(
            &key.host_id,
            RpcCommand::RunProjectChecks {
                thread_id: key.thread_id,
                check_name,
            },
        );
        cx.notify();
    }

    pub(crate) fn init_active_project_workflow(&mut self, cx: &mut Context<Self>) {
        let Some((host_id, project_id, _)) = self.target_project_for_new_thread() else {
            self.last_error_toast =
                Some("Add a project folder first to initialize .hasilanx.toml.".into());
            cx.notify();
            return;
        };
        self.last_error_toast = None;
        self.hosts_mgr.send(
            &host_id,
            RpcCommand::InitProjectWorkflow { project_id },
        );
        self.show_checks_drawer = true;
        cx.notify();
    }

    pub(crate) fn reload_active_project_workflow(&mut self, cx: &mut Context<Self>) {
        let Some((host_id, project_id, _)) = self.target_project_for_new_thread() else {
            return;
        };
        self.hosts_mgr.send(
            &host_id,
            RpcCommand::ReloadProjectWorkflow { project_id },
        );
        cx.notify();
    }

    pub(crate) fn auto_fix_failed_check(
        &mut self,
        check: &CheckResultRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let exit_str = check
            .exit_code
            .map(|c| format!(" (exit code {c})"))
            .unwrap_or_default();
        let output_tail: String = if check.output.trim().is_empty() {
            "(no output captured)".to_string()
        } else {
            let chars: Vec<char> = check.output.chars().collect();
            if chars.len() > 6000 {
                chars[chars.len() - 6000..].iter().collect()
            } else {
                check.output.clone()
            }
        };
        let prompt = format!(
            "The `.hasilanx.toml` quality gate check **{}** (`{}`) failed{exit_str}.\n\
             Diagnose the root cause, fix it cleanly, and confirm the check passes.\n\n\
             ```text\n{output_tail}\n```",
            check.name, check.command
        );
        self.send_direct_prompt(prompt, window, cx);
    }

    pub(crate) fn start_parallel_worktree_compare(&mut self, cx: &mut Context<Self>) {
        let Some((host_id, project_id, _)) = self.target_project_for_new_thread() else {
            self.last_error_toast = Some("Add a project folder first to run parallel compare.".into());
            cx.notify();
            return;
        };
        let (cur_agent, cur_title, mode) = self
            .selected_thread()
            .map(|(_, th)| {
                (
                    th.active_agent_id.clone(),
                    th.title.clone(),
                    th.execution_mode,
                )
            })
            .unwrap_or_else(|| {
                (
                    self.draft_agent_id.clone(),
                    "Session".to_string(),
                    self.draft_mode,
                )
            });

        // Prefer .hasilanx.toml [roles].reviewer (or implementer) if configured & available
        let configured_role_agent = self
            .host_states
            .get(&host_id)
            .and_then(|hs| {
                let cfg = hs.project_configs.get(&project_id)?;
                let preferred = if cfg.roles.reviewer.as_deref() != Some(cur_agent.as_str()) {
                    cfg.roles.reviewer.as_deref()
                } else {
                    cfg.roles.implementer.as_deref()
                };
                preferred.and_then(|wanted| {
                    hs.agents
                        .iter()
                        .find(|a| a.available && a.id == wanted)
                        .map(|a| a.id.clone())
                })
            });

        // Otherwise pick an alternate available agent when one exists (e.g. codex <-> agy / opencode)
        let alt_agent = configured_role_agent
            .or_else(|| {
                self.host_states.get(&host_id).and_then(|hs| {
                    hs.agents
                        .iter()
                        .find(|a| a.available && a.id != cur_agent)
                        .map(|a| a.id.clone())
                })
            })
            .unwrap_or_else(|| cur_agent.clone());

        let base = cur_title.trim();
        let short_base: String = base.chars().take(32).collect();
        let title = format!(
            "{} ({})",
            if short_base.is_empty() {
                "Compare"
            } else {
                &short_base
            },
            crate::time_fmt::agent_display_name(&alt_agent)
        );

        self.last_error_toast = None;
        self.pending_split_for_new_thread = self.selected.is_some();
        self.hosts_mgr.send(
            &host_id,
            RpcCommand::CreateThread {
                project_id,
                title: Some(title),
                agent_id: alt_agent,
                model: None,
                thinking: None,
                execution_mode: mode,
                workspace_mode: WorkspaceMode::Worktree,
            },
        );
        cx.notify();
    }

    pub(crate) fn send_direct_prompt(
        &mut self,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_input.update(cx, |s, cx| {
            s.set_value(prompt, window, cx);
        });
        self.send_or_queue_prompt(false, window, cx);
    }

    pub(crate) fn send_or_queue_prompt(
        &mut self,
        force_steer: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = self.prompt_input.read(cx).value().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        if !force_steer && self.try_execute_exact_slash_command(&prompt, window, cx) {
            return;
        }
        self.last_error_toast = None;
        let attachments = std::mem::take(&mut self.pending_attachments);

        self.prompt_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });

        let Some(key) = self.selected.clone() else {
            self.pending_auto_prompt = Some((prompt, attachments));
            if let Some((hid, pid, _)) = self.target_project_for_new_thread() {
                self.create_thread(hid, pid, Some(window), cx);
            } else {
                let cwd = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "~".to_string());
                self.hosts_mgr.send(
                    LOCAL_HOST_ID,
                    RpcCommand::CreateProject {
                        name: None,
                        root_path: cwd,
                    },
                );
            }
            cx.notify();
            return;
        };

        if force_steer {
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::SteerTurn {
                    thread_id: key.thread_id,
                    prompt,
                },
            );
        } else {
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::SendPrompt {
                    thread_id: key.thread_id,
                    prompt,
                    attachments,
                },
            );
        }
        self.timeline_scroll.scroll_to_bottom();
        cx.notify();
    }

    pub(crate) fn cancel_turn(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = self.selected.clone() {
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::CancelTurn {
                    thread_id: key.thread_id,
                },
            );
            cx.notify();
        }
    }

    pub(crate) fn set_thread_agent(&mut self, agent_id: String, cx: &mut Context<Self>) {
        self.draft_agent_id = agent_id.clone();
        self.draft_model = None;
        self.draft_thinking = None;
        if let Some(key) = self.selected.clone() {
            if let Some(th) = self
                .host_states
                .get_mut(&key.host_id)
                .and_then(|h| h.threads.iter_mut().find(|t| t.id == key.thread_id))
            {
                th.active_agent_id = agent_id.clone();
                th.active_model = None;
                th.active_thinking = None;
            }
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::UpdateThreadSettings {
                    thread_id: key.thread_id,
                    title: None,
                    agent_id: Some(agent_id),
                    model: Some(None),
                    thinking: Some(None),
                    execution_mode: None,
                },
            );
        }
        cx.notify();
    }

    pub(crate) fn set_thread_mode(&mut self, mode: ExecutionMode, cx: &mut Context<Self>) {
        self.draft_mode = mode;
        if let Some(key) = self.selected.clone() {
            if let Some(th) = self
                .host_states
                .get_mut(&key.host_id)
                .and_then(|h| h.threads.iter_mut().find(|t| t.id == key.thread_id))
            {
                th.execution_mode = mode;
            }
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::UpdateThreadSettings {
                    thread_id: key.thread_id,
                    title: None,
                    agent_id: None,
                    model: None,
                    thinking: None,
                    execution_mode: Some(mode),
                },
            );
        }
        cx.notify();
    }

    pub(crate) fn select_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        self.draft_model = model.clone();
        self.draft_thinking = None;
        if let Some(key) = self.selected.clone() {
            if let Some(th) = self
                .host_states
                .get_mut(&key.host_id)
                .and_then(|h| h.threads.iter_mut().find(|t| t.id == key.thread_id))
            {
                th.active_model = model.clone();
                th.active_thinking = None;
            }
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::UpdateThreadSettings {
                    thread_id: key.thread_id,
                    title: None,
                    agent_id: None,
                    model: Some(model),
                    thinking: Some(None),
                    execution_mode: None,
                },
            );
        }
        cx.notify();
    }

    pub(crate) fn set_thread_thinking(&mut self, thinking: Option<String>, cx: &mut Context<Self>) {
        self.draft_thinking = thinking.clone();
        if let Some(key) = self.selected.clone() {
            if let Some(th) = self
                .host_states
                .get_mut(&key.host_id)
                .and_then(|h| h.threads.iter_mut().find(|t| t.id == key.thread_id))
            {
                th.active_thinking = thinking.clone();
            }
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::UpdateThreadSettings {
                    thread_id: key.thread_id,
                    title: None,
                    agent_id: None,
                    model: None,
                    thinking: Some(thinking),
                    execution_mode: None,
                },
            );
        }
        cx.notify();
    }

    pub(crate) fn apply_custom_model_input(&mut self, cx: &mut Context<Self>) {
        let raw = self.model_input.read(cx).value().trim().to_string();
        let model = if raw.is_empty() { None } else { Some(raw) };
        self.select_model(model, cx);
    }

    pub(crate) fn refresh_agent_models(&mut self, host_id: &str, cx: &mut Context<Self>) {
        self.hosts_mgr.send(host_id, RpcCommand::RefreshModels);
        if self.active_modal == ActiveModal::Settings {
            self.settings_notice =
                Some((false, "Refreshing live model catalogs from agents...".into()));
        }
        cx.notify();
    }

    pub(crate) fn submit_git_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let msg = self.commit_msg_input.read(cx).value().trim().to_string();
        if msg.is_empty() {
            self.last_error_toast = Some("Enter a commit message first.".into());
            cx.notify();
            return;
        }
        self.hosts_mgr.send(
            &key.host_id,
            RpcCommand::GitCommit {
                thread_id: key.thread_id,
                message: msg,
            },
        );
        self.commit_msg_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    pub(crate) fn resolve_approval(
        &mut self,
        host_id: String,
        approval_id: String,
        decision: ApprovalDecision,
        option_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.hosts_mgr.send(
            &host_id,
            RpcCommand::ResolveApproval {
                approval_id,
                decision,
                option_id,
            },
        );
        cx.notify();
    }

    pub(crate) fn cancel_selected_turn(&mut self, cx: &mut Context<Self>) {
        self.cancel_turn(cx);
    }

    pub(crate) fn toggle_diff_pane(&mut self, cx: &mut Context<Self>) {
        if self.show_diff_pane && self.inspector_tab == InspectorTab::Changes {
            self.show_diff_pane = false;
        } else {
            self.show_diff_pane = true;
            self.inspector_tab = InspectorTab::Changes;
            if let Some(key) = self.selected.clone() {
                self.hosts_mgr.send(
                    &key.host_id,
                    RpcCommand::GetThreadDiff {
                        thread_id: key.thread_id,
                    },
                );
            }
            self.sync_diff_state(cx);
        }
        cx.notify();
    }

    pub(crate) fn toggle_files_pane(&mut self, cx: &mut Context<Self>) {
        if self.show_diff_pane && self.inspector_tab == InspectorTab::Files {
            self.show_diff_pane = false;
        } else {
            self.show_diff_pane = true;
            self.inspector_tab = InspectorTab::Files;
            self.sync_files_inspector(false);
        }
        cx.notify();
    }

    pub(crate) fn toggle_diff_mode(&mut self, cx: &mut Context<Self>) {
        self.diff_mode = match self.diff_mode {
            DiffMode::Split => DiffMode::Unified,
            _ => DiffMode::Split,
        };
        let mode = self.diff_mode;
        self.diff_state.update(cx, |s, cx| {
            s.set_mode(mode, cx);
        });
        cx.notify();
    }

    pub(crate) fn toggle_terminal_dock(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.show_terminal_dock = !self.show_terminal_dock;
        if self.show_terminal_dock {
            let (cols, rows) = {
                let l = self.terminal_layout.borrow();
                (l.cols.clamp(20, 400), l.rows.clamp(4, 150))
            };
            if let Some(key) = self.selected.clone() {
                if let Some(h) = self.host_states.get_mut(&key.host_id) {
                    h.terminals
                        .entry(key.thread_id.clone())
                        .or_insert_with(|| crate::terminal::TerminalBuffer::with_size(cols, rows));
                }
                self.hosts_mgr.send(
                    &key.host_id,
                    RpcCommand::TerminalOpen {
                        thread_id: key.thread_id,
                        cols: cols as u16,
                        rows: rows as u16,
                    },
                );
            }
            if let Some(w) = window {
                self.terminal_focus.focus(w, cx);
            }
        } else {
            self.terminal_maximized = false;
            if let Some(w) = window {
                self.prompt_input.update(cx, |s, cx| s.focus(w, cx));
            }
        }
        cx.notify();
    }

    pub(crate) fn send_terminal_bytes(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        self.hosts_mgr.send(
            &key.host_id,
            RpcCommand::TerminalWrite {
                thread_id: key.thread_id,
                data_b64,
            },
        );
        cx.notify();
    }

    fn render_top_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();
        let selected_info = self.selected_thread().map(|(hs, th)| {
            let pname = hs
                .projects
                .iter()
                .find(|pr| pr.id == th.project_id)
                .map(|pr| pr.name.clone())
                .unwrap_or_default();
            (
                hs.host_id.clone(),
                hs.label.clone(),
                hs.is_local,
                pname,
                th.clone(),
            )
        });

        let (total_add, total_del, has_changes) = self
            .selected_diff_snapshot()
            .map(|d| {
                let add: u32 = d.files.iter().map(|f| f.additions).sum();
                let del: u32 = d.files.iter().map(|f| f.deletions).sum();
                (add, del, !d.files.is_empty())
            })
            .unwrap_or((0, 0, false));
        let can_back = self.can_navigate_back();
        let can_fwd = self.can_navigate_forward();
        let is_hero = self.is_hero_stage();

        div()
            .h(px(42.0))
            .w_full()
            .px_3()
            .when(!is_hero, |el| el.border_b_1().border_color(p.hairline))
            .bg(if is_hero {
                rgba(0x00000000)
            } else {
                p.canvas
            })
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            // Left: Sidebar reopen + Back/Forward history + Editable Thread Title + quiet subtitle
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .flex_1()
                    .when(!self.show_sidebar, |el| {
                        el.child(
                            Button::new("reopen-sidebar")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(Lucide::PanelLeftOpen))
                                .tooltip("Show Sidebar (Ctrl+B)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_sidebar = true;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_0p5()
                            .flex_shrink_0()
                            .child(
                                Button::new("nav-back-btn")
                                    .xsmall()
                                    .ghost()
                                    .disabled(!can_back)
                                    .icon(Icon::new(Lucide::ArrowLeft))
                                    .tooltip("Previous session (Alt+[)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.navigate_back(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("nav-fwd-btn")
                                    .xsmall()
                                    .ghost()
                                    .disabled(!can_fwd)
                                    .icon(Icon::new(Lucide::ArrowRight))
                                    .tooltip("Next session (Alt+])")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.navigate_forward(window, cx);
                                    })),
                            ),
                    )
                    .when_some(
                        selected_info.clone(),
                        |el, (_, host_label, is_local, pname, th)| {
                            let is_running = th.status == ThreadStatus::Running;
                            let is_approval = th.status == ThreadStatus::WaitingApproval;
                            let is_error = th.status == ThreadStatus::Error;
                            let meta_sub = {
                                let mut parts = Vec::new();
                                if !pname.is_empty() {
                                    if is_local {
                                        parts.push(pname);
                                    } else {
                                        parts.push(format!("{pname} @ {host_label}"));
                                    }
                                }
                                if let Some(ref br) = th.branch_name {
                                    parts.push(br.clone());
                                }
                                parts.join(" · ")
                            };
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .min_w_0()
                                    .child(if self.editing_thread_title {
                                        div()
                                            .w(px(240.0))
                                            .child(Input::new(&self.thread_title_input).xsmall())
                                            .into_any_element()
                                    } else {
                                        div()
                                            .id("hdr-thread-title")
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded_md()
                                            .hover(|s| s.bg(p.surface_hover))
                                            .cursor_pointer()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                GlobalState::suppress_text_selection(cx);
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.start_thread_title_edit(window, cx);
                                            }))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_medium()
                                                    .text_color(p.text)
                                                    .truncate()
                                                    .child(th.title.clone()),
                                            )
                                            .when(!meta_sub.is_empty(), |e| {
                                                e.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(p.text_faint)
                                                        .truncate()
                                                        .child(meta_sub),
                                                )
                                            })
                                            .into_any_element()
                                    })
                                    .when(is_running, |e| {
                                        e.child(
                                            div()
                                                .px_2()
                                                .py_0p5()
                                                .rounded_full()
                                                .bg(p.running_soft)
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .text_xs()
                                                .text_color(p.running)
                                                .flex_shrink_0()
                                                .child(
                                                    Spinner::new()
                                                        .xsmall()
                                                        .color(p.running_hsla()),
                                                )
                                                .child("Working"),
                                        )
                                    })
                                    .when(is_approval, |e| {
                                        e.child(
                                            div()
                                                .px_2()
                                                .py_0p5()
                                                .rounded_full()
                                                .bg(p.approval_soft)
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(p.approval)
                                                .flex_shrink_0()
                                                .child(
                                                    Icon::new(Lucide::ShieldAlert).size(px(11.0)),
                                                )
                                                .child("Needs input"),
                                        )
                                    })
                                    .when(is_error, |e| {
                                        e.child(
                                            div()
                                                .px_2()
                                                .py_0p5()
                                                .rounded_full()
                                                .bg(p.danger_soft)
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .text_xs()
                                                .text_color(p.danger)
                                                .flex_shrink_0()
                                                .child(
                                                    Icon::new(Lucide::CircleAlert).size(px(11.0)),
                                                )
                                                .child("Failed"),
                                        )
                                    }),
                            )
                        },
                    )
                    .when(selected_info.is_none(), |el| {
                        el.child(
                            div()
                                .text_xs()
                                .font_medium()
                                .text_color(p.text_muted)
                                .child("New session"),
                        )
                    }),
            )
            // Right: Clean pill controls for Search, Split, Parallel, Changes, Files, Terminal, Trash
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .when_some(selected_info, |el, (hid, _, _, _, th)| {
                        let changes_on =
                            self.show_diff_pane && self.inspector_tab == InspectorTab::Changes;
                        let files_on =
                            self.show_diff_pane && self.inspector_tab == InspectorTab::Files;
                        let split_on = self.split_thread.is_some()
                            && self.split_thread.as_ref() != self.selected.as_ref();
                        let tid_del = th.id.clone();

                        let wf_check_count = self
                            .active_project_workflow()
                            .map(|c| c.checks.commands.len())
                            .unwrap_or(0);
                        let checks = self.selected_thread_checks();
                        let checks_running = checks
                            .iter()
                            .filter(|c| c.status == CheckRunStatus::Running)
                            .count();
                        let checks_failed = checks
                            .iter()
                            .filter(|c| {
                                matches!(c.status, CheckRunStatus::Failed | CheckRunStatus::TimedOut)
                            })
                            .count();
                        let checks_passed = checks
                            .iter()
                            .filter(|c| c.status == CheckRunStatus::Passed)
                            .count();
                        let show_checks_pill = wf_check_count > 0 || !checks.is_empty();
                        let checks_never_run = checks.is_empty() && wf_check_count > 0;

                        el.when(show_checks_pill, |row| {
                            let (pill_bg, pill_fg, label_txt) = if checks_running > 0 {
                                (
                                    p.running_soft,
                                    p.running,
                                    format!("Checking ({checks_running})"),
                                )
                            } else if checks_failed > 0 {
                                (
                                    p.danger_soft,
                                    p.danger,
                                    format!("{checks_failed} Failed"),
                                )
                            } else if checks_passed > 0 {
                                (
                                    p.success_soft,
                                    p.success,
                                    format!("{checks_passed}/{} Checks", checks.len()),
                                )
                            } else {
                                let checks_open = self.show_checks_drawer && !is_hero;
                                (
                                    if checks_open {
                                        p.surface_selected
                                    } else {
                                        rgba(0x00000000)
                                    },
                                    if checks_open {
                                        p.text
                                    } else {
                                        p.text_muted
                                    },
                                    "Checks".to_string(),
                                )
                            };
                            row.child(
                                div()
                                    .id("header-checks-pill")
                                    .h(px(26.0))
                                    .px_2()
                                    .rounded_md()
                                    .bg(pill_bg)
                                    .hover(|s| s.bg(p.surface_hover))
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        GlobalState::suppress_text_selection(cx);
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if checks_never_run && !this.show_checks_drawer {
                                            this.run_selected_project_checks(None, cx);
                                        } else {
                                            this.show_checks_drawer = !this.show_checks_drawer;
                                            cx.notify();
                                        }
                                    }))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(pill_fg)
                                    .child(if checks_running > 0 {
                                        Spinner::new()
                                            .xsmall()
                                            .color(p.running_hsla())
                                            .into_any_element()
                                    } else if checks_failed > 0 {
                                        Icon::new(Lucide::CircleX)
                                            .size(px(12.0))
                                            .into_any_element()
                                    } else if checks_passed > 0 {
                                        Icon::new(Lucide::CircleCheck)
                                            .size(px(12.0))
                                            .into_any_element()
                                    } else {
                                        Icon::new(Lucide::Play)
                                            .size(px(11.0))
                                            .into_any_element()
                                    })
                                    .child(label_txt),
                            )
                        })
                        .child(
                            Button::new("header-search-toggle")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(Lucide::Search))
                                .tooltip("Find in session (Ctrl+F)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_session_search(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .id("header-split-toggle")
                                .h(px(26.0))
                                .px_2()
                                .rounded_md()
                                .bg(if split_on {
                                    p.surface_selected
                                } else {
                                    rgba(0x00000000)
                                })
                                .hover(|s| s.bg(p.surface_hover))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_split_pane(cx);
                                }))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(if split_on { p.accent } else { p.text_muted })
                                .child(Icon::new(Lucide::Columns2).size(px(12.0)))
                                .child("Split"),
                        )
                        .child(
                            div()
                                .id("header-parallel-btn")
                                .h(px(26.0))
                                .px_2()
                                .rounded_md()
                                .hover(|s| s.bg(p.surface_hover).text_color(p.text))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_parallel_worktree_compare(cx);
                                }))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(p.text_muted)
                                .child(Icon::new(Lucide::GitFork).size(px(12.0)))
                                .child("Parallel"),
                        )
                        .child(div().w(px(1.0)).h(px(14.0)).mx_0p5().bg(p.hairline))
                        .child(
                            div()
                                .id("header-diff-toggle")
                                .h(px(26.0))
                                .px_2()
                                .rounded_md()
                                .bg(if changes_on {
                                    p.surface_selected
                                } else {
                                    rgba(0x00000000)
                                })
                                .hover(|s| s.bg(p.surface_hover))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_diff_pane(cx);
                                }))
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .text_color(if changes_on { p.text } else { p.text_muted })
                                .child(Icon::new(Lucide::FileDiff).size(px(12.0)))
                                .when(has_changes, |e| {
                                    e.child(
                                        div()
                                            .font_family(mono_family.clone())
                                            .text_color(p.success)
                                            .child(format!("+{total_add}")),
                                    )
                                    .child(
                                        div()
                                            .font_family(mono_family.clone())
                                            .text_color(p.danger)
                                            .child(format!("-{total_del}")),
                                    )
                                })
                                .when(!has_changes, |e| e.child("Changes")),
                        )
                        .child(
                            div()
                                .id("header-files-toggle")
                                .h(px(26.0))
                                .px_2()
                                .rounded_md()
                                .bg(if files_on {
                                    p.surface_selected
                                } else {
                                    rgba(0x00000000)
                                })
                                .hover(|s| s.bg(p.surface_hover))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_files_pane(cx);
                                }))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(if files_on { p.text } else { p.text_muted })
                                .child(Icon::new(Lucide::FileCode).size(px(12.0)))
                                .child("Files"),
                        )
                        .child(
                            div()
                                .id("header-term-toggle")
                                .h(px(26.0))
                                .px_2()
                                .rounded_md()
                                .bg(if self.show_terminal_dock {
                                    p.surface_selected
                                } else {
                                    rgba(0x00000000)
                                })
                                .hover(|s| s.bg(p.surface_hover))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_terminal_dock(Some(window), cx);
                                }))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(if self.show_terminal_dock {
                                    p.text
                                } else {
                                    p.text_muted
                                })
                                .child(Icon::new(Lucide::SquareTerminal).size(px(12.0)))
                                .child("Terminal"),
                        )
                        .child(
                            Button::new("hdr-del-thread")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(Lucide::Trash))
                                .tooltip("Delete Session")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.hosts_mgr.send(
                                        &hid,
                                        RpcCommand::DeleteThread {
                                            thread_id: tid_del.clone(),
                                        },
                                    );
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Render for HasilanxApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();

        let sidebar_el = if self.show_sidebar {
            self.render_sidebar(cx)
        } else {
            div().into_any_element()
        };
        let top_bar_el = self.render_top_bar(cx);
        let stage_el = self.render_stage(cx);
        let active_split_key = self
            .split_thread
            .clone()
            .filter(|k| self.selected.as_ref() != Some(k));
        let stage_body: AnyElement = if let Some(ref split_key) = active_split_key {
            let split_el = self.render_split_stage(split_key, cx);
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    h_resizable("center-split-arena")
                        .with_state(&self.split_resize_state)
                        .child(resizable_panel().child(div().size_full().flex().flex_col().child(stage_el)))
                        .child(
                            resizable_panel()
                                .size_range(px(280.0)..px(960.0))
                                .child(split_el),
                        ),
                )
                .into_any_element()
        } else {
            stage_el
        };
        let term_el = if self.show_terminal_dock {
            self.render_terminal_dock(window, cx)
        } else {
            div().into_any_element()
        };
        let diff_el = if self.show_diff_pane {
            self.render_diff_pane(cx)
        } else {
            div().into_any_element()
        };
        let modal_el = self.render_active_modal(cx);

        let center_vertical: AnyElement = if self.show_terminal_dock && self.terminal_maximized {
            div()
                .size_full()
                .rounded_xl()
                .border_1()
                .border_color(p.hairline)
                .bg(p.canvas)
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(top_bar_el)
                .child(div().flex_1().min_h_0().w_full().child(term_el))
                .into_any_element()
        } else {
            div()
                .size_full()
                .rounded_xl()
                .border_1()
                .border_color(p.hairline)
                .bg(p.canvas)
                .overflow_hidden()
                .child(
                    v_resizable("center-v-split")
                        .with_state(&self.v_resize_state)
                        .child(
                            resizable_panel().child({
                                let full_bleed_hero =
                                    self.is_hero_stage() && active_split_key.is_none();
                                div()
                                    .relative()
                                    .size_full()
                                    .bg(p.canvas)
                                    .overflow_hidden()
                                    .when(full_bleed_hero, |el| {
                                        el.child(backdrop::render_hero_backdrop(
                                            self.backdrop_style,
                                            p,
                                        ))
                                    })
                                    .child(
                                        div()
                                            .relative()
                                            .size_full()
                                            .flex()
                                            .flex_col()
                                            .child(top_bar_el)
                                            .child(stage_body),
                                    )
                            }),
                        )
                        .child(
                            resizable_panel()
                                .visible(self.show_terminal_dock)
                                .size(px(260.0))
                                .size_range(px(140.0)..px(620.0))
                                .child(term_el),
                        ),
                )
                .into_any_element()
        };

        // Zeron Inset Floating Card Shell: subtle window gutter around rounded-xl cards
        let main_horizontal = h_resizable("main-h-split")
            .with_state(&self.h_resize_state)
            .child(
                resizable_panel()
                    .visible(self.show_sidebar)
                    .size(px(252.0))
                    .size_range(px(208.0)..px(340.0))
                    .child(div().size_full().pr(px(4.0)).child(sidebar_el)),
            )
            .child(
                resizable_panel().size(px(900.0)).child(
                    div()
                        .size_full()
                        .when(self.show_sidebar, |el| el.pl(px(2.0)))
                        .when(self.show_diff_pane, |el| el.pr(px(2.0)))
                        .child(center_vertical),
                ),
            )
            .child(
                resizable_panel()
                    .visible(self.show_diff_pane)
                    .size(px(460.0))
                    .size_range(px(320.0)..px(760.0))
                    .child(div().size_full().pl(px(4.0)).child(diff_el)),
            );

        div()
            .relative()
            .size_full()
            .p(px(6.0))
            .bg(p.window_bg)
            .text_color(p.text)
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, window, cx| {
                if this.active_modal == ActiveModal::CommandPalette {
                    this.active_modal = ActiveModal::None;
                    cx.notify();
                } else {
                    this.open_command_palette(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleDiffPane, _, cx| {
                this.toggle_diff_pane(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleFilesPane, _, cx| {
                this.toggle_files_pane(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSessionSearch, window, cx| {
                this.toggle_session_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSplitPane, _, cx| {
                this.toggle_split_pane(cx);
            }))
            .on_action(cx.listener(|this, _: &NavigateBack, window, cx| {
                this.navigate_back(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NavigateForward, window, cx| {
                this.navigate_forward(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleTerminalDock, window, cx| {
                this.toggle_terminal_dock(Some(window), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.show_sidebar = !this.show_sidebar;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleAllTools, _, cx| {
                this.toggle_all_tool_calls(cx);
            }))
            .on_action(cx.listener(|this, _: &QuickJump1, window, cx| {
                this.quick_jump_session(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickJump2, window, cx| {
                this.quick_jump_session(2, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickJump3, window, cx| {
                this.quick_jump_session(3, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickJump4, window, cx| {
                this.quick_jump_session(4, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickJump5, window, cx| {
                this.quick_jump_session(5, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickJump6, window, cx| {
                this.quick_jump_session(6, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickJump7, window, cx| {
                this.quick_jump_session(7, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickJump8, window, cx| {
                this.quick_jump_session(8, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickJump9, window, cx| {
                this.quick_jump_session(9, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewThreadAction, window, cx| {
                if let Some((hid, pid, _)) = this.target_project_for_new_thread() {
                    this.create_thread(hid, pid, Some(window), cx);
                } else {
                    this.open_add_project_modal(LOCAL_HOST_ID.to_string(), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SendPromptAction, window, cx| {
                this.send_or_queue_prompt(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.settings_notice = None;
                if this.active_modal == ActiveModal::Settings {
                    this.active_modal = ActiveModal::None;
                } else {
                    this.active_modal = ActiveModal::Settings;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CycleTheme, _, cx| {
                let next = this.theme_preset.next();
                this.set_theme(next, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, window, cx| {
                if this.active_modal != ActiveModal::None {
                    this.active_modal = ActiveModal::None;
                } else if this.show_session_search {
                    this.show_session_search = false;
                    this.prompt_input.update(cx, |s, cx| s.focus(window, cx));
                } else if this.show_checks_drawer {
                    this.show_checks_drawer = false;
                } else {
                    this.editing_thread_title = false;
                }
                cx.notify();
            }))
            .child(main_horizontal)
            .when_some(modal_el, |el, modal| el.child(modal))
    }
}

pub(crate) fn status_color(p: &UiPalette, status: ThreadStatus) -> Rgba {
    match status {
        ThreadStatus::Idle => p.text_faint,
        ThreadStatus::Running => p.running,
        ThreadStatus::WaitingApproval => p.approval,
        ThreadStatus::Error => p.danger,
    }
}
