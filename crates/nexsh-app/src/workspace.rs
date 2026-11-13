//! 主窗口：左侧主机列表，右侧标签页和终端。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui::{
    Context, Div, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Window,
    div, prelude::*, px, rgb, rgba,
};
use nexsh_core::{Host, HostStore, LatencyLevel, search, ssh_config};

use crate::terminal_view::{SessionState, TerminalView};
use crate::text_input::TextInput;
use crate::theme;

/// "添加主机"对话框。
struct HostForm {
    name: Entity<TextInput>,
    target: Entity<TextInput>,
    group: Entity<TextInput>,
    identity_file: Entity<TextInput>,
    error: Option<String>,
}

/// 侧栏里的一个分组，`hosts` 是 `Workspace::hosts` 的下标。
struct Section {
    title: String,
    hosts: Vec<usize>,
}

/// 没有填分组的已保存主机归在这里。
const UNGROUPED: &str = "未分组";

impl HostForm {
    /// 按对话框里从上到下的顺序，Tab 依此切换。
    fn inputs(&self) -> [&Entity<TextInput>; 4] {
        [&self.target, &self.name, &self.group, &self.identity_file]
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
    /// 侧栏顶部的搜索框，也用来输入地址直接连接。
    search: Entity<TextInput>,
    /// 被折叠的分组（按标题）。搜索时不生效：有匹配的分组总是展开。
    collapsed: HashSet<String>,
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
        let search = cx.new(|cx| TextInput::new("搜索，或输入 user@host 连接", cx));
        // 搜索框内容变化时重新筛选列表。
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let mut workspace = Self {
            focus_handle: cx.focus_handle(),
            search,
            collapsed: HashSet::new(),
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

    /// 当前查询下要显示的分组，保持主机原有的先后顺序。
    fn sections(&self, query: &str) -> Vec<Section> {
        let mut sections: Vec<Section> = Vec::new();
        for (index, host) in self.hosts.iter().enumerate() {
            if !search::matches(host, query) {
                continue;
            }
            let title = host.group.as_deref().unwrap_or(UNGROUPED);
            match sections.iter_mut().find(|section| section.title == title) {
                Some(section) => section.hosts.push(index),
                None => sections.push(Section { title: title.to_string(), hosts: vec![index] }),
            }
        }
        sections
    }

    /// 查询能解析成一个地址、且列表里没有这个地址时，可以直接连接它。
    fn quick_target(&self, query: &str) -> Option<Host> {
        let target = search::quick_target(query)?;
        let address = target.address();
        (!self.hosts.iter().any(|host| host.address() == address)).then_some(target)
    }

    /// 在搜索框里按回车：优先打开第一台匹配的主机，没有匹配就把输入当作地址连接。
    fn submit_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search.read(cx).text().trim().to_string();
        if query.is_empty() {
            return;
        }
        let first_match =
            self.sections(&query).first().and_then(|section| section.hosts.first().copied());
        let host = match first_match {
            Some(index) => self.hosts.get(index).cloned(),
            None => self.quick_target(&query),
        };
        if let Some(host) = host {
            self.search.update(cx, |input, cx| input.clear(cx));
            self.open_host(host, window, cx);
        }
    }

    fn toggle_section(&mut self, title: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(title) {
            self.collapsed.insert(title.to_string());
        }
        cx.notify();
    }

    fn open_host(&mut self, host: Host, window: &mut Window, cx: &mut Context<Self>) {
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
        let group = cx.new(|cx| TextInput::new("可留空，例如 生产环境", cx));
        let identity_file = cx.new(|cx| TextInput::new("可留空，例如 ~/.ssh/id_ed25519", cx));
        let first = target.read(cx).focus_handle(cx);
        window.focus(&first);
        self.form = Some(HostForm { name, target, group, identity_file, error: None });
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
        let group = form.group.read(cx).text().trim().to_string();
        let identity_file = form.identity_file.read(cx).text().trim().to_string();

        let result = self.add_host(&name, &target, &group, &identity_file);
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

    fn add_host(
        &mut self,
        name: &str,
        target: &str,
        group: &str,
        identity_file: &str,
    ) -> Result<(), String> {
        if target.is_empty() {
            return Err("请填写地址".to_string());
        }
        let mut host = Host::parse_target(target)
            .ok_or_else(|| "地址格式不对，应为 user@host 或 user@host:端口".to_string())?;
        if !name.is_empty() {
            host.name = name.to_string();
        }
        if !group.is_empty() {
            host.group = Some(group.to_string());
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
            // 没有对话框时，这里只处理搜索框里的回车和 Esc。
            if !self.search.read(cx).focus_handle(cx).is_focused(window) {
                return;
            }
            match event.keystroke.key.as_str() {
                "enter" => self.submit_search(window, cx),
                "escape" => {
                    self.search.update(cx, |input, cx| input.clear(cx));
                    self.focus_active(window, cx);
                }
                _ => return,
            }
            cx.stop_propagation();
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
        let query = self.search.read(cx).text().trim().to_string();
        let searching = !query.is_empty();
        let sections = self.sections(&query);
        let quick_target = self.quick_target(&query);

        // 每台主机最"值得注意"的会话状态，以及当前标签页对应的主机。
        let mut states: HashMap<&str, SessionState> = HashMap::new();
        for tab in &self.tabs {
            let view = tab.read(cx);
            let state = view.state();
            states
                .entry(view.host().name.as_str())
                .and_modify(|known| {
                    if state_rank(state) > state_rank(*known) {
                        *known = state;
                    }
                })
                .or_insert(state);
        }
        let active_host = self.tabs.get(self.active).map(|tab| tab.read(cx).host().name.as_str());

        let mut rows = Vec::new();
        if let Some(target) = quick_target {
            let label = format!("连接 {}", target.address());
            rows.push(
                div()
                    .mx_2()
                    .mb_1()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .justify_between()
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
                    .child(div().min_w_0().truncate().child(label))
                    // 有匹配的主机时回车打开的是第一台主机，这一行要用鼠标点。
                    .when(sections.is_empty(), |row| {
                        row.child(div().text_xs().text_color(rgb(theme::TEXT_MUTED)).child("回车"))
                    }),
            );
        }

        for section in &sections {
            let collapsed = !searching && self.collapsed.contains(&section.title);
            let title = section.title.clone();
            rows.push(
                div()
                    .px_3()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_MUTED))
                    .hover(|style| style.text_color(rgb(theme::TEXT)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            this.toggle_section(&title, cx)
                        }),
                    )
                    .child(div().w(px(10.)).child(if collapsed { "▸" } else { "▾" }))
                    .child(div().flex_1().min_w_0().truncate().child(section.title.clone()))
                    .child(section.hosts.len().to_string()),
            );
            if collapsed {
                continue;
            }
            for &index in &section.hosts {
                let host = &self.hosts[index];
                let deletable = index < self.saved_count;
                let selected = active_host == Some(host.name.as_str());
                let dot_color = states.get(host.name.as_str()).map(|state| state_color(*state));
                let target = host.clone();
                rows.push(
                    div()
                        .group("host-row")
                        .mx_2()
                        .pl_4()
                        .pr_1()
                        .py_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(selected, |row| row.bg(rgb(theme::HOVER_BG)))
                        .hover(|style| style.bg(rgb(theme::HOVER_BG)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                this.open_host(target.clone(), window, cx)
                            }),
                        )
                        // 没有打开会话的主机也留出圆点的位置，名称才能对齐。
                        .child(dot(dot_color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().truncate().child(host.name.clone()))
                                .child(
                                    div()
                                        .truncate()
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
                                    // 删除按钮只在鼠标移到这一行时出现，平时不抢视线。
                                    .opacity(0.)
                                    .group_hover("host-row", |style| style.opacity(1.))
                                    .hover(|style| {
                                        style.bg(rgb(theme::BORDER)).text_color(rgb(theme::ERROR))
                                    })
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
                        }),
                );
            }
        }

        let hint = if !rows.is_empty() || self.error.is_some() {
            None
        } else if searching {
            Some("没有匹配的主机")
        } else {
            Some("还没有主机。点上方的「+ 添加」，或在 ~/.ssh/config 里配置。")
        };
        let hint = hint
            .map(|text| div().px_3().py_2().text_color(rgb(theme::TEXT_MUTED)).child(text));
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
            .w(px(240.))
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
            .child(div().px_2().pb_2().child(self.search.clone()))
            .children(error)
            .children(hint)
            .child(div().id("host-list").flex_1().overflow_y_scroll().pb_2().children(rows))
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.iter().enumerate().map(|(index, view)| {
            let active = index == self.active;
            let title = view.read(cx).tab_title();
            let state = view.read(cx).state();
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
                .child(dot(Some(state_color(state))))
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

    /// 底部状态栏：当前标签页的连接状态、延迟和终端尺寸。只显示真实测到的值。
    fn render_status_bar(&self, cx: &mut Context<Self>) -> Div {
        let bar = div()
            .h(px(24.))
            .w_full()
            .px_3()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(rgb(theme::TEXT_MUTED))
            .bg(rgb(theme::SIDEBAR_BG))
            .border_t_1()
            .border_color(rgb(theme::BORDER));
        let Some(tab) = self.tabs.get(self.active) else {
            return bar.child("未连接");
        };
        let view = tab.read(cx);
        let state = view.state();
        let address = view.host().address();
        let summary = match state {
            SessionState::Connecting => format!("正在连接 {address}"),
            SessionState::NeedsInput => format!("等待输入凭据 {address}"),
            SessionState::Connected => format!("已连接 {address}"),
            SessionState::Ended => format!("已断开 {address}"),
            SessionState::Failed => format!("连接失败 {address}"),
        };
        let (cols, rows) = view.grid_size();

        // 延迟只在连接存活时有意义。
        let latency = (state == SessionState::Connected).then(|| view.latency()).and_then(|meter| {
            let color = match meter.level()? {
                LatencyLevel::Good => theme::SUCCESS,
                LatencyLevel::Fair => theme::NOTICE,
                LatencyLevel::Poor => theme::ERROR,
            };
            let text = if meter.timed_out() {
                "无响应".to_string()
            } else {
                format!("{} ms", meter.millis()?)
            };
            Some(div().text_color(rgb(color)).child(text))
        });

        bar.child(dot(Some(state_color(state))))
            .child(div().flex_1().min_w_0().truncate().child(summary))
            .children(latency)
            .child(format!("{cols}×{rows}"))
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
                    .child(field("分组", &form.group))
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

fn state_color(state: SessionState) -> u32 {
    match state {
        SessionState::Connecting | SessionState::NeedsInput => theme::NOTICE,
        SessionState::Connected => theme::SUCCESS,
        SessionState::Ended => theme::TEXT_MUTED,
        SessionState::Failed => theme::ERROR,
    }
}

/// 一台主机开了多个标签页时，侧栏显示其中哪个状态：需要用户处理的排在前面。
fn state_rank(state: SessionState) -> u8 {
    match state {
        SessionState::Ended => 0,
        SessionState::Connected => 1,
        SessionState::Connecting => 2,
        SessionState::Failed => 3,
        SessionState::NeedsInput => 4,
    }
}

/// 状态圆点；颜色为 None 时只占位不显示。
fn dot(color: Option<u32>) -> Div {
    let dot = div().w(px(7.)).h(px(7.)).flex_shrink_0().rounded_full();
    match color {
        Some(color) => dot.bg(rgb(color)),
        None => dot,
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
                    .child(content)
                    .child(self.render_status_bar(cx)),
            )
            .children(form)
    }
}
