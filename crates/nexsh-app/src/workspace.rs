//! 主窗口：左侧主机列表，右侧标签页和终端。

use std::collections::{HashMap, HashSet};

use gpui::{
    Context, Div, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    Subscription, Window, div, prelude::*, px, rgb,
};
use nexsh_core::secrets::{self, SecretKind};
use nexsh_core::{Host, HostStore, LatencyLevel, search, ssh_config};

use crate::host_form::{HostDraft, HostForm, HostFormEvent, SecretChange};
use crate::terminal_view::{SessionState, TerminalView};
use crate::text_input::TextInput;
use crate::theme;

/// 侧栏里的一个分组，`hosts` 是 `Workspace::hosts` 的下标。
struct Section {
    title: String,
    hosts: Vec<usize>,
}

/// 没有填分组的已保存主机归在这里。
const UNGROUPED: &str = "未分组";

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
    form: Option<Entity<HostForm>>,
    /// 对话框关闭时一并丢弃，不再接收它的事件。
    form_subscription: Option<Subscription>,
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
            form_subscription: None,
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
        if result.is_ok() {
            // 主机没了，它的密码也不该留在钥匙串里。读写钥匙串可能卡顿，放到后台。
            drop(nexsh_core::spawn_blocking(move || secrets::forget(&name)));
        }
        self.error = result.err().map(|error| error.to_string());
        self.rebuild_hosts();
        cx.notify();
    }

    /// 打开主机对话框；`original` 为 None 时是新建。
    fn open_form(&mut self, original: Option<Host>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.as_ref() else { return };
        let editing = original.as_ref().map(|host| host.name.as_str());
        // 名称在整个列表里唯一，~/.ssh/config 里的也算。
        let taken_names = self
            .hosts
            .iter()
            .map(|host| host.name.clone())
            .filter(|name| Some(name.as_str()) != editing)
            .collect();
        let mut groups: Vec<String> = Vec::new();
        for group in store.hosts().iter().filter_map(|host| host.group.as_ref()) {
            if !groups.contains(group) {
                groups.push(group.clone());
            }
        }

        let form = cx.new(|cx| HostForm::new(original, taken_names, groups, window, cx));
        let subscription = cx.subscribe_in(
            &form,
            window,
            |this, form, event: &HostFormEvent, window, cx| match event {
                HostFormEvent::Cancel => this.close_form(window, cx),
                HostFormEvent::Submit { draft, connect } => match this.apply_draft(draft) {
                    Ok(()) => {
                        this.error = None;
                        this.rebuild_hosts();
                        this.close_form(window, cx);
                        if *connect {
                            this.open_host(draft.host.clone(), window, cx);
                        }
                    }
                    Err(message) => form.update(cx, |form, cx| form.set_failure(message, cx)),
                },
            },
        );
        self.form = Some(form);
        self.form_subscription = Some(subscription);
        cx.notify();
    }

    fn close_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form = None;
        self.form_subscription = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// 保存对话框的内容：先写钥匙串，再写主机列表。
    /// 密码存不进去就整个不保存，不留下一台"以为存了密码"的主机。
    fn apply_draft(&mut self, draft: &HostDraft) -> Result<(), String> {
        let store = self.store.as_mut().ok_or_else(|| "主机列表不可用".to_string())?;
        let name = draft.host.name.as_str();

        if let Some(old_name) = draft.original_name.as_deref().filter(|old| *old != name) {
            // 改名时把原来保存的密码带过去；钥匙串不可用时本来就没有东西可带。
            let _ = secrets::rename(old_name, name);
        }
        let changes = [
            (SecretKind::Password, &draft.password, "密码"),
            (SecretKind::Passphrase, &draft.passphrase, "口令"),
        ];
        for (kind, change, label) in changes {
            match change {
                SecretChange::Keep => {}
                SecretChange::Clear => {
                    let _ = secrets::delete(kind, name);
                }
                SecretChange::Set(secret) => secrets::store(kind, name, secret).map_err(|error| {
                    format!("{label}没有保存：{error}。把{label}留空可以先保存主机，连接时再输入。")
                })?,
            }
        }

        let listed = match &draft.original_name {
            Some(old_name) => store.replace(old_name, draft.host.clone()),
            None => store.add(draft.host.clone()),
        };
        listed.map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())
    }

    /// 搜索框里的回车和 Esc。对话框自己处理按键，其余按键归终端。
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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
                        // 编辑和删除只在鼠标移到这一行时出现，平时不抢视线。
                        .when(deletable, |row| {
                            let action = |label: &'static str, danger: bool| {
                                div()
                                    .px_1()
                                    .rounded_sm()
                                    .text_xs()
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .opacity(0.)
                                    .group_hover("host-row", |style| style.opacity(1.))
                                    .hover(move |style| {
                                        let color = if danger { theme::ERROR } else { theme::TEXT };
                                        style.bg(rgb(theme::BORDER)).text_color(rgb(color))
                                    })
                                    .child(label)
                            };
                            let editable = host.clone();
                            row.child(action("编辑", false).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    // 不让点击冒泡到整行，否则会同时去连接。
                                    cx.stop_propagation();
                                    this.open_form(Some(editable.clone()), window, cx);
                                }),
                            ))
                            .child(action("删除", true).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.delete_host(index, cx);
                                }),
                            ))
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
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.open_form(None, window, cx)
                    }),
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
            .children(self.form.clone())
    }
}
