use super::{HasilanxApp, InspectorTab};
use crate::time_fmt::shorten_path;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::GlobalState;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Icon, Sizable, StyledExt, Theme};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use hasilanx_protocol::Attachment;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_FILES_SCAN: usize = 600;
const MAX_PREVIEW_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceFileEntry {
    pub(crate) rel_path: String,
    pub(crate) abs_path: PathBuf,
    pub(crate) size_bytes: u64,
}

#[derive(Debug, Default)]
pub(crate) struct FilesInspectorState {
    pub(crate) root_dir: Option<PathBuf>,
    pub(crate) entries: Vec<WorkspaceFileEntry>,
    pub(crate) selected_rel: Option<String>,
    pub(crate) preview_content: Option<String>,
    pub(crate) preview_lang: &'static str,
    pub(crate) preview_truncated: bool,
    pub(crate) preview_error: Option<String>,
}

impl FilesInspectorState {
    pub(crate) fn refresh_for_root(&mut self, root: &Path) {
        if self.root_dir.as_deref() == Some(root) && !self.entries.is_empty() {
            return;
        }
        self.force_refresh(root);
    }

    pub(crate) fn force_refresh(&mut self, root: &Path) {
        self.root_dir = Some(root.to_path_buf());
        self.entries = scan_workspace_files(root);
        // Preserve selection if still valid
        if let Some(ref sel) = self.selected_rel.clone() {
            if let Some(entry) = self.entries.iter().find(|e| &e.rel_path == sel).cloned() {
                self.load_preview(&entry);
                return;
            }
        }
        self.selected_rel = None;
        self.preview_content = None;
        self.preview_error = None;
    }

    pub(crate) fn select_file(&mut self, rel_path: &str) {
        if let Some(entry) = self
            .entries
            .iter()
            .find(|e| e.rel_path == rel_path)
            .cloned()
        {
            self.selected_rel = Some(entry.rel_path.clone());
            self.load_preview(&entry);
        }
    }

    fn load_preview(&mut self, entry: &WorkspaceFileEntry) {
        self.preview_lang = detect_ext_lang(&entry.abs_path);
        self.preview_truncated = false;
        self.preview_error = None;
        match fs::read(&entry.abs_path) {
            Ok(bytes) => {
                if bytes.iter().take(512).any(|&b| b == 0) {
                    self.preview_content = None;
                    self.preview_error = Some(format!(
                        "Binary file ({})",
                        format_file_size(entry.size_bytes)
                    ));
                    return;
                }
                let slice = if bytes.len() > MAX_PREVIEW_BYTES {
                    self.preview_truncated = true;
                    &bytes[..MAX_PREVIEW_BYTES]
                } else {
                    &bytes[..]
                };
                self.preview_content = Some(String::from_utf8_lossy(slice).into_owned());
            }
            Err(err) => {
                self.preview_content = None;
                self.preview_error = Some(format!("Cannot read file: {err}"));
            }
        }
    }
}

fn scan_workspace_files(root: &Path) -> Vec<WorkspaceFileEntry> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_FILES_SCAN {
            break;
        }
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        children.sort();
        for path in children {
            if out.len() >= MAX_FILES_SCAN {
                break;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if should_skip_entry(name) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(WorkspaceFileEntry {
                        rel_path: rel.to_string_lossy().replace('\\', "/"),
                        abs_path: path,
                        size_bytes: meta.len(),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

fn should_skip_entry(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".next"
            | ".nuxt"
            | "dist"
            | "build"
            | ".venv"
            | "__pycache__"
            | ".idea"
            | ".DS_Store"
    )
}

fn detect_ext_lang(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("rs") => "rust",
        Some("ts" | "tsx") => "typescript",
        Some("js" | "jsx" | "mjs" | "cjs") => "javascript",
        Some("py") => "python",
        Some("go") => "go",
        Some("toml") => "toml",
        Some("json") => "json",
        Some("yaml" | "yml") => "yaml",
        Some("md" | "markdown") => "markdown",
        Some("sh" | "bash" | "zsh") => "bash",
        Some("html" | "htm") => "html",
        Some("css" | "scss") => "css",
        Some("sql") => "sql",
        Some("c" | "h") => "c",
        Some("cpp" | "cc" | "hpp") => "cpp",
        Some("lua" | "luau") => "lua",
        _ => "",
    }
}

fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn safe_code_fence(lang: &str, content: &str) -> String {
    let mut max_run = 0usize;
    let mut current = 0usize;
    for ch in content.chars() {
        if ch == '`' {
            current += 1;
            max_run = max_run.max(current);
        } else {
            current = 0;
        }
    }
    let fence: String = std::iter::repeat_n('`', (max_run + 1).max(3)).collect();
    if lang.is_empty() {
        format!("{fence}\n{content}\n{fence}")
    } else {
        format!("{fence}{lang}\n{content}\n{fence}")
    }
}

pub(crate) fn ext_badge(rel_path: &str, is_dark: bool) -> (&'static str, Rgba, Rgba) {
    let ext = Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    match (ext.as_deref(), is_dark) {
        (Some("rs"), true) => ("RS", rgba(0xf9731622), rgb(0xfb923c)),
        (Some("rs"), false) => ("RS", rgba(0xea580c16), rgb(0xc2410c)),
        (Some("toml"), true) => ("TOML", rgba(0x8e74c826), rgb(0xc4b5fd)),
        (Some("toml"), false) => ("TOML", rgba(0x6b4ce616), rgb(0x6b4ce6)),
        (Some("ts" | "tsx"), true) => ("TS", rgba(0x3b82f624), rgb(0x60a5fa)),
        (Some("ts" | "tsx"), false) => ("TS", rgba(0x2563eb16), rgb(0x2563eb)),
        (Some("js" | "jsx"), true) => ("JS", rgba(0xeab30824), rgb(0xfacc15)),
        (Some("js" | "jsx"), false) => ("JS", rgba(0xca8a0416), rgb(0xa16207)),
        (Some("py"), true) => ("PY", rgba(0x22c55e22), rgb(0x4ade80)),
        (Some("py"), false) => ("PY", rgba(0x16a34a16), rgb(0x15803d)),
        (Some("md"), true) => ("MD", rgba(0x38bdf822), rgb(0x7dd3fc)),
        (Some("md"), false) => ("MD", rgba(0x0284c716), rgb(0x0369a1)),
        (Some("json"), true) => ("JSON", rgba(0xf59e0b22), rgb(0xfbbf24)),
        (Some("json"), false) => ("JSON", rgba(0xd9770616), rgb(0xb45309)),
        (Some("sh" | "bash" | "zsh"), true) => ("SH", rgba(0x34d39922), rgb(0x34d399)),
        (Some("sh" | "bash" | "zsh"), false) => ("SH", rgba(0x05966916), rgb(0x059669)),
        (_, true) => ("FILE", rgba(0xededef14), rgb(0x9e9ea8)),
        (_, false) => ("FILE", rgba(0x11182710), rgb(0x5a5a68)),
    }
}

impl HasilanxApp {
    pub(crate) fn active_workspace_root_str(&self) -> Option<&str> {
        let (hs, thread) = self.selected_thread()?;
        if !hs.is_local {
            return None;
        }
        if let Some(ref wt) = thread.worktree_path {
            if !wt.is_empty() {
                return Some(wt.as_str());
            }
        }
        hs.projects
            .iter()
            .find(|pr| pr.id == thread.project_id)
            .map(|pr| pr.root_path.as_str())
    }

    pub(crate) fn active_workspace_dir(&self) -> Option<PathBuf> {
        self.active_workspace_root_str().map(PathBuf::from)
    }

    pub(crate) fn sync_files_inspector(&mut self, force: bool) {
        if let Some(root) = self.active_workspace_dir() {
            if force {
                self.files_inspector.force_refresh(&root);
            } else {
                self.files_inspector.refresh_for_root(&root);
            }
        } else {
            self.files_inspector = FilesInspectorState::default();
        }
    }

    pub(crate) fn open_file_in_inspector(&mut self, raw_path: &str, cx: &mut Context<Self>) {
        let Some(root) = self.active_workspace_dir() else {
            return;
        };
        self.files_inspector.refresh_for_root(&root);
        let cleaned = raw_path
            .trim()
            .trim_start_matches("update ")
            .trim_start_matches("add ")
            .trim_start_matches("delete ")
            .split(" -> ")
            .last()
            .unwrap_or(raw_path)
            .trim();
        let candidate = Path::new(cleaned);
        let rel = if candidate.is_absolute() {
            candidate
                .strip_prefix(&root)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
        } else {
            Some(cleaned.trim_start_matches("./").replace('\\', "/"))
        };
        let Some(rel_str) = rel else {
            return;
        };

        self.show_diff_pane = true;
        self.inspector_tab = InspectorTab::Files;
        if self
            .files_inspector
            .entries
            .iter()
            .any(|e| e.rel_path == rel_str)
        {
            self.files_inspector.select_file(&rel_str);
        } else {
            // Try loading direct file even if beyond MAX_FILES_SCAN
            let abs = root.join(&rel_str);
            if abs.is_file() {
                let size_bytes = fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
                let entry = WorkspaceFileEntry {
                    rel_path: rel_str.clone(),
                    abs_path: abs,
                    size_bytes,
                };
                self.files_inspector.selected_rel = Some(rel_str);
                self.files_inspector.load_preview(&entry);
            }
        }
        cx.notify();
    }

    pub(crate) fn render_files_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let mono_family = Theme::global(cx).mono_font_family.clone();

        let Some(root_dir) = self.active_workspace_dir() else {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .text_xs()
                .text_color(p.text_faint)
                .child("File explorer is available for local workspaces.")
                .into_any_element();
        };

        let query = self.files_filter_input.read(cx).value().trim().to_lowercase();
        let filtered: Vec<WorkspaceFileEntry> = self
            .files_inspector
            .entries
            .iter()
            .filter(|e| query.is_empty() || e.rel_path.to_lowercase().contains(&query))
            .take(150)
            .cloned()
            .collect();

        let selected_rel = self.files_inspector.selected_rel.clone();
        let preview_content = self.files_inspector.preview_content.clone();
        let preview_lang = self.files_inspector.preview_lang;
        let preview_truncated = self.files_inspector.preview_truncated;
        let preview_error = self.files_inspector.preview_error.clone();
        let selected_abs = selected_rel.as_ref().and_then(|rel| {
            self.files_inspector
                .entries
                .iter()
                .find(|e| &e.rel_path == rel)
                .map(|e| e.abs_path.clone())
        });

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            // Filter bar
            .child(
                div()
                    .px_2p5()
                    .py_2()
                    .border_b_1()
                    .border_color(p.hairline)
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
                            Input::new(&self.files_filter_input)
                                .xsmall()
                                .appearance(false)
                                .bordered(false),
                        ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.text_faint)
                            .child(format!("{}", filtered.len())),
                    )
                    .child(
                        Button::new("files-refresh-btn")
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(Lucide::RefreshCw))
                            .tooltip("Rescan workspace files")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sync_files_inspector(true);
                                cx.notify();
                            })),
                    ),
            )
            // File preview (when a file is selected) OR file list
            .child(if let Some(rel) = selected_rel {
                let rel_copy = rel.clone();
                let abs_for_attach = selected_abs.clone();
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    // Breadcrumb + back + attach bar
                    .child(
                        div()
                            .px_2p5()
                            .py_1p5()
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
                                    .gap_1p5()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        Button::new("file-preview-back")
                                            .xsmall()
                                            .ghost()
                                            .icon(Icon::new(Lucide::ChevronLeft))
                                            .tooltip("Back to file list")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.files_inspector.selected_rel = None;
                                                this.files_inspector.preview_content = None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family(mono_family.clone())
                                            .text_color(p.text)
                                            .truncate()
                                            .child(rel.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .flex_shrink_0()
                                    .when_some(abs_for_attach, |el, abs_path| {
                                        let label = rel_copy.clone();
                                        let path_str = abs_path.to_string_lossy().to_string();
                                        el.child(
                                            Button::new("file-attach-btn")
                                                .xsmall()
                                                .ghost()
                                                .icon(Icon::new(Lucide::Paperclip))
                                                .label("Attach")
                                                .tooltip("Attach file to prompt")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if !this
                                                        .pending_attachments
                                                        .iter()
                                                        .any(|a| a.path == path_str)
                                                    {
                                                        this.pending_attachments.push(Attachment {
                                                            label: label.clone(),
                                                            path: path_str.clone(),
                                                        });
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                    })
                                    .when_some(preview_content.clone(), |el, body_for_copy| {
                                        let copied_code = self.is_copied("file-copy-code");
                                        el.child(
                                            Button::new("file-copy-content-btn")
                                                .xsmall()
                                                .ghost()
                                                .icon(Icon::new(if copied_code {
                                                    Lucide::Check
                                                } else {
                                                    Lucide::FileText
                                                }))
                                                .label(if copied_code { "Copied" } else { "Copy code" })
                                                .tooltip("Copy file contents to clipboard")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.copy_with_feedback(
                                                        "file-copy-code",
                                                        body_for_copy.clone(),
                                                        cx,
                                                    );
                                                })),
                                        )
                                    })
                                    .child({
                                        let copied_path = self.is_copied("file-copy-path");
                                        Button::new("file-copy-path-btn")
                                            .xsmall()
                                            .ghost()
                                            .icon(Icon::new(if copied_path {
                                                Lucide::Check
                                            } else {
                                                Lucide::Copy
                                            }))
                                            .tooltip("Copy relative path")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.copy_with_feedback(
                                                    "file-copy-path",
                                                    rel.clone(),
                                                    cx,
                                                );
                                            }))
                                    }),
                            ),
                    )
                    // Code viewer
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .p_2p5()
                            .overflow_y_scrollbar()
                            .child(if let Some(err) = preview_error {
                                div()
                                    .p_4()
                                    .text_xs()
                                    .text_color(p.text_muted)
                                    .child(err)
                                    .into_any_element()
                            } else if let Some(content) = preview_content {
                                let mut body = content;
                                if preview_truncated {
                                    body.push_str("\n\n… (preview truncated at 64 KB)");
                                }
                                let md = safe_code_fence(preview_lang, &body);
                                div()
                                    .text_xs()
                                    .child(
                                        TextView::markdown("inspector-file-preview-md", md)
                                            .selectable(true),
                                    )
                                    .into_any_element()
                            } else {
                                div()
                                    .p_4()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child("Empty file")
                                    .into_any_element()
                            }),
                    )
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .text_color(p.text_faint)
                            .truncate()
                            .child(shorten_path(&root_dir.to_string_lossy())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .px_1p5()
                            .pb_2()
                            .overflow_y_scrollbar()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .children(filtered.into_iter().enumerate().map(|(idx, entry)| {
                                let rel = entry.rel_path.clone();
                                let size_str = format_file_size(entry.size_bytes);
                                let (badge_txt, badge_bg, badge_fg) =
                                    ext_badge(&entry.rel_path, p.is_dark);
                                div()
                                    .id(SharedString::from(format!("ws-file-{idx}")))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .hover(|s| s.bg(p.surface_hover))
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        GlobalState::suppress_text_selection(cx);
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.files_inspector.select_file(&rel);
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
                                            .gap_1p5()
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
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.text_faint)
                                            .flex_shrink_0()
                                            .child(size_str),
                                    )
                            })),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }
}
