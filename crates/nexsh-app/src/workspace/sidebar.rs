//! 侧栏：搜索框、主机树和右键菜单。

use gpui::{Context, Div, MouseButton, MouseDownEvent, Window, div, prelude::*, px, rgb, rgba};
use nexsh_core::tree::{self, Row};

use super::{MenuTarget, Workspace, dot, state_color, state_rank};
use crate::icons::{Icon, icon};
use crate::terminal_view::SessionState;
use crate::theme;

const SIDEBAR_WIDTH: f32 = 248.;
const ROW_HEIGHT: f32 = 30.;
/// 每深一级缩进多少。
const INDENT: f32 = 14.;
const MENU_WIDTH: f32 = 196.;
const MENU_ITEM_HEIGHT: f32 = 28.;

/// 头像的底色，按主机名固定选一种：同一台主机每次都是同一个颜色，扫一眼就能认出来。
const AVATAR_COLORS: [u32; 8] =
    [0x7aa2f7, 0x9ece6a, 0xe0af68, 0xf7768e, 0xbb9af7, 0x7dcfff, 0xff9e64, 0x73daca];

fn avatar(name: &str) -> Div {
    let hash = name.bytes().fold(0usize, |hash, byte| hash.wrapping_mul(31).wrapping_add(byte as usize));
    let color = AVATAR_COLORS[hash % AVATAR_COLORS.len()];
    let letter = name
        .chars()
        .find(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    div()
        .w(px(20.))
        .h(px(20.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded_sm()
        // 同一个颜色的淡底配实色字。
        .bg(rgba((color << 8) | 0x38))
        .text_color(rgb(color))
        .text_size(px(11.))
        .child(letter)
}

fn indent(depth: usize) -> gpui::Pixels {
    px(8. + depth as f32 * INDENT)
}

impl Workspace {
    /// 一台主机开了多个标签页时，显示其中最需要注意的那个状态。
    fn host_state(&self, name: &str, cx: &Context<Self>) -> Option<SessionState> {
        self.tabs
            .iter()
            .map(|tab| tab.read(cx))
            .filter(|view| view.host().name == name)
            .map(|view| view.state())
            .max_by_key(|state| state_rank(*state))
    }

    fn group_row(
        &self,
        path: &str,
        name: &str,
        depth: usize,
        count: usize,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let toggle_path = path.to_string();
        let menu_path = path.to_string();
        let chevron = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
        div()
            .mx_1()
            .h(px(ROW_HEIGHT))
            .pl(indent(depth))
            .pr_2()
            .flex()
            .items_center()
            .gap_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(rgb(theme::TEXT_MUTED))
            .hover(|style| style.bg(rgb(theme::HOVER_BG)).text_color(rgb(theme::TEXT)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                    this.toggle_group(&toggle_path, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.open_menu(MenuTarget::Group(menu_path.clone()), event.position, cx)
                }),
            )
            .child(icon(chevron, 12., theme::TEXT_MUTED))
            .child(icon(Icon::Folder, 14., theme::TEXT_MUTED))
            .child(div().ml_1().flex_1().min_w_0().truncate().child(name.to_string()))
            .child(div().text_xs().child(count.to_string()))
    }

    fn host_row(&self, index: usize, depth: usize, highlighted: bool, cx: &mut Context<Self>) -> Div {
        let host = &self.hosts[index];
        let state = self.host_state(&host.name, cx);
        let address = host.address();
        let target = host.clone();
        div()
            .group("host-row")
            .mx_1()
            .h(px(ROW_HEIGHT))
            // 主机比同级的分组多缩进一个箭头的宽度，头像和分组的文件夹图标对齐。
            .pl(indent(depth) + px(16.))
            .pr_1()
            .flex()
            .items_center()
            .gap_2()
            .rounded_sm()
            .cursor_pointer()
            .when(highlighted, |row| row.bg(rgb(theme::HOVER_BG)))
            .hover(|style| style.bg(rgb(theme::HOVER_BG)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.open_host(target.clone(), window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.open_menu(MenuTarget::Host(index), event.position, cx)
                }),
            )
            .child(avatar(&host.name))
            .child(div().flex_shrink_0().max_w(px(132.)).truncate().child(host.name.clone()))
            // 名称就是地址时不重复显示。
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_MUTED))
                    .when(address != host.name, |element| element.child(address)),
            )
            .children(state.map(|state| dot(Some(state_color(state)))))
            // "更多"按钮只在鼠标移到这一行时出现，打开的菜单和右键是同一个。
            .child(
                div()
                    .p_0p5()
                    .rounded_sm()
                    .opacity(0.)
                    .group_hover("host-row", |style| style.opacity(1.))
                    .hover(|style| style.bg(rgb(theme::BORDER)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            // 不让点击冒泡到整行，否则会同时去连接。
                            cx.stop_propagation();
                            this.open_menu(MenuTarget::Host(index), event.position, cx);
                        }),
                    )
                    .child(icon(Icon::More, 14., theme::TEXT_MUTED)),
            )
    }

    pub(super) fn render_sidebar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let query = self.query(cx);
        let searching = !query.is_empty();
        let rows = tree::rows(&self.hosts, &query, &self.collapsed);
        let visible_count = rows.iter().filter(|row| matches!(row, Row::Host { .. })).count();
        let search_focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let selected = self.selected.min(visible_count.saturating_sub(1));
        let active_host = self.tabs.get(self.active).map(|tab| tab.read(cx).host().name.clone());

        let mut elements: Vec<Div> = Vec::new();
        if let Some(target) = self.quick_target(&query) {
            let label = format!("连接 {}", target.address());
            elements.push(
                div()
                    .mx_1()
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(theme::ACCENT))
                    .hover(|style| style.bg(rgb(theme::HOVER_BG)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.search.update(cx, |input, cx| input.clear(cx));
                            this.open_host(target.clone(), window, cx);
                        }),
                    )
                    .child(icon(Icon::Terminal, 14., theme::ACCENT))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    // 有匹配的主机时回车打开的是选中的那台，这一行要用鼠标点。
                    .when(visible_count == 0, |row| {
                        row.child(div().text_xs().text_color(rgb(theme::TEXT_MUTED)).child("回车"))
                    }),
            );
        }

        let mut position = 0;
        for row in &rows {
            match row {
                Row::Group { path, name, depth, count, collapsed } => {
                    elements.push(self.group_row(path, name, *depth, *count, *collapsed, cx));
                }
                Row::Host { index, depth } => {
                    let host = &self.hosts[*index];
                    // 键盘在搜索框里时高亮上下键选中的那台，否则高亮当前标签页的主机。
                    let highlighted = if search_focused {
                        position == selected
                    } else {
                        active_host.as_deref() == Some(host.name.as_str())
                    };
                    elements.push(self.host_row(*index, *depth, highlighted, cx));
                    position += 1;
                }
            }
        }

        let hint = if !elements.is_empty() || self.error.is_some() {
            None
        } else if searching {
            Some("没有匹配的主机")
        } else {
            Some("还没有主机。点右上角的加号添加，或在 ~/.ssh/config 里配置。")
        };
        let hint = hint
            .map(|text| div().px_3().py_2().text_color(rgb(theme::TEXT_MUTED)).child(text));
        let error = self.error.clone().map(|message| {
            div().px_3().py_2().text_color(rgb(theme::ERROR)).child(message)
        });
        let add_button = self.store.is_some().then(|| {
            div()
                .p_1()
                .rounded_sm()
                .cursor_pointer()
                .hover(|style| style.bg(rgb(theme::HOVER_BG)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.open_form(None, None, window, cx)
                    }),
                )
                .child(icon(Icon::Plus, 15., theme::TEXT))
        });
        let search_border = if search_focused { theme::ACCENT } else { theme::BORDER };
        let summary = if searching {
            format!("匹配 {visible_count} 台，共 {} 台", self.hosts.len())
        } else {
            format!("{} 台主机", self.hosts.len())
        };

        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .bg(rgb(theme::SIDEBAR_BG))
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .h(px(34.))
                    .pl_3()
                    .pr_2()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .child("主机")
                    .children(add_button),
            )
            .child(
                div()
                    .mx_2()
                    .mb_2()
                    .px_2()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .bg(rgb(theme::INPUT_BG))
                    .border_1()
                    .border_color(rgb(search_border))
                    .child(icon(Icon::Search, 14., theme::TEXT_MUTED))
                    .child(div().flex_1().min_w_0().child(self.search.clone())),
            )
            .children(error)
            .children(hint)
            .child(div().id("host-list").flex_1().overflow_y_scroll().children(elements))
            .child(
                div()
                    .h(px(24.))
                    .px_3()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_MUTED))
                    .border_t_1()
                    .border_color(rgb(theme::BORDER))
                    .child(summary),
            )
    }

    /// 右键菜单。盖住整个窗口的透明层负责在点到别处时关掉它。
    pub(super) fn render_menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let menu = self.menu.as_ref()?;
        let item = |kind: Icon, label: &str, danger: bool| {
            let color = if danger { theme::ERROR } else { theme::TEXT };
            div()
                .mx_1()
                .h(px(MENU_ITEM_HEIGHT))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded_sm()
                .cursor_pointer()
                .text_color(rgb(color))
                .hover(|style| style.bg(rgb(theme::HOVER_BG)))
                .child(icon(kind, 14., if danger { theme::ERROR } else { theme::TEXT_MUTED }))
                .child(label.to_string())
        };

        let mut items: Vec<Div> = Vec::new();
        match &menu.target {
            MenuTarget::Host(index) => {
                let index = *index;
                let host = self.hosts.get(index)?.clone();
                items.push(item(Icon::Terminal, "连接", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.close_menu(cx);
                        this.open_host(host.clone(), window, cx);
                    }),
                ));
                items.push(item(Icon::Copy, "复制地址", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.copy_address(index, cx);
                        this.close_menu(cx);
                    }),
                ));
                // ~/.ssh/config 里的主机是只读的。
                if index < self.saved_count {
                    items.push(item(Icon::Pencil, "编辑", false).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.close_menu(cx);
                            if let Some(host) = this.hosts.get(index).cloned() {
                                this.open_form(Some(host), None, window, cx);
                            }
                        }),
                    ));
                    // 删除不可撤销，要点两次：第一次只是把这一项变成确认。
                    let label = if menu.confirming_delete { "再点一次确认删除" } else { "删除" };
                    items.push(item(Icon::Trash, label, true).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            let confirmed =
                                this.menu.as_ref().is_some_and(|menu| menu.confirming_delete);
                            if confirmed {
                                this.close_menu(cx);
                                this.delete_host(index, cx);
                            } else if let Some(menu) = this.menu.as_mut() {
                                menu.confirming_delete = true;
                                cx.notify();
                            }
                        }),
                    ));
                }
            }
            MenuTarget::Group(path) => {
                if self.store.is_some() {
                    let group = path.clone();
                    items.push(item(Icon::Plus, "在此分组新建主机", false).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.close_menu(cx);
                            this.open_form(None, Some(group.clone()), window, cx);
                        }),
                    ));
                }
                let collapsed = self.collapsed.contains(path);
                let (kind, label) = if collapsed {
                    (Icon::ChevronDown, "展开")
                } else {
                    (Icon::ChevronRight, "折叠")
                };
                let group = path.clone();
                items.push(item(kind, label, false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.close_menu(cx);
                        this.toggle_group(&group, cx);
                    }),
                ));
            }
        }

        // 菜单整个留在窗口里：靠近右边或下边时往回挪。
        let viewport = window.viewport_size();
        let height = items.len() as f32 * MENU_ITEM_HEIGHT + 10.;
        let left = f32::from(menu.position.x).min(f32::from(viewport.width) - MENU_WIDTH - 4.).max(4.);
        let top = f32::from(menu.position.y).min(f32::from(viewport.height) - height - 4.).max(4.);

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| this.close_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| this.close_menu(cx)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(MENU_WIDTH))
                        .py_1()
                        .flex()
                        .flex_col()
                        .bg(rgb(theme::SIDEBAR_BG))
                        .border_1()
                        .border_color(rgb(theme::BORDER))
                        .rounded_md()
                        .children(items),
                ),
        )
    }
}
