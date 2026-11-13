//! 主窗口：左侧主机列表，右侧标签页和终端。

use gpui::{
    Context, Entity, Focusable, MouseButton, MouseDownEvent, Window, div, prelude::*, px, rgb,
};
use nexsh_core::{Host, HostStore, ssh_config};

use crate::terminal_view::TerminalView;
use crate::theme;

pub struct Workspace {
    hosts: Vec<Host>,
    /// 读取主机列表失败时显示在侧栏里。
    load_error: Option<String>,
    tabs: Vec<Entity<TerminalView>>,
    active: usize,
}

impl Workspace {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let mut hosts = Vec::new();
        let mut load_error = None;
        match HostStore::load_default() {
            Ok(store) => hosts.extend(store.hosts().iter().cloned()),
            Err(error) => load_error = Some(error.to_string()),
        }
        // ~/.ssh/config 里的主机排在后面，同名时以 nexsh 自己保存的为准。
        if let Some(path) = ssh_config::default_path() {
            for host in ssh_config::load(&path) {
                if !hosts.iter().any(|existing: &Host| existing.name == host.name) {
                    hosts.push(host);
                }
            }
        }
        Self { hosts, load_error, tabs: Vec::new(), active: 0 }
    }

    fn open_host(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.hosts.get(index).cloned() else { return };
        let view = cx.new(|cx| TerminalView::new(host, cx));
        // 终端标题变化时重绘标签栏。
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        self.tabs.push(view);
        self.active = self.tabs.len() - 1;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active = index;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// 关闭标签页会丢弃终端视图，SSH 会话随之断开。
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.active > index || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.tabs.get(self.active) {
            let handle = view.read(cx).focus_handle(cx);
            window.focus(&handle);
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hosts = self.hosts.iter().enumerate().map(|(index, host)| {
            div()
                .px_3()
                .py_1()
                .flex()
                .flex_col()
                .cursor_pointer()
                .hover(|style| style.bg(rgb(theme::HOVER_BG)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.open_host(index, window, cx)
                    }),
                )
                .child(host.name.clone())
                .child(div().text_xs().text_color(rgb(theme::TEXT_MUTED)).child(host.address()))
        });

        let empty_hint = (self.hosts.is_empty() && self.load_error.is_none()).then(|| {
            div()
                .px_3()
                .py_2()
                .text_color(rgb(theme::TEXT_MUTED))
                .child("还没有主机。用 nexsh-cli host add 添加，或在 ~/.ssh/config 里配置。")
        });
        let error = self.load_error.clone().map(|message| {
            div().px_3().py_2().text_color(rgb(theme::ERROR)).child(message)
        });

        div()
            .w(px(220.))
            .h_full()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .bg(rgb(theme::SIDEBAR_BG))
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(div().px_3().py_2().text_color(rgb(theme::TEXT_MUTED)).child("主机"))
            .children(error)
            .children(empty_hint)
            .child(div().id("host-list").flex_1().overflow_y_scroll().children(hosts))
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.iter().enumerate().map(|(index, view)| {
            let active = index == self.active;
            let title = view.read(cx).tab_title();
            let text_color = if active { theme::TEXT } else { theme::TEXT_MUTED };
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h_full()
                .cursor_pointer()
                .text_color(rgb(text_color))
                .border_r_1()
                .border_color(rgb(theme::BORDER))
                .when(active, |tab| tab.bg(rgb(theme::WINDOW_BG)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.activate_tab(index, window, cx)
                    }),
                )
                .child(title)
                .child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .hover(|style| style.bg(rgb(theme::HOVER_BG)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                // 不让点击继续冒泡到标签本身，否则会去激活一个已关闭的标签。
                                cx.stop_propagation();
                                this.close_tab(index, window, cx);
                            }),
                        )
                        .child("×"),
                )
        });

        div()
            .h(px(34.))
            .w_full()
            .flex()
            .flex_shrink_0()
            .bg(rgb(theme::SIDEBAR_BG))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .children(tabs)
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.tabs.get(self.active) {
            Some(view) => div().flex_1().min_h_0().w_full().child(view.clone()),
            None => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(theme::TEXT_MUTED))
                .child("从左侧选择一台主机开始连接"),
        };

        div()
            .size_full()
            .flex()
            .bg(rgb(theme::WINDOW_BG))
            .text_color(rgb(theme::TEXT))
            .text_size(px(13.))
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.render_tab_bar(cx))
                    .child(content),
            )
    }
}
