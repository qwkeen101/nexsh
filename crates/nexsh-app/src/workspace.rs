//! 主窗口：左侧主机列表，右侧标签页和终端。

use std::path::PathBuf;

use gpui::{
    Context, Div, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Window,
    div, prelude::*, px, rgb, rgba,
};
use nexsh_core::{Host, HostStore, ssh_config};

use crate::terminal_view::TerminalView;
use crate::text_input::TextInput;
use crate::theme;

/// "添加主机"对话框。
struct HostForm {
    name: Entity<TextInput>,
    target: Entity<TextInput>,
    identity_file: Entity<TextInput>,
    error: Option<String>,
}

impl HostForm {
    fn inputs(&self) -> [&Entity<TextInput>; 3] {
        [&self.name, &self.target, &self.identity_file]
    }
}

pub struct Workspace {
    focus_handle: FocusHandle,
    /// nexsh 自己保存的主机；读取失败时为 None，此时不允许添加或删除，以免覆盖原文件。
    store: Option<HostStore>,
    /// 来自 ~/.ssh/config 的主机，只读。
    config_hosts: Vec<Host>,
    /// 侧栏显示的列表：前 `saved_count` 项来自 store，其余来自 ~/.ssh/config。
    hosts: Vec<Host>,
    saved_count: usize,
    /// 读写主机列表失败时显示在侧栏里。
    error: Option<String>,
    form: Option<HostForm>,
    tabs: Vec<Entity<TerminalView>>,
    active: usize,
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (store, error) = match HostStore::load_default() {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let config_hosts =
            ssh_config::default_path().map(|path| ssh_config::load(&path)).unwrap_or_default();
        let mut workspace = Self {
            focus_handle: cx.focus_handle(),
            store,
            config_hosts,
            hosts: Vec::new(),
            saved_count: 0,
            error,
            form: None,
            tabs: Vec::new(),
            active: 0,
        };
        workspace.rebuild_hosts();
        workspace
    }

    /// ~/.ssh/config 里的主机排在后面，同名时以 nexsh 自己保存的为准。
    fn rebuild_hosts(&mut self) {
        let mut hosts: Vec<Host> =
            self.store.as_ref().map(|store| store.hosts().to_vec()).unwrap_or_default();
        self.saved_count = hosts.len();
        for host in &self.config_hosts {
            if !hosts.iter().any(|existing| existing.name == host.name) {
                hosts.push(host.clone());
            }
        }
        self.hosts = hosts;
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

    /// 只删除 nexsh 自己保存的主机；已打开的标签页不受影响。
    fn delete_host(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.saved_count {
            return;
        }
        let Some(store) = self.store.as_mut() else { return };
        let name = self.hosts[index].name.clone();
        let result = store.remove(&name).and_then(|_| store.save());
        self.error = result.err().map(|error| error.to_string());
        self.rebuild_hosts();
        cx.notify();
    }

    fn open_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.is_none() {
            return;
        }
        let name = cx.new(|cx| TextInput::new("可留空，默认用地址", cx));
        let target = cx.new(|cx| TextInput::new("user@host 或 user@host:端口", cx));
        let identity_file = cx.new(|cx| TextInput::new("可留空，例如 ~/.ssh/id_ed25519", cx));
        let first = target.read(cx).focus_handle(cx);
        window.focus(&first);
        self.form = Some(HostForm { name, target, identity_file, error: None });
        cx.notify();
    }

    fn close_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn submit_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.form.as_ref() else { return };
        let name = form.name.read(cx).text().trim().to_string();
        let target = form.target.read(cx).text().trim().to_string();
        let identity_file = form.identity_file.read(cx).text().trim().to_string();

        let result = self.add_host(&name, &target, &identity_file);
        match result {
            Ok(()) => {
                self.error = None;
                self.rebuild_hosts();
                self.close_form(window, cx);
            }
            Err(message) => {
                if let Some(form) = self.form.as_mut() {
                    form.error = Some(message);
                }
                cx.notify();
            }
        }
    }

    fn add_host(&mut self, name: &str, target: &str, identity_file: &str) -> Result<(), String> {
        if target.is_empty() {
            return Err("请填写地址".to_string());
        }
        let mut host = Host::parse_target(target)
            .ok_or_else(|| "地址格式不对，应为 user@host 或 user@host:端口".to_string())?;
        if !name.is_empty() {
            host.name = name.to_string();
        }
        if !identity_file.is_empty() {
            host.identity_file = Some(expand_home(identity_file));
        }
        let store = self.store.as_mut().ok_or_else(|| "主机列表不可用".to_string())?;
        let name = host.name.clone();
        store.add(host).map_err(|error| error.to_string())?;
        if let Err(error) = store.save() {
            // 没写进文件就不要留在列表里，否则下次启动会凭空消失。
            let _ = store.remove(&name);
            return Err(error.to_string());
        }
        Ok(())
    }

    /// Tab 在对话框的输入框之间循环，Shift+Tab 反向。
    fn focus_next_field(&self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.form.as_ref() else { return };
        let handles = form.inputs().map(|input| input.read(cx).focus_handle(cx));
        let current = handles.iter().position(|handle| handle.is_focused(window));
        let count = handles.len();
        let next = match (current, backwards) {
            (Some(index), false) => (index + 1) % count,
            (Some(index), true) => (index + count - 1) % count,
            (None, _) => 0,
        };
        window.focus(&handles[next]);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "enter" => self.submit_form(window, cx),
            "escape" => self.close_form(window, cx),
            "tab" => {
                self.focus_next_field(event.keystroke.modifiers.shift, window, cx);
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
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
        match self.tabs.get(self.active) {
            Some(view) => {
                let handle = view.read(cx).focus_handle(cx);
                window.focus(&handle);
            }
            None => window.focus(&self.focus_handle),
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hosts = self.hosts.iter().enumerate().map(|(index, host)| {
            let deletable = index < self.saved_count;
            div()
                .px_3()
                .py_1()
                .flex()
                .items_center()
                .cursor_pointer()
                .hover(|style| style.bg(rgb(theme::HOVER_BG)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.open_host(index, window, cx)
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(host.name.clone())
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme::TEXT_MUTED))
                                .child(host.address()),
                        ),
                )
                .when(deletable, |row| {
                    row.child(
                        div()
                            .px_1()
                            .rounded_sm()
                            .text_color(rgb(theme::TEXT_MUTED))
                            .hover(|style| style.bg(rgb(theme::BORDER)).text_color(rgb(theme::ERROR)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                    // 不让点击冒泡到整行，否则删除的同时会去连接。
                                    cx.stop_propagation();
                                    this.delete_host(index, cx);
                                }),
                            )
                            .child("×"),
                    )
                })
        });

        let empty_hint = (self.hosts.is_empty() && self.error.is_none()).then(|| {
            div()
                .px_3()
                .py_2()
                .text_color(rgb(theme::TEXT_MUTED))
                .child("还没有主机。点上方的「+ 添加」，或在 ~/.ssh/config 里配置。")
        });
        let error = self.error.clone().map(|message| {
            div().px_3().py_2().text_color(rgb(theme::ERROR)).child(message)
        });
        let add_button = self.store.is_some().then(|| {
            div()
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .text_color(rgb(theme::ACCENT))
                .hover(|style| style.bg(rgb(theme::HOVER_BG)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| this.open_form(window, cx)),
                )
                .child("+ 添加")
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
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_color(rgb(theme::TEXT_MUTED)).child("主机"))
                    .children(add_button),
            )
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

    /// 盖住整个窗口的半透明遮罩，中间是对话框。
    fn render_form(&self, form: &HostForm, cx: &mut Context<Self>) -> Div {
        let field = |label: &'static str, input: &Entity<TextInput>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_color(rgb(theme::TEXT_MUTED)).child(label))
                .child(input.clone())
        };
        let button = |label: &'static str, color: u32| {
            div()
                .px_3()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .border_1()
                .border_color(rgb(theme::BORDER))
                .text_color(rgb(color))
                .hover(|style| style.bg(rgb(theme::HOVER_BG)))
                .child(label)
        };

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            // 挡住遮罩下面的主机列表和终端，鼠标点不穿。
            .occlude()
            .child(
                div()
                    .w(px(400.))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(rgb(theme::SIDEBAR_BG))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .rounded_md()
                    .child("添加主机")
                    .child(field("地址", &form.target))
                    .child(field("名称", &form.name))
                    .child(field("私钥文件", &form.identity_file))
                    .children(form.error.clone().map(|message| {
                        div().text_color(rgb(theme::ERROR)).child(message)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .child("Tab 切换，回车保存，Esc 取消。密码在连接时输入。"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(button("取消", theme::TEXT).on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                            this.close_form(window, cx)
                                        }),
                                    ))
                                    .child(button("保存", theme::ACCENT).on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                            this.submit_form(window, cx)
                                        }),
                                    )),
                            ),
                    ),
            )
    }
}

/// 把开头的 `~/` 换成用户主目录。
fn expand_home(path: &str) -> PathBuf {
    let rest = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"));
    match (rest, std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
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
        let form = self.form.as_ref().map(|form| self.render_form(form, cx));

        div()
            .size_full()
            .relative()
            .flex()
            .bg(rgb(theme::WINDOW_BG))
            .text_color(rgb(theme::TEXT))
            .text_size(px(13.))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
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
            .children(form)
    }
}
