//! 单行文本输入框。GPUI 不自带输入控件，这里实现表单和密码框需要的最小功能：
//! 光标移动、删除、粘贴、输入法组合，以及密码遮挡显示。
//!
//! 回车、Tab、Esc 不在这里处理，会冒泡给父元素，由表单决定提交、切换或取消。

use std::ops::Range;

use gpui::{
    App, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, KeyDownEvent, MouseButton, MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine,
    SharedString, TextRun, UTF16Selection, UnderlineStyle, Window, canvas, div, fill, point,
    prelude::*, px, rgb, size,
};

use crate::theme;

/// 密码框里代替每个字符显示的圆点。
const MASK: char = '•';

pub struct TextInput {
    focus_handle: FocusHandle,
    text: String,
    /// 光标位置，是 `text` 里的字节偏移，总是落在字符边界上。
    cursor: usize,
    /// 输入法正在组合的那一段在 `text` 里的字节范围。
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    masked: bool,
    /// 不画边框和底色，由外层容器来画（例如带图标的搜索框）。
    bare: bool,
    /// 内容没通过校验，边框标红。
    invalid: bool,
    /// 内容每改动一次加一。观察者用它区分"文字变了"和"只是重绘"。
    revision: u64,
    /// 最近一次改动是不是粘贴。表单用它判断要不要把粘贴进来的地址拆开。
    pasted: bool,
    /// 上一帧的排版结果，输入法用它来摆放候选框。
    last_line: Option<ShapedLine>,
}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            text: String::new(),
            cursor: 0,
            marked: None,
            placeholder: placeholder.into(),
            masked: false,
            bare: false,
            invalid: false,
            revision: 0,
            pasted: false,
            last_line: None,
        }
    }

    /// 密码模式：内容显示为圆点。
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    /// 不画边框和底色，嵌在别的容器里用。
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    /// 换掉全部内容，光标放到末尾。
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.text != text || self.marked.is_some() {
            self.text = text.to_string();
            self.cursor = self.text.len();
            self.marked = None;
            self.pasted = false;
            self.revision += 1;
            cx.notify();
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn set_invalid(&mut self, invalid: bool, cx: &mut Context<Self>) {
        if self.invalid != invalid {
            self.invalid = invalid;
            cx.notify();
        }
    }

    /// 最近一次改动是否为粘贴；读取后复位。
    pub fn take_pasted(&mut self) -> bool {
        std::mem::take(&mut self.pasted)
    }

    /// 把 `range` 换成 `new_text`，光标放到新文字之后。
    fn splice(&mut self, range: Range<usize>, new_text: &str) {
        self.text.replace_range(range.clone(), new_text);
        self.cursor = range.start + new_text.len();
        self.pasted = false;
        self.revision += 1;
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.text[..offset].char_indices().next_back().map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..].chars().next().map_or(offset, |ch| offset + ch.len_utf8())
    }

    /// 输入法接口用 UTF-16 偏移，内部用 UTF-8 字节偏移。
    fn offset_from_utf16(&self, offset_utf16: usize) -> usize {
        let mut utf16_count = 0;
        for (index, ch) in self.text.char_indices() {
            if utf16_count >= offset_utf16 {
                return index;
            }
            utf16_count += ch.len_utf16();
        }
        self.text.len()
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset].encode_utf16().count()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    /// 实际画出来的文字，以及内部偏移在其中对应的位置。
    fn display_text(&self) -> String {
        if self.masked {
            MASK.to_string().repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    fn display_offset(&self, offset: usize) -> usize {
        if self.masked {
            self.text[..offset].chars().count() * MASK.len_utf8()
        } else {
            offset
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // 组合期间的按键属于输入法。
        if self.marked.is_some() {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let command = modifiers.platform || modifiers.control;

        let handled = match keystroke.key.as_str() {
            "backspace" if self.cursor > 0 => {
                let start = self.previous_boundary(self.cursor);
                self.splice(start..self.cursor, "");
                true
            }
            "delete" if self.cursor < self.text.len() => {
                let end = self.next_boundary(self.cursor);
                self.splice(self.cursor..end, "");
                true
            }
            "backspace" | "delete" => true,
            "left" => {
                self.cursor = self.previous_boundary(self.cursor);
                true
            }
            "right" => {
                self.cursor = self.next_boundary(self.cursor);
                true
            }
            "home" => {
                self.cursor = 0;
                true
            }
            "end" => {
                self.cursor = self.text.len();
                true
            }
            "v" if command => {
                if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    // 单行输入框：粘贴内容里的换行直接去掉。
                    let pasted: String = pasted.chars().filter(|ch| !ch.is_control()).collect();
                    self.splice(self.cursor..self.cursor, &pasted);
                    self.pasted = true;
                }
                true
            }
            "a" if command => {
                self.cursor = self.text.len();
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn on_mouse_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.cursor = self.text.len();
        cx.notify();
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let cursor = self.offset_to_utf16(self.cursor);
        Some(UTF16Selection { range: cursor..cursor, reversed: false })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.splice(range, new_text);
        self.marked = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        let start = range.start;
        self.splice(range, new_text);
        self.marked = (!new_text.is_empty()).then(|| start..start + new_text.len());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_line.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = line.x_for_index(self.display_offset(range.start));
        let end = line.x_for_index(self.display_offset(range.end));
        Some(Bounds::from_corners(
            point(bounds.left() + start, bounds.top()),
            point(bounds.left() + end, bounds.bottom()),
        ))
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

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout_view = cx.entity();
        let paint_view = cx.entity();
        let focus_handle = self.focus_handle.clone();
        let focused = self.focus_handle.is_focused(window);
        let border = if self.invalid {
            theme::ERROR
        } else if focused {
            theme::ACCENT
        } else {
            theme::BORDER
        };

        let frame = div().h(px(32.)).w_full().flex().items_center();
        let frame = if self.bare {
            frame
        } else {
            frame
                .px(px(10.))
                .bg(rgb(theme::INPUT_BG))
                .border_1()
                .border_color(rgb(border))
                .rounded(px(theme::RADIUS_CONTROL))
        };
        frame
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .child(
                canvas(
                    move |bounds, window, cx| layout_input(&layout_view, bounds, window, cx),
                    move |bounds, layout, window, cx| {
                        paint_input(&paint_view, &focus_handle, bounds, layout, window, cx)
                    },
                )
                .size_full(),
            )
    }
}

struct InputLayout {
    line: ShapedLine,
    origin: Point<Pixels>,
    line_height: Pixels,
    cursor: PaintQuad,
}

fn layout_input(
    view: &Entity<TextInput>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> InputLayout {
    let input = view.read(cx);
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let line_height = window.line_height();

    let display = input.display_text();
    let cursor_offset = input.display_offset(input.cursor);
    let (text, color) = if display.is_empty() {
        (input.placeholder.clone(), rgb(theme::TEXT_MUTED).into())
    } else {
        (SharedString::from(display), style.color)
    };

    let plain = TextRun {
        len: text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    // 输入法组合中的那一段加下划线。
    let marked = input
        .marked
        .as_ref()
        .filter(|_| !input.text.is_empty())
        .map(|range| input.display_offset(range.start)..input.display_offset(range.end));
    let runs: Vec<TextRun> = match marked {
        Some(marked) => [
            TextRun { len: marked.start, ..plain.clone() },
            TextRun {
                len: marked.end - marked.start,
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                }),
                ..plain.clone()
            },
            TextRun { len: text.len() - marked.end, ..plain },
        ]
        .into_iter()
        .filter(|run| run.len > 0)
        .collect(),
        None => vec![plain],
    };

    let line = window.text_system().shape_line(text, font_size, &runs, None);
    // 文字在输入框里垂直居中。
    let origin = point(bounds.left(), bounds.top() + (bounds.size.height - line_height) * 0.5);
    let cursor_x = if input.text.is_empty() { px(0.) } else { line.x_for_index(cursor_offset) };
    let cursor = fill(
        Bounds::new(point(origin.x + cursor_x, origin.y), size(px(1.5), line_height)),
        rgb(theme::TEXT),
    );

    InputLayout { line, origin, line_height, cursor }
}

fn paint_input(
    view: &Entity<TextInput>,
    focus_handle: &FocusHandle,
    bounds: Bounds<Pixels>,
    layout: InputLayout,
    window: &mut Window,
    cx: &mut App,
) {
    window.handle_input(focus_handle, ElementInputHandler::new(bounds, view.clone()), cx);
    let _ = layout.line.paint(layout.origin, layout.line_height, window, cx);
    if focus_handle.is_focused(window) {
        window.paint_quad(layout.cursor);
    }
    view.update(cx, |input, _| input.last_line = Some(layout.line));
}
