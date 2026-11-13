//! 一个终端标签页：持有 SSH 会话和终端状态，负责绘制与输入。

use std::ops::Range;

use gpui::{
    App, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, PaintQuad, Pixels, Point,
    ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle, Task, TextRun,
    UTF16Selection, UnderlineStyle, Window, canvas, div, fill, font, point, prelude::*, px, rgb,
    size,
};
use nexsh_core::{
    ConnectOptions, Host, LatencyMeter, SessionCommand, SessionEvent, SessionHandle, SshError,
    spawn_session,
};
use nexsh_term::{KeyMods, Rgb, TermEvent, Terminal, bracketed_paste, encode_key};
use tokio::sync::mpsc::UnboundedSender;

use crate::text_input::TextInput;
use crate::theme;

const INITIAL_COLS: usize = 80;
const INITIAL_ROWS: usize = 24;
/// 一次界面更新最多合并多少个会话事件。输出很快时，多个数据包只触发一次重绘。
const MAX_EVENT_BATCH: usize = 512;

enum Status {
    Connecting,
    Connected,
    Exited(Option<u32>),
    Failed(String),
}

/// 会话所处的阶段，供标签栏、侧栏和状态栏显示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Connecting,
    /// 等用户输入密码或口令。
    NeedsInput,
    Connected,
    Ended,
    Failed,
}

/// 连接需要用户补充的凭据。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Secret {
    Password,
    Passphrase,
}

struct Prompt {
    secret: Secret,
    message: String,
}

pub struct TerminalView {
    focus_handle: FocusHandle,
    host: Host,
    /// 连接参数。用户输入的密码和口令只保存在这里（内存中），用于重连。
    options: ConnectOptions,
    /// 正在向用户要密码或口令。
    prompt: Option<Prompt>,
    prompt_input: Entity<TextInput>,
    terminal: Terminal,
    commands: UnboundedSender<SessionCommand>,
    status: Status,
    /// 远端程序设置的窗口标题。
    title: Option<String>,
    /// 首次连接时记住主机密钥的提示。
    notice: Option<String>,
    latency: LatencyMeter,
    /// 输入法正在组合、尚未提交的文字。
    composing: Option<String>,
    /// 光标所在单元格的位置，用来摆放输入法候选框。
    cursor_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    /// 滚轮滚动不足一行的余量。
    scroll_remainder: f32,
    _reader: Task<()>,
}

impl TerminalView {
    pub fn new(host: Host, cx: &mut Context<Self>) -> Self {
        let options = ConnectOptions::new(host.clone(), INITIAL_COLS as u32, INITIAL_ROWS as u32);
        let (commands, reader) = Self::start_session(options.clone(), cx);
        let prompt_input = cx.new(|cx| TextInput::new("", cx).masked());

        Self {
            focus_handle: cx.focus_handle(),
            host,
            options,
            prompt: None,
            prompt_input,
            terminal: Terminal::new(INITIAL_COLS, INITIAL_ROWS),
            commands,
            status: Status::Connecting,
            title: None,
            notice: None,
            latency: LatencyMeter::default(),
            composing: None,
            cursor_bounds: None,
            line_height: px(theme::FONT_SIZE * theme::LINE_HEIGHT),
            scroll_remainder: 0.0,
            _reader: reader,
        }
    }

    /// 启动会话，并把它的事件成批送回界面。
    fn start_session(
        options: ConnectOptions,
        cx: &mut Context<Self>,
    ) -> (UnboundedSender<SessionCommand>, Task<()>) {
        let SessionHandle { commands, mut events } = spawn_session(options);
        let reader = cx.spawn(async move |this, cx| {
            while let Some(first) = events.recv().await {
                let mut batch = vec![first];
                while batch.len() < MAX_EVENT_BATCH
                    && let Ok(event) = events.try_recv()
                {
                    batch.push(event);
                }
                let alive = this
                    .update(cx, |view, cx| {
                        view.apply_events(batch);
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        (commands, reader)
    }

    /// 用当前的连接参数重新连接。旧会话的命令通道随之关闭，会话自行结束。
    fn reconnect(&mut self, cx: &mut Context<Self>) {
        let (cols, rows) = self.terminal.size();
        self.options.cols = cols as u32;
        self.options.rows = rows as u32;
        self.terminal = Terminal::new(cols, rows);
        self.status = Status::Connecting;
        self.title = None;
        self.notice = None;
        self.latency = LatencyMeter::default();
        self.prompt = None;
        self.composing = None;
        let (commands, reader) = Self::start_session(self.options.clone(), cx);
        self.commands = commands;
        self._reader = reader;
        cx.notify();
    }

    /// 认证失败时，判断是否该向用户要密码或口令。
    fn prompt_for(&self, error: &SshError) -> Option<Prompt> {
        match error {
            SshError::PassphraseRequired(path) => {
                let retry = if self.options.key_passphrase.is_some() { "口令不正确。" } else { "" };
                Some(Prompt {
                    secret: Secret::Passphrase,
                    message: format!(
                        "{retry}输入私钥 {} 的口令（留空回车则跳过此私钥）",
                        path.display()
                    ),
                })
            }
            SshError::PasswordRequired => Some(Prompt {
                secret: Secret::Password,
                message: format!("输入 {} 的密码", self.host.address()),
            }),
            // 试过密码但被拒绝：可能是刚输错了，也可能是钥匙串里保存的密码已经失效。
            SshError::AuthFailed { tried } if tried.iter().any(|method| method == "密码") => {
                Some(Prompt {
                    secret: Secret::Password,
                    message: format!("密码不正确，重新输入 {} 的密码", self.host.address()),
                })
            }
            _ => None,
        }
    }

    fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else { return };
        let secret = self.prompt_input.read(cx).text().to_string();
        self.prompt_input.update(cx, |input, cx| input.clear(cx));
        match prompt.secret {
            Secret::Password => self.options.password = Some(secret),
            Secret::Passphrase => self.options.key_passphrase = Some(secret),
        }
        window.focus(&self.focus_handle);
        self.reconnect(cx);
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn state(&self) -> SessionState {
        match self.status {
            _ if self.prompt.is_some() => SessionState::NeedsInput,
            Status::Connecting => SessionState::Connecting,
            Status::Connected => SessionState::Connected,
            Status::Exited(_) => SessionState::Ended,
            Status::Failed(_) => SessionState::Failed,
        }
    }

    /// 终端当前的列数和行数。
    pub fn grid_size(&self) -> (usize, usize) {
        self.terminal.size()
    }

    pub fn latency(&self) -> &LatencyMeter {
        &self.latency
    }

    pub fn tab_title(&self) -> String {
        match &self.title {
            Some(title) if !title.is_empty() => title.clone(),
            _ => self.host.name.clone(),
        }
    }

    fn apply_events(&mut self, events: Vec<SessionEvent>) {
        for event in events {
            match event {
                SessionEvent::Output(bytes) => self.terminal.feed(&bytes),
                SessionEvent::HostKeyLearned(fingerprint) => {
                    self.notice = Some(format!("首次连接，已记住主机密钥 {fingerprint}"));
                }
                SessionEvent::Connected => {
                    self.status = Status::Connected;
                    // 连接期间窗口大小可能已经变过，补发一次当前尺寸。
                    let (cols, rows) = self.terminal.size();
                    self.send(SessionCommand::Resize { cols: cols as u32, rows: rows as u32 });
                }
                SessionEvent::Latency(sample) => self.latency.record(sample),
                SessionEvent::Exited(code) => self.status = Status::Exited(code),
                SessionEvent::Failed(error) => {
                    self.prompt = self.prompt_for(&error);
                    self.status = Status::Failed(error.to_string());
                }
                SessionEvent::Closed => {
                    if matches!(self.status, Status::Connecting | Status::Connected) {
                        self.status = Status::Exited(None);
                    }
                }
            }
        }
        for event in self.terminal.take_events() {
            match event {
                TermEvent::PtyWrite(bytes) => self.send(SessionCommand::Input(bytes)),
                TermEvent::Title(title) => self.title = Some(title),
                TermEvent::ResetTitle => self.title = None,
                TermEvent::Bell => {}
            }
        }
    }

    fn send(&self, command: SessionCommand) {
        // 会话已结束时发送会失败，忽略即可。
        let _ = self.commands.send(command);
    }

    fn send_input(&mut self, bytes: Vec<u8>) {
        self.terminal.scroll_to_bottom();
        self.send(SessionCommand::Input(bytes));
    }

    fn set_grid(&mut self, cols: usize, rows: usize) {
        if self.terminal.resize(cols, rows) {
            let (cols, rows) = self.terminal.size();
            self.send(SessionCommand::Resize { cols: cols as u32, rows: rows as u32 });
        }
    }

    /// 终端上方的状态条：连接中、出错、已断开或首次连接提示。
    fn banner(&self) -> Option<(String, u32)> {
        match &self.status {
            Status::Connecting => {
                Some((format!("正在连接 {} ...", self.host.address()), theme::TEXT_MUTED))
            }
            Status::Failed(message) => {
                Some((format!("{message}　按回车重新连接"), theme::ERROR))
            }
            Status::Exited(Some(code)) => Some((
                format!("会话已结束（退出码 {code}）　按回车重新连接"),
                theme::TEXT_MUTED,
            )),
            Status::Exited(None) => {
                Some(("连接已断开　按回车重新连接".to_string(), theme::TEXT_MUTED))
            }
            Status::Connected => self.notice.clone().map(|notice| (notice, theme::NOTICE)),
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // 输入法组合期间，回车、退格等按键属于输入法。
        if self.composing.is_some() {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;

        // 询问密码时，按键归输入框；这里只负责回车提交，其余一律不发往远端。
        if self.prompt.is_some() {
            if keystroke.key == "enter" {
                self.submit_prompt(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        // 会话已经结束：回车重新连接，其他按键没有去处。
        if matches!(self.status, Status::Exited(_) | Status::Failed(_)) {
            if keystroke.key == "enter" {
                self.reconnect(cx);
                cx.stop_propagation();
            }
            return;
        }

        // 粘贴：macOS 用 Cmd+V，其他平台用 Ctrl+Shift+V。
        let is_paste = keystroke.key == "v"
            && ((modifiers.platform && !modifiers.control)
                || (modifiers.control && modifiers.shift && !modifiers.platform));
        if is_paste {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let bytes = bracketed_paste(&text, self.terminal.bracketed_paste());
                self.send_input(bytes);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        // 其余带系统键（Cmd/Win）的组合留给系统和应用快捷键。
        if modifiers.platform {
            return;
        }

        let mods = KeyMods { ctrl: modifiers.control, alt: modifiers.alt, shift: modifiers.shift };
        if let Some(bytes) = encode_key(&keystroke.key, mods, self.terminal.app_cursor()) {
            self.send_input(bytes);
            cx.stop_propagation();
            cx.notify();
        }
        // 返回 None 的是普通字符，交给下面的文本输入处理（含输入法）。
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line_height = f32::from(self.line_height);
        let delta = f32::from(event.delta.pixel_delta(self.line_height).y);
        self.scroll_remainder += delta;
        let lines = (self.scroll_remainder / line_height).trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32 * line_height;

        if self.terminal.alt_screen() {
            // 全屏程序（vim、less）没有回滚历史，把滚轮转成方向键。
            let key = if lines > 0 { "up" } else { "down" };
            if let Some(bytes) = encode_key(key, KeyMods::default(), self.terminal.app_cursor()) {
                for _ in 0..lines.unsigned_abs() {
                    self.send(SessionCommand::Input(bytes.clone()));
                }
            }
        } else {
            self.terminal.scroll(lines);
        }
        cx.notify();
    }

    fn on_mouse_down(&mut self, _: &MouseDownEvent, window: &mut Window, _cx: &mut Context<Self>) {
        // 询问密码时焦点留在输入框里。
        if self.prompt.is_none() {
            window.focus(&self.focus_handle);
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// 文本输入（含输入法）。终端没有可编辑的文档，所以"文档"永远是空的：
/// 提交的文字直接发给远端，组合中的文字暂存在 `composing` 里画在光标处。
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.composing.as_ref().map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.composing = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composing = None;
        if !text.is_empty() {
            self.send_input(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composing = (!new_text.is_empty()).then(|| new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout_view = cx.entity();
        let paint_view = cx.entity();
        let focus_handle = self.focus_handle.clone();
        let background = theme::color(self.terminal.frame_background());

        // 需要输入密码时，把这个标签页的焦点交给输入框，用户可以直接打字。
        if self.prompt.is_some() && self.focus_handle.is_focused(window) {
            let input_focus = self.prompt_input.read(cx).focus_handle(cx);
            window.focus(&input_focus);
        }
        let prompt = self.prompt.as_ref().map(|prompt| {
            div()
                .px_2()
                .py_2()
                .flex()
                .flex_col()
                .gap_1()
                .flex_shrink_0()
                .bg(rgb(theme::SIDEBAR_BG))
                .text_color(rgb(theme::NOTICE))
                .child(prompt.message.clone())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(320.)).child(self.prompt_input.clone()))
                        .child(
                            div().text_color(rgb(theme::TEXT_MUTED)).child("回车确认"),
                        ),
                )
        });
        let banner = if prompt.is_some() { None } else { self.banner() };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(background)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .children(prompt)
            .when_some(banner, |element, (text, color)| {
                element.child(
                    div()
                        .px_2()
                        .py_1()
                        .flex_shrink_0()
                        .bg(rgb(theme::SIDEBAR_BG))
                        .text_color(rgb(color))
                        .child(text),
                )
            })
            .child(
                canvas(
                    move |bounds, window, cx| layout_terminal(&layout_view, bounds, window, cx),
                    move |bounds, layout, window, cx| {
                        paint_terminal(&paint_view, &focus_handle, bounds, layout, window, cx)
                    },
                )
                .flex_1()
                .w_full(),
            )
    }
}

/// 一帧里要画的全部内容，在布局阶段算好。
struct TerminalLayout {
    backgrounds: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    line_height: Pixels,
}

/// 同一行里样式相同、列连续的一段文字，整体排版一次。
struct TextSpan {
    row: usize,
    col: usize,
    /// 已占用的列数（宽字符占两列）。
    width: usize,
    text: String,
    fg: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
    strikeout: bool,
}

struct BackgroundSpan {
    row: usize,
    col: usize,
    width: usize,
    color: Rgb,
}

fn layout_terminal(
    view: &Entity<TerminalView>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> TerminalLayout {
    let font_size = px(theme::FONT_SIZE);
    let base_font = font(theme::FONT_FAMILY);
    let text_system = window.text_system().clone();
    let font_id = text_system.resolve_font(&base_font);
    let cell_width = text_system
        .advance(font_id, font_size, 'm')
        .map(|advance| advance.width)
        .unwrap_or(font_size * 0.6);
    let line_height = (font_size * theme::LINE_HEIGHT).round();

    let cols = (f32::from(bounds.size.width) / f32::from(cell_width)).floor() as usize;
    let rows = (f32::from(bounds.size.height) / f32::from(line_height)).floor() as usize;

    let focused = view.read(cx).focus_handle.is_focused(window);
    let (frame, composing) = view.update(cx, |view, _| {
        view.set_grid(cols, rows);
        view.line_height = line_height;
        (view.terminal.frame(), view.composing.clone())
    });

    let cell_origin = |row: usize, col: usize| {
        point(
            bounds.origin.x + cell_width * col as f32,
            bounds.origin.y + line_height * row as f32,
        )
    };
    let cell_bounds = |row: usize, col: usize, width: usize| {
        Bounds::new(cell_origin(row, col), size(cell_width * width as f32, line_height))
    };

    // 背景：合并同一行里相邻的同色单元格，减少绘制的矩形数量。
    let mut background_spans: Vec<BackgroundSpan> = Vec::new();
    // 文字：ASCII 字符按样式合并成一段；其他字符（中文、符号、表情）可能来自
    // 回退字体，宽度不等于等宽字体的单元格，所以逐个按列定位。
    let mut text_spans: Vec<TextSpan> = Vec::new();

    for cell in &frame.cells {
        let width = if cell.wide { 2 } else { 1 };

        if cell.bg != frame.background {
            match background_spans.last_mut() {
                Some(span)
                    if span.row == cell.row
                        && span.color == cell.bg
                        && span.col + span.width == cell.col =>
                {
                    span.width += width;
                }
                _ => background_spans.push(BackgroundSpan {
                    row: cell.row,
                    col: cell.col,
                    width,
                    color: cell.bg,
                }),
            }
        }

        if cell.ch == ' ' || cell.ch == '\0' {
            continue;
        }
        let extendable = cell.ch.is_ascii()
            && text_spans.last().is_some_and(|span| {
                span.row == cell.row
                    && span.text.is_ascii()
                    && span.col + span.width <= cell.col
                    && span.fg == cell.fg
                    && span.bold == cell.bold
                    && span.italic == cell.italic
                    && span.underline == cell.underline
                    && span.strikeout == cell.strikeout
            });
        if extendable && let Some(span) = text_spans.last_mut() {
            // 中间跳过的是空白单元格，用空格补齐以保持列对齐。
            let gap = cell.col - (span.col + span.width);
            span.text.extend(std::iter::repeat_n(' ', gap));
            span.text.push(cell.ch);
            span.width += gap + 1;
        } else {
            text_spans.push(TextSpan {
                row: cell.row,
                col: cell.col,
                width,
                text: cell.ch.to_string(),
                fg: cell.fg,
                bold: cell.bold,
                italic: cell.italic,
                underline: cell.underline,
                strikeout: cell.strikeout,
            });
        }
    }

    let mut backgrounds: Vec<PaintQuad> = background_spans
        .iter()
        .map(|span| fill(cell_bounds(span.row, span.col, span.width), theme::color(span.color)))
        .collect();

    let shape = |window: &mut Window, text: String, color: Hsla, span: Option<&TextSpan>| {
        let mut run_font = base_font.clone();
        if span.is_some_and(|span| span.bold) {
            run_font = run_font.bold();
        }
        if span.is_some_and(|span| span.italic) {
            run_font = run_font.italic();
        }
        // 输入法组合中的文字（span 为 None）总是带下划线。
        let underline = span.is_none_or(|span| span.underline);
        let strikeout = span.is_some_and(|span| span.strikeout);
        let run = TextRun {
            len: text.len(),
            font: run_font,
            color,
            background_color: None,
            underline: underline.then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: strikeout.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(color),
            }),
        };
        window.text_system().shape_line(SharedString::from(text), font_size, &[run], None)
    };

    let mut lines = Vec::with_capacity(text_spans.len() + 1);
    for span in &text_spans {
        let line = shape(window, span.text.clone(), theme::color(span.fg), Some(span));
        lines.push((cell_origin(span.row, span.col), line));
    }

    let cursor_bounds = frame.cursor.map(|cursor| cell_bounds(cursor.row, cursor.col, 1));
    let cursor = cursor_bounds.map(|bounds| {
        let mut color = theme::color(frame.foreground);
        // 失去焦点时光标变淡。
        color.a = if focused { 0.7 } else { 0.3 };
        fill(bounds, color)
    });

    // 输入法组合中的文字画在光标位置，盖住下面的内容。
    if let (Some(text), Some(cursor_bounds)) = (composing, cursor_bounds) {
        let columns: usize = text.chars().map(|ch| if ch.is_ascii() { 1 } else { 2 }).sum();
        backgrounds.push(fill(
            Bounds::new(cursor_bounds.origin, size(cell_width * columns as f32, line_height)),
            theme::color(frame.background),
        ));
        let line = shape(window, text, theme::color(frame.foreground), None);
        lines.push((cursor_bounds.origin, line));
    }

    view.update(cx, |view, _| view.cursor_bounds = cursor_bounds);

    TerminalLayout { backgrounds, cursor, lines, line_height }
}

fn paint_terminal(
    view: &Entity<TerminalView>,
    focus_handle: &FocusHandle,
    bounds: Bounds<Pixels>,
    layout: TerminalLayout,
    window: &mut Window,
    cx: &mut App,
) {
    // 注册文本输入处理器：只有获得焦点时才会真正接收输入。
    window.handle_input(focus_handle, ElementInputHandler::new(bounds, view.clone()), cx);

    for quad in layout.backgrounds {
        window.paint_quad(quad);
    }
    if let Some(cursor) = layout.cursor {
        window.paint_quad(cursor);
    }
    for (origin, line) in &layout.lines {
        // 个别字形画不出来时跳过，不影响其余内容。
        let _ = line.paint(*origin, layout.line_height, window, cx);
    }
}
