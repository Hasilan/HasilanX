use super::{ActiveModal, HasilanxApp};
use crate::terminal::{
    TermCell, TermPoint, TermSelection, TerminalBuffer, format_paste_bytes, keystroke_to_pty_bytes,
};
use crate::time_fmt::shorten_path;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::GlobalState;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Icon, Sizable, StyledExt, Theme};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use hasilanx_protocol::RpcCommand;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 8.0;

#[derive(Debug, Clone)]
pub(crate) enum TerminalImeEvent {
    Commit(String),
    PreeditChanged,
}

pub(crate) struct TerminalImeState {
    pub(crate) preedit: String,
    pub(crate) cursor_bounds: Bounds<Pixels>,
}

impl TerminalImeState {
    pub(crate) fn new() -> Self {
        Self {
            preedit: String::new(),
            cursor_bounds: Bounds::default(),
        }
    }
}

impl EventEmitter<TerminalImeEvent> for TerminalImeState {}

impl EntityInputHandler for TerminalImeState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let utf16: Vec<u16> = self.preedit.encode_utf16().collect();
        let start = range_utf16.start.min(utf16.len());
        let end = range_utf16.end.min(utf16.len()).max(start);
        *adjusted_range = Some(start..end);
        Some(String::from_utf16_lossy(&utf16[start..end]))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self.preedit.encode_utf16().count();
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        if self.preedit.is_empty() {
            None
        } else {
            Some(0..self.preedit.encode_utf16().count())
        }
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.preedit.is_empty() {
            self.preedit.clear();
            cx.emit(TerminalImeEvent::PreeditChanged);
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit.clear();
        if !text.is_empty() {
            cx.emit(TerminalImeEvent::Commit(text.to_string()));
        } else {
            cx.emit(TerminalImeEvent::PreeditChanged);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit = new_text.to_string();
        cx.emit(TerminalImeEvent::PreeditChanged);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.cursor_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.preedit.encode_utf16().count())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TerminalLayoutSnapshot {
    pub(crate) origin: Point<Pixels>,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) cell_width: Pixels,
    pub(crate) line_height: Pixels,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
}

impl Default for TerminalLayoutSnapshot {
    fn default() -> Self {
        Self {
            origin: point(px(0.0), px(0.0)),
            bounds: Bounds::default(),
            cell_width: px(8.0),
            line_height: px(18.0),
            cols: 100,
            rows: 24,
        }
    }
}

impl TerminalLayoutSnapshot {
    pub(crate) fn pixel_to_cell(
        &self,
        pos: Point<Pixels>,
        viewport_start_line: usize,
    ) -> TermPoint {
        let cw = f32::from(self.cell_width).max(1.0);
        let lh = f32::from(self.line_height).max(1.0);
        let rel_x = (f32::from(pos.x - self.origin.x)).max(0.0);
        let rel_y = (f32::from(pos.y - self.origin.y)).max(0.0);
        let col = ((rel_x / cw).floor() as usize).min(self.cols.saturating_sub(1));
        let row = ((rel_y / lh).floor() as usize).min(self.rows.saturating_sub(1));
        TermPoint {
            line: viewport_start_line + row,
            col,
        }
    }
}

struct PaintSnapshot {
    rows: Vec<(usize, Vec<TermCell>)>,
    selection: Option<TermSelection>,
    cursor: Option<(usize, usize)>,
    scroll_offset: usize,
    total_lines: usize,
    visible_rows: usize,
    hovered_url: Option<(usize, usize, usize)>, // (abs_line, col_start, col_end)
    preedit: String,
    is_focused: bool,
    is_dark: bool,
    font_family: SharedString,
    font_size: f32,
    terminal_bg: Rgba,
    default_fg: Rgba,
    cursor_bg: Rgba,
    cursor_fg: Rgba,
    selection_bg: Hsla,
    unfocused_cursor_border: Hsla,
    scrollbar_color: Hsla,
    preedit_bg: Rgba,
    accent: Rgba,
}

impl HasilanxApp {
    pub(crate) fn active_terminal_mut(&mut self) -> Option<&mut TerminalBuffer> {
        let key = self.selected.as_ref()?;
        self.host_states
            .get_mut(&key.host_id)
            .map(|h| h.terminals.entry(key.thread_id.clone()).or_default())
    }

    pub(crate) fn active_terminal(&self) -> Option<&TerminalBuffer> {
        let key = self.selected.as_ref()?;
        self.host_states
            .get(&key.host_id)
            .and_then(|h| h.terminals.get(&key.thread_id))
    }

    pub(crate) fn sync_terminal_size(&mut self, cols: usize, rows: usize, cx: &mut Context<Self>) {
        let cols = cols.clamp(20, 400);
        let rows = rows.clamp(4, 150);
        let Some(key) = self.selected.clone() else {
            return;
        };
        let resized = self
            .host_states
            .get_mut(&key.host_id)
            .map(|h| {
                h.terminals
                    .entry(key.thread_id.clone())
                    .or_default()
                    .resize(cols, rows)
            })
            .unwrap_or(false);

        if resized {
            self.hosts_mgr.send(
                &key.host_id,
                RpcCommand::TerminalResize {
                    thread_id: key.thread_id,
                    cols: cols as u16,
                    rows: rows as u16,
                },
            );
            cx.notify();
        }
    }

    pub(crate) fn restart_terminal(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let (cols, rows) = {
            let layout = self.terminal_layout.borrow();
            (layout.cols.clamp(20, 400), layout.rows.clamp(4, 150))
        };
        if let Some(h) = self.host_states.get_mut(&key.host_id) {
            let buf = TerminalBuffer::with_size(cols, rows);
            h.terminals.insert(key.thread_id.clone(), buf);
        }
        self.hosts_mgr.send(
            &key.host_id,
            RpcCommand::TerminalClose {
                thread_id: key.thread_id.clone(),
            },
        );
        self.hosts_mgr.send(
            &key.host_id,
            RpcCommand::TerminalOpen {
                thread_id: key.thread_id,
                cols: cols as u16,
                rows: rows as u16,
            },
        );
        if let Some(w) = window {
            self.terminal_focus.focus(w, cx);
        }
        cx.notify();
    }

    pub(crate) fn paste_to_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let bracketed = self
            .active_terminal()
            .map(|b| b.bracketed_paste())
            .unwrap_or(false);
        if let Some(buf) = self.active_terminal_mut() {
            buf.scroll_to_bottom();
            buf.clear_selection();
        }
        let bytes = format_paste_bytes(&text, bracketed);
        self.send_terminal_bytes(&bytes, cx);
    }

    pub(crate) fn copy_from_terminal(&mut self, cx: &mut Context<Self>) {
        let text = self.active_terminal().and_then(|b| {
            b.selected_text().or_else(|| {
                let all = b.plain_text();
                (!all.is_empty()).then_some(all)
            })
        });
        if let Some(t) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(t));
            self.terminal_copied_until = Some(Instant::now() + Duration::from_millis(1400));
            cx.notify();
        }
    }

    pub(crate) fn zoom_terminal(&mut self, delta: f32, reset: bool, cx: &mut Context<Self>) {
        if reset {
            self.set_terminal_font_size(13.0, cx);
        } else {
            self.set_terminal_font_size(self.terminal_font_size + delta, cx);
        }
    }

    pub(crate) fn send_terminal_to_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let excerpt = self.active_terminal().and_then(|b| {
            b.selected_text().or_else(|| {
                let all = b.plain_text();
                let tail = extract_terminal_tail(&all, 40);
                (!tail.is_empty()).then_some(tail)
            })
        });
        let Some(snippet) = excerpt else {
            return;
        };
        let current = self.prompt_input.read(cx).value().to_string();
        let next = if current.trim().is_empty() {
            format!("Please check this terminal output:\n```\n{snippet}\n```\n")
        } else {
            format!("{}\n\n```\n{snippet}\n```\n", current.trim_end())
        };
        self.prompt_input.update(cx, |s, cx| {
            s.set_value(next, window, cx);
            s.focus(window, cx);
        });
        if self.terminal_maximized {
            self.terminal_maximized = false;
        }
        cx.notify();
    }

    pub(crate) fn handle_terminal_key_down(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_modal != ActiveModal::None {
            return;
        }

        // If shell exited, pressing Enter immediately restarts it
        let exited = self
            .active_terminal()
            .is_some_and(|b| b.exited_code.is_some());
        if exited && ev.keystroke.key == "enter" {
            self.restart_terminal(Some(window), cx);
            cx.stop_propagation();
            return;
        }

        let ks = &ev.keystroke;

        // Font zoom shortcuts: Ctrl+= / Ctrl+- / Ctrl+0
        if ks.modifiers.control && !ks.modifiers.alt && !ks.modifiers.shift {
            match ks.key.as_str() {
                "=" | "+" => {
                    self.zoom_terminal(1.0, false, cx);
                    cx.stop_propagation();
                    return;
                }
                "-" => {
                    self.zoom_terminal(-1.0, false, cx);
                    cx.stop_propagation();
                    return;
                }
                "0" => {
                    self.zoom_terminal(0.0, true, cx);
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }

        // Clipboard paste: Ctrl+Shift+V or Cmd+V
        let is_paste = (ks.modifiers.control && ks.modifiers.shift && ks.key == "v")
            || (ks.modifiers.platform && ks.key == "v");
        if is_paste {
            self.paste_to_terminal(cx);
            cx.stop_propagation();
            return;
        }

        // Clipboard copy: Ctrl+Shift+C or Cmd+C
        let is_copy = (ks.modifiers.control && ks.modifiers.shift && ks.key == "c")
            || (ks.modifiers.platform && ks.key == "c");
        if is_copy {
            self.copy_from_terminal(cx);
            cx.stop_propagation();
            return;
        }

        // Scrollback navigation: Shift+PageUp / Shift+PageDown / Shift+Home / Shift+End
        if ks.modifiers.shift && !ks.modifiers.control && !ks.modifiers.alt {
            let page = self
                .active_terminal()
                .map(|b| (b.rows().saturating_sub(2)).max(1) as isize)
                .unwrap_or(10);
            match ks.key.as_str() {
                "pageup" => {
                    if let Some(buf) = self.active_terminal_mut() {
                        buf.scroll_by(page);
                    }
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                "pagedown" => {
                    if let Some(buf) = self.active_terminal_mut() {
                        buf.scroll_by(-page);
                    }
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                "home" => {
                    if let Some(buf) = self.active_terminal_mut() {
                        buf.scroll_to_top();
                    }
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                "end" => {
                    if let Some(buf) = self.active_terminal_mut() {
                        buf.scroll_to_bottom();
                    }
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }

        // While Japanese/CJK IME preedit is active, let IME handle keys
        if !self.terminal_ime.read(cx).preedit.is_empty() {
            return;
        }

        let app_cursor = self
            .active_terminal()
            .map(|b| b.app_cursor_mode())
            .unwrap_or(false);

        if let Some(bytes) = keystroke_to_pty_bytes(ks, app_cursor, true) {
            if let Some(buf) = self.active_terminal_mut() {
                buf.scroll_to_bottom();
                buf.clear_selection();
            }
            self.send_terminal_bytes(&bytes, cx);
            cx.stop_propagation();
        }
    }

    pub(crate) fn render_terminal_dock(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let Some(key) = self.selected.clone() else {
            return div()
                .size_full()
                .bg(p.terminal_bg)
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(p.text_faint)
                .child("Select a thread to open an interactive terminal.")
                .into_any_element();
        };

        let is_focused = self.terminal_focus.is_focused(window);
        let is_local = self
            .host_states
            .get(&key.host_id)
            .map(|h| h.is_local)
            .unwrap_or(true);
        let host_label = self
            .host_states
            .get(&key.host_id)
            .map(|h| h.label.clone())
            .unwrap_or_else(|| key.host_id.clone());

        let buf_opt = self
            .host_states
            .get(&key.host_id)
            .and_then(|h| h.terminals.get(&key.thread_id));

        let (cols, rows, scroll_offset, total_lines, exited_code, has_selection, header_title) =
            if let Some(b) = buf_opt {
                let title_str = b
                    .title()
                    .map(ToOwned::to_owned)
                    .or_else(|| b.cwd().map(shorten_path))
                    .unwrap_or_else(|| {
                        if is_local {
                            "Terminal".to_string()
                        } else {
                            format!("Terminal · {host_label}")
                        }
                    });
                (
                    b.cols(),
                    b.rows(),
                    b.scroll_offset(),
                    b.total_lines(),
                    b.exited_code,
                    b.has_selection(),
                    title_str,
                )
            } else {
                (
                    self.terminal_layout.borrow().cols,
                    self.terminal_layout.borrow().rows,
                    0,
                    0,
                    None,
                    false,
                    if is_local {
                        "Terminal".to_string()
                    } else {
                        format!("Terminal · {host_label}")
                    },
                )
            };

        let copied_recently = self
            .terminal_copied_until
            .is_some_and(|until| Instant::now() < until);

        // Snapshot visible grid rows for zero-lock GPU canvas painting
        let paint_rows: Vec<(usize, Vec<TermCell>)> = if let Some(b) = buf_opt {
            let start = b.viewport_start_line();
            (0..b.rows())
                .map(|r| {
                    let abs_line = start + r;
                    let cells = b
                        .line_cells(abs_line)
                        .map(|s| s.to_vec())
                        .unwrap_or_else(|| vec![TermCell::default(); b.cols()]);
                    (abs_line, cells)
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut selection_bg: Hsla = p.accent.into();
        selection_bg.a = 0.34;
        let mut unfocused_cursor_border: Hsla = p.text_muted.into();
        unfocused_cursor_border.a = 0.75;
        let mut scrollbar_color: Hsla = p.text_muted.into();
        scrollbar_color.a = 0.38;

        let mono_family = Theme::global(cx).mono_font_family.clone();

        let snapshot = PaintSnapshot {
            rows: paint_rows,
            selection: buf_opt.and_then(|b| b.selection),
            cursor: buf_opt.and_then(|b| b.viewport_cursor()),
            scroll_offset,
            total_lines,
            visible_rows: rows,
            hovered_url: self.terminal_hovered_url.as_ref().map(|(l, c0, c1, _)| (*l, *c0, *c1)),
            preedit: self.terminal_ime.read(cx).preedit.clone(),
            is_focused,
            is_dark: p.is_dark,
            font_family: mono_family.clone(),
            font_size: self.terminal_font_size,
            terminal_bg: p.terminal_bg,
            default_fg: if p.is_dark {
                rgb(0xe4e4e7)
            } else {
                rgb(0x1f1f29)
            },
            cursor_bg: p.accent,
            cursor_fg: p.accent_fg,
            selection_bg,
            unfocused_cursor_border,
            scrollbar_color,
            preedit_bg: p.surface_elevated,
            accent: p.accent,
        };

        let layout_rc = Rc::clone(&self.terminal_layout);
        let app_weak = cx.entity().downgrade();
        let focus_for_paint = self.terminal_focus.clone();
        let ime_for_prepaint = self.terminal_ime.clone();
        let ime_for_paint = self.terminal_ime.clone();
        let cur_cols = cols;
        let cur_rows = rows;

        let term_canvas = canvas(
            move |bounds, window, cx| {
                let font = monospace_font(&snapshot.font_family, false, false);
                let font_size = px(snapshot.font_size);
                let font_id = window.text_system().resolve_font(&font);
                let cell_w = window
                    .text_system()
                    .advance(font_id, font_size, '0')
                    .map(|s| s.width)
                    .unwrap_or_else(|_| px(snapshot.font_size * 0.6));
                let cell_w = px(f32::from(cell_w).max(5.0));
                let line_h = px((snapshot.font_size * 1.40).round().max(15.0));

                let inner_w = (f32::from(bounds.size.width) - PAD_X * 2.0 - 8.0).max(120.0);
                let inner_h = (f32::from(bounds.size.height) - PAD_Y * 2.0).max(40.0);
                let measured_cols = ((inner_w / f32::from(cell_w)).floor() as usize).clamp(20, 400);
                let measured_rows = ((inner_h / f32::from(line_h)).floor() as usize).clamp(4, 150);

                let origin = point(bounds.origin.x + px(PAD_X), bounds.origin.y + px(PAD_Y));

                {
                    let mut snap = layout_rc.borrow_mut();
                    snap.origin = origin;
                    snap.bounds = bounds;
                    snap.cell_width = cell_w;
                    snap.line_height = line_h;
                    snap.cols = measured_cols;
                    snap.rows = measured_rows;
                }

                // Update IME cursor rect so Fcitx5/Mozc popup floats right beneath the terminal cursor
                let cursor_rect = if let Some((cr, cc)) = snapshot.cursor {
                    Bounds::new(
                        point(
                            origin.x + px(cc as f32 * f32::from(cell_w)),
                            origin.y + px(cr as f32 * f32::from(line_h)),
                        ),
                        size(cell_w, line_h),
                    )
                } else {
                    Bounds::new(origin, size(cell_w, line_h))
                };
                ime_for_prepaint.update(cx, |ime, _| {
                    ime.cursor_bounds = cursor_rect;
                });

                // Automatically resize PTY whenever viewport grid dimensions change
                if measured_cols != cur_cols || measured_rows != cur_rows {
                    let weak = app_weak.clone();
                    cx.defer(move |cx| {
                        let _ = weak.update(cx, |app, cx| {
                            app.sync_terminal_size(measured_cols, measured_rows, cx);
                        });
                    });
                }

                (snapshot, origin, cell_w, line_h)
            },
            move |bounds, (snap, origin, cell_w, line_h), window, cx| {
                // Register native OS / Wayland IME handler whenever painted
                window.handle_input(
                    &focus_for_paint,
                    ElementInputHandler::new(bounds, ime_for_paint.clone()),
                    cx,
                );

                // Background
                window.paint_quad(fill(bounds, snap.terminal_bg));

                let cw = f32::from(cell_w);
                let lh = f32::from(line_h);
                let font_size = px(snap.font_size);

                if snap.rows.is_empty() {
                    let msg = "Starting interactive shell...";
                    let run = TextRun {
                        len: msg.len(),
                        font: monospace_font(&snap.font_family, false, false),
                        color: rgb(0x64748b).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let shaped = window.text_system().shape_line(
                        SharedString::from(msg),
                        font_size,
                        &[run],
                        None,
                    );
                    let _ = shaped.paint(origin, line_h, TextAlign::Left, None, window, cx);
                    return;
                }

                let sel_norm = snap
                    .selection
                    .filter(|s| !s.is_empty())
                    .map(|s| s.normalized());

                for (vis_r, (abs_line, cells)) in snap.rows.iter().enumerate() {
                    let row_y = origin.y + px(vis_r as f32 * lh);

                    // 1. Paint cell backgrounds & selection highlights (merged runs)
                    let mut col = 0usize;
                    while col < cells.len() {
                        let is_sel = sel_norm.is_some_and(|(s, e)| {
                            let pt = TermPoint {
                                line: *abs_line,
                                col,
                            };
                            pt >= s && pt <= e
                        });
                        let bg_hsla: Option<Hsla> = if is_sel {
                            Some(snap.selection_bg)
                        } else {
                            cells[col].bg.map(Into::into)
                        };
                        if let Some(bg_col) = bg_hsla {
                            let start_c = col;
                            col += 1;
                            while col < cells.len() {
                                let next_sel = sel_norm.is_some_and(|(s, e)| {
                                    let pt = TermPoint {
                                        line: *abs_line,
                                        col,
                                    };
                                    pt >= s && pt <= e
                                });
                                let next_bg: Option<Hsla> = if next_sel {
                                    Some(snap.selection_bg)
                                } else {
                                    cells[col].bg.map(Into::into)
                                };
                                if next_bg == Some(bg_col) {
                                    col += 1;
                                } else {
                                    break;
                                }
                            }
                            let rect = Bounds::new(
                                point(origin.x + px(start_c as f32 * cw), row_y),
                                size(px((col - start_c) as f32 * cw), line_h),
                            );
                            window.paint_quad(fill(rect, bg_col));
                        } else {
                            col += 1;
                        }
                    }

                    // 2. Paint text runs (batch contiguous narrow runs; anchor wide CJK glyphs individually)
                    let mut c = 0usize;
                    while c < cells.len() {
                        let cell = cells[c];
                        if cell.wide_spacer || cell.hidden || (cell.ch == ' ' && !cell.underline && !cell.strikethrough) {
                            c += 1;
                            continue;
                        }
                        let is_hovered_url = snap.hovered_url.is_some_and(|(ul, c0, c1)| {
                            ul == *abs_line && c >= c0 && c <= c1
                        });

                        if cell.wide {
                            let mut color: Hsla = if is_hovered_url {
                                snap.accent.into()
                            } else {
                                cell.fg.unwrap_or(snap.default_fg).into()
                            };
                            if !snap.is_dark && cell.fg.is_some() && cell.bg.is_none() {
                                color.l = color.l.min(0.42);
                            }
                            if cell.dim {
                                color.a *= 0.55;
                            }
                            let s = cell.ch.to_string();
                            let run = TextRun {
                                len: s.len(),
                                font: monospace_font(&snap.font_family, cell.bold, cell.italic),
                                color,
                                background_color: None,
                                underline: (cell.underline || is_hovered_url).then(|| {
                                    UnderlineStyle {
                                        color: Some(color),
                                        thickness: px(1.0),
                                        wavy: false,
                                    }
                                }),
                                strikethrough: cell.strikethrough.then(|| StrikethroughStyle {
                                    color: Some(color),
                                    thickness: px(1.0),
                                }),
                            };
                            let shaped = window.text_system().shape_line(
                                SharedString::from(s),
                                font_size,
                                &[run],
                                None,
                            );
                            let _ = shaped.paint(
                                point(origin.x + px(c as f32 * cw), row_y),
                                line_h,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                            c += 1;
                            continue;
                        }

                        // Collect a narrow span with identical visual styling
                        let start_c = c;
                        let mut text = String::new();
                        text.push(cell.ch);
                        c += 1;
                        while c < cells.len() {
                            let next = cells[c];
                            let next_url = snap.hovered_url.is_some_and(|(ul, c0, c1)| {
                                ul == *abs_line && c >= c0 && c <= c1
                            });
                            if !next.wide
                                && !next.wide_spacer
                                && !next.hidden
                                && next.fg == cell.fg
                                && next.bold == cell.bold
                                && next.dim == cell.dim
                                && next.italic == cell.italic
                                && next.underline == cell.underline
                                && next.strikethrough == cell.strikethrough
                                && next_url == is_hovered_url
                            {
                                text.push(next.ch);
                                c += 1;
                            } else {
                                break;
                            }
                        }

                        let trimmed_len = text.trim_end_matches(' ').len();
                        let draw_text = if cell.underline || cell.strikethrough || is_hovered_url {
                            text
                        } else if trimmed_len == 0 {
                            continue;
                        } else {
                            text[..trimmed_len].to_string()
                        };

                        let mut color: Hsla = if is_hovered_url {
                            snap.accent.into()
                        } else {
                            cell.fg.unwrap_or(snap.default_fg).into()
                        };
                        if !snap.is_dark && cell.fg.is_some() && cell.bg.is_none() {
                            color.l = color.l.min(0.42);
                        }
                        if cell.dim {
                            color.a *= 0.55;
                        }
                        let run = TextRun {
                            len: draw_text.len(),
                            font: monospace_font(&snap.font_family, cell.bold, cell.italic),
                            color,
                            background_color: None,
                            underline: (cell.underline || is_hovered_url).then(|| UnderlineStyle {
                                color: Some(color),
                                thickness: px(1.0),
                                wavy: false,
                            }),
                            strikethrough: cell.strikethrough.then(|| StrikethroughStyle {
                                color: Some(color),
                                thickness: px(1.0),
                            }),
                        };
                        let shaped = window.text_system().shape_line(
                            SharedString::from(draw_text),
                            font_size,
                            &[run],
                            None,
                        );
                        let _ = shaped.paint(
                            point(origin.x + px(start_c as f32 * cw), row_y),
                            line_h,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }

                    // 3. Paint cursor & live Japanese IME preedit on cursor row
                    if let Some((cr, cc)) = snap.cursor {
                        if cr == vis_r {
                            let cur_cell = cells.get(cc).copied().unwrap_or_default();
                            let cur_w = if cur_cell.wide { cw * 2.0 } else { cw };
                            let cur_origin = point(origin.x + px(cc as f32 * cw), row_y);
                            let cur_rect = Bounds::new(cur_origin, size(px(cur_w), line_h));

                            if snap.is_focused {
                                if !snap.preedit.is_empty() {
                                    // Render live IME composition text right at cursor
                                    let run = TextRun {
                                        len: snap.preedit.len(),
                                        font: monospace_font(&snap.font_family, false, false),
                                        color: snap.default_fg.into(),
                                        background_color: None,
                                        underline: Some(UnderlineStyle {
                                            color: Some(snap.accent.into()),
                                            thickness: px(1.5),
                                            wavy: false,
                                        }),
                                        strikethrough: None,
                                    };
                                    let shaped = window.text_system().shape_line(
                                        SharedString::from(snap.preedit.clone()),
                                        font_size,
                                        &[run],
                                        None,
                                    );
                                    let pre_rect = Bounds::new(
                                        cur_origin,
                                        size(shaped.width + px(4.0), line_h),
                                    );
                                    window.paint_quad(fill(pre_rect, snap.preedit_bg));
                                    let _ = shaped.paint(
                                        point(cur_origin.x + px(2.0), row_y),
                                        line_h,
                                        TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                } else {
                                    window.paint_quad(fill(cur_rect, snap.cursor_bg));
                                    if !cur_cell.wide_spacer && cur_cell.ch != ' ' && !cur_cell.hidden {
                                        let ch_str = cur_cell.ch.to_string();
                                        let run = TextRun {
                                            len: ch_str.len(),
                                            font: monospace_font(
                                                &snap.font_family,
                                                cur_cell.bold,
                                                cur_cell.italic,
                                            ),
                                            color: snap.cursor_fg.into(),
                                            background_color: None,
                                            underline: None,
                                            strikethrough: None,
                                        };
                                        let shaped = window.text_system().shape_line(
                                            SharedString::from(ch_str),
                                            font_size,
                                            &[run],
                                            None,
                                        );
                                        let _ = shaped.paint(
                                            cur_origin,
                                            line_h,
                                            TextAlign::Left,
                                            None,
                                            window,
                                            cx,
                                        );
                                    }
                                }
                            } else {
                                window.paint_quad(outline(
                                    cur_rect,
                                    snap.unfocused_cursor_border,
                                    BorderStyle::Solid,
                                ));
                            }
                        }
                    }
                }

                // 4. Sleek vertical scrollbar thumb when scrollback exists
                if snap.total_lines > snap.visible_rows && snap.visible_rows > 0 {
                    let track_h = (f32::from(bounds.size.height) - 8.0).max(20.0);
                    let ratio = (snap.visible_rows as f32 / snap.total_lines as f32).clamp(0.08, 1.0);
                    let thumb_h = (track_h * ratio).max(20.0);
                    let max_scroll = (snap.total_lines - snap.visible_rows).max(1) as f32;
                    let from_top = 1.0 - (snap.scroll_offset as f32 / max_scroll).clamp(0.0, 1.0);
                    let thumb_y =
                        f32::from(bounds.origin.y) + 4.0 + from_top * (track_h - thumb_h);
                    let thumb_x = f32::from(bounds.origin.x + bounds.size.width) - 8.0;
                    let mut quad = fill(
                        Bounds::new(point(px(thumb_x), px(thumb_y)), size(px(4.0), px(thumb_h))),
                        snap.scrollbar_color,
                    );
                    quad.corner_radii = px(2.0).into();
                    window.paint_quad(quad);
                }
            },
        )
        .size_full();

        let hid_clear = key.host_id.clone();
        let tid_clear = key.thread_id.clone();
        let hid_kill = key.host_id.clone();
        let tid_kill = key.thread_id.clone();
        let is_maximized = self.terminal_maximized;
        let has_hovered_url = self.terminal_hovered_url.is_some();

        div()
            .size_full()
            .bg(p.terminal_bg)
            .border_t_1()
            .border_color(if is_focused { p.accent } else { p.hairline })
            .flex()
            .flex_col()
            // Compact 32px terminal header bar
            .child(
                div()
                    .h(px(32.0))
                    .px_3()
                    .border_b_1()
                    .border_color(p.hairline)
                    .bg(p.sidebar)
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
                                    .bg(if exited_code.is_some() {
                                        p.danger
                                    } else {
                                        p.success
                                    }),
                            )
                            .child(
                                Icon::new(Lucide::Terminal)
                                    .size(px(12.0))
                                    .text_color(p.text_muted)
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(p.text)
                                    .truncate()
                                    .child(header_title),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(mono_family)
                                    .text_color(p.text_faint)
                                    .flex_shrink_0()
                                    .child(format!("{cols}×{rows}")),
                            )
                            .when(scroll_offset > 0, |el| {
                                el.child(
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(p.surface)
                                        .text_xs()
                                        .text_color(p.accent)
                                        .flex_shrink_0()
                                        .child(format!("↑{scroll_offset}")),
                                )
                            })
                            .when(!is_focused && exited_code.is_none(), |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(p.text_faint)
                                        .truncate()
                                        .child("Click to type"),
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
                                Button::new("term-zoom-out")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::Minus))
                                    .tooltip("Decrease font size (Ctrl+-)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.zoom_terminal(-1.0, false, cx);
                                        this.terminal_focus.focus(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("term-zoom-in")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::Plus))
                                    .tooltip("Increase font size (Ctrl+=)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.zoom_terminal(1.0, false, cx);
                                        this.terminal_focus.focus(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("term-ask-ai")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::Sparkles))
                                    .label("Ask AI")
                                    .tooltip("Send selected output (or recent lines) to chat composer")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send_terminal_to_composer(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("term-ctrlc")
                                    .xsmall()
                                    .ghost()
                                    .label("^C")
                                    .tooltip("Send SIGINT (Ctrl+C)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send_terminal_bytes(&[0x03], cx);
                                        this.terminal_focus.focus(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("term-paste")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::ClipboardPaste))
                                    .tooltip("Paste (Ctrl+Shift+V / Middle-click)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.paste_to_terminal(cx);
                                        this.terminal_focus.focus(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("term-copy")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(if copied_recently {
                                        Lucide::Check
                                    } else {
                                        Lucide::Copy
                                    }))
                                    .when(copied_recently || has_selection, |b| {
                                        b.label(if copied_recently {
                                            "Copied"
                                        } else {
                                            "Copy"
                                        })
                                    })
                                    .tooltip("Copy selection or output (Ctrl+Shift+C)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.copy_from_terminal(cx);
                                        this.terminal_focus.focus(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("term-clear")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::Eraser))
                                    .tooltip("Clear scrollback (Ctrl+L)")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(h) = this.host_states.get_mut(&hid_clear) {
                                            if let Some(buf) = h.terminals.get_mut(&tid_clear) {
                                                buf.clear();
                                            }
                                        }
                                        this.send_terminal_bytes(&[0x0c], cx);
                                        this.terminal_focus.focus(window, cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("term-restart")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::RotateCw))
                                    .tooltip("Restart shell")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.restart_terminal(Some(window), cx);
                                    })),
                            )
                            .child(
                                Button::new("term-maximize")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(if is_maximized {
                                        Lucide::Minimize2
                                    } else {
                                        Lucide::Maximize2
                                    }))
                                    .tooltip(if is_maximized {
                                        "Restore height"
                                    } else {
                                        "Expand height"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.terminal_maximized = !this.terminal_maximized;
                                        this.terminal_focus.focus(window, cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("term-hide")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::ChevronDown))
                                    .tooltip("Hide panel (Ctrl+` — keeps shell running)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_terminal_dock = false;
                                        this.prompt_input
                                            .update(cx, |s, cx| s.focus(window, cx));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("term-kill")
                                    .xsmall()
                                    .ghost()
                                    .icon(Icon::new(Lucide::X))
                                    .tooltip("Terminate shell and close")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.show_terminal_dock = false;
                                        this.terminal_maximized = false;
                                        if let Some(h) = this.host_states.get_mut(&hid_kill) {
                                            h.terminals.remove(&tid_kill);
                                        }
                                        this.hosts_mgr.send(
                                            &hid_kill,
                                            RpcCommand::TerminalClose {
                                                thread_id: tid_kill.clone(),
                                            },
                                        );
                                        this.prompt_input
                                            .update(cx, |s, cx| s.focus(window, cx));
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            // GPU-rendered interactive terminal canvas with selection, IME, scrollback, URL open
            .child(
                div()
                    .id("term-interactive-viewport")
                    .key_context("Terminal")
                    .track_focus(&self.terminal_focus)
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .when(has_hovered_url, |el| el.cursor_pointer())
                    .when(!has_hovered_url, |el| el.cursor_text())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            GlobalState::suppress_text_selection(cx);
                            this.terminal_focus.focus(window, cx);

                            let layout = this.terminal_layout.borrow().clone();
                            let start_line = this
                                .active_terminal()
                                .map(|b| b.viewport_start_line())
                                .unwrap_or(0);
                            let pt = layout.pixel_to_cell(ev.position, start_line);

                            // Ctrl+Click opens URL in default browser
                            if ev.modifiers.control || ev.modifiers.platform {
                                if let Some((_, _, url)) = this
                                    .active_terminal()
                                    .and_then(|b| b.url_at(pt.line, pt.col))
                                {
                                    open_external_url(&url);
                                    cx.stop_propagation();
                                    return;
                                }
                            }

                            if let Some(buf) = this.active_terminal_mut() {
                                match ev.click_count {
                                    2 => {
                                        buf.select_word_at(pt.line, pt.col);
                                        this.terminal_selecting = false;
                                    }
                                    3 => {
                                        buf.select_line_at(pt.line);
                                        this.terminal_selecting = false;
                                    }
                                    _ => {
                                        buf.selection = Some(TermSelection {
                                            anchor: pt,
                                            head: pt,
                                        });
                                        this.terminal_selecting = true;
                                    }
                                }
                            }
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|this, _, window, cx| {
                            GlobalState::suppress_text_selection(cx);
                            this.terminal_focus.focus(window, cx);
                            this.paste_to_terminal(cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        let layout = this.terminal_layout.borrow().clone();
                        let start_line = this
                            .active_terminal()
                            .map(|b| b.viewport_start_line())
                            .unwrap_or(0);
                        let pt = layout.pixel_to_cell(ev.position, start_line);

                        if this.terminal_selecting {
                            if ev.pressed_button != Some(MouseButton::Left) {
                                this.terminal_selecting = false;
                            } else {
                                // Gentle auto-scroll when dragging past top/bottom edge
                                let top_y = f32::from(layout.bounds.origin.y);
                                let bot_y = top_y + f32::from(layout.bounds.size.height);
                                let py = f32::from(ev.position.y);
                                if let Some(buf) = this.active_terminal_mut() {
                                    if py < top_y + 8.0 {
                                        buf.scroll_by(1);
                                    } else if py > bot_y - 8.0 {
                                        buf.scroll_by(-1);
                                    }
                                    let updated_pt =
                                        layout.pixel_to_cell(ev.position, buf.viewport_start_line());
                                    if let Some(ref mut sel) = buf.selection {
                                        sel.head = updated_pt;
                                    }
                                }
                                cx.notify();
                            }
                        }

                        // Highlight URL under cursor when Ctrl/Cmd is held
                        let next_url = if ev.modifiers.control || ev.modifiers.platform {
                            this.active_terminal()
                                .and_then(|b| b.url_at(pt.line, pt.col))
                                .map(|(c0, c1, url)| (pt.line, c0, c1, url))
                        } else {
                            None
                        };
                        if next_url != this.terminal_hovered_url {
                            this.terminal_hovered_url = next_url;
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.terminal_selecting {
                                this.terminal_selecting = false;
                                // Clear zero-width single-click selection
                                if let Some(buf) = this.active_terminal_mut() {
                                    if buf.selection.is_some_and(|s| s.is_empty()) {
                                        buf.clear_selection();
                                    }
                                }
                                cx.notify();
                            }
                        }),
                    )
                    .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                        let line_h = f32::from(this.terminal_layout.borrow().line_height).max(14.0);
                        let delta_lines: isize = match ev.delta {
                            ScrollDelta::Lines(pt) => (pt.y * 3.0).round() as isize,
                            ScrollDelta::Pixels(pt) => {
                                let lines = f32::from(pt.y) / line_h;
                                if lines.abs() < 1.0 && lines != 0.0 {
                                    if lines > 0.0 { 1 } else { -1 }
                                } else {
                                    lines.round() as isize
                                }
                            }
                        };
                        if delta_lines == 0 {
                            return;
                        }

                        let (in_alt, app_cursor) = this
                            .active_terminal()
                            .map(|b| (b.in_alt_screen(), b.app_cursor_mode()))
                            .unwrap_or((false, false));

                        if in_alt {
                            // Forward wheel scrolling as Up/Down arrows in TUIs like `less`/`vim`/`git log`
                            let count = delta_lines.unsigned_abs().min(5);
                            let seq: &[u8] = if delta_lines > 0 {
                                if app_cursor { b"\x1bOA" } else { b"\x1b[A" }
                            } else if app_cursor {
                                b"\x1bOB"
                            } else {
                                b"\x1b[B"
                            };
                            let mut payload = Vec::with_capacity(seq.len() * count);
                            for _ in 0..count {
                                payload.extend_from_slice(seq);
                            }
                            this.send_terminal_bytes(&payload, cx);
                        } else if let Some(buf) = this.active_terminal_mut() {
                            buf.scroll_by(delta_lines);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }))
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        this.handle_terminal_key_down(ev, window, cx);
                    }))
                    .child(term_canvas)
                    // Floating "Jump to bottom" pill when scrolled up into history
                    .when(scroll_offset > 0, |el| {
                        el.child(
                            div()
                                .id("term-jump-bottom")
                                .absolute()
                                .right_4()
                                .bottom_3()
                                .px_2p5()
                                .py_1()
                                .rounded_full()
                                .border_1()
                                .border_color(p.hairline_strong)
                                .bg(p.surface_elevated)
                                .hover(|s| s.bg(p.surface_hover))
                                .shadow_md()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    GlobalState::suppress_text_selection(cx);
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(buf) = this.active_terminal_mut() {
                                        buf.scroll_to_bottom();
                                    }
                                    this.terminal_focus.focus(window, cx);
                                    cx.notify();
                                }))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(p.text)
                                .child(Icon::new(Lucide::ArrowDown).size(px(11.0)))
                                .child("Bottom"),
                        )
                    })
                    // Process exited banner
                    .when_some(exited_code, |el, code| {
                        el.child(
                            div()
                                .absolute()
                                .left_3()
                                .right_3()
                                .bottom_2p5()
                                .px_3()
                                .py_1p5()
                                .rounded_lg()
                                .border_1()
                                .border_color(p.hairline_strong)
                                .bg(p.surface_elevated)
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(p.text_muted)
                                        .child(format!(
                                            "Shell exited (code {code}) · Press Enter to restart"
                                        )),
                                )
                                .child(
                                    Button::new("term-banner-restart")
                                        .xsmall()
                                        .primary()
                                        .icon(Icon::new(Lucide::RotateCw))
                                        .label("Restart")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.restart_terminal(Some(window), cx);
                                        })),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}

fn monospace_font(family: &SharedString, bold: bool, italic: bool) -> Font {
    let mut f = font(family.clone());
    if bold {
        f.weight = FontWeight::BOLD;
    }
    if italic {
        f.style = FontStyle::Italic;
    }
    f
}

fn open_external_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(target_os = "macos"))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

pub(crate) fn extract_terminal_tail(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end())
        .collect();
    let last_non_empty = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    let trimmed = &lines[..last_non_empty];
    let start = trimmed.len().saturating_sub(max_lines);
    trimmed[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn extracts_non_empty_terminal_tail() {
        let screen = "line1\nline2\nerror: borrow failed\n\n   \n";
        assert_eq!(extract_terminal_tail(screen, 2), "line2\nerror: borrow failed");
        assert_eq!(extract_terminal_tail("   \n\n", 10), "");
    }
}
