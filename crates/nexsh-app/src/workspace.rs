//! 主窗口：左侧主机列表，右侧标签页和终端。

mod sidebar;

use std::collections::HashSet;

use gpui::{
    ClipboardItem, Context, Div, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, Pixels, Point, Subscription, Window, div, prelude::*, px, rgb,
};
use nexsh_core::secrets::{self, SecretKind};
use nexsh_core::tree::{self, Row};
use nexsh_core::{Host, HostStore, LatencyLevel, search, ssh_config};

use crate::icons::{Icon, icon};
use crate::host_form::{HostDraft, HostForm, HostFormEvent, SecretChange};
use crate::terminal_view::{SessionState, TerminalView};
use crate::text_input::TextInput;
use crate::theme;

/// 右键菜单是针对谁的。
#[derive(Clone, Debug, PartialEq, Eq)]
enum MenuTarget {
    /// `Workspace::hosts` 的下标。
    Host(usize),
    /// 分组的完整路径。
    Group(String),
}

/// 侧栏的右键菜单。
struct Menu {
    position: Point<Pixels>,
    target: MenuTarget,
    /// 已经点过一次"删除"，再点一次才真的删。
    confirming_delete: bool,
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
    /// 被折叠的分组（按完整路径）。搜索时不生效：有匹配的分组总是展开。
    collapsed: HashSet<String>,
    /// 在搜索框里用上下键选中的是当前可见主机里的第几台。
    selected: usize,
    menu: Option<Menu>,
    sidebar_visible: bool,
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
        let search = cx.new(|cx| TextInput::new("搜索，或输入 user@host 连接", cx).bare());
        // 搜索框内容变化时重新筛选列表，选中项回到第一台。
        let mut seen = 0;
        cx.observe(&search, move |this: &mut Self, search, cx| {
            let revision = search.read(cx).revision();
            if revision != seen {
                seen = revision;
                this.selected = 0;
            }
            cx.notify();
        })
        .detach();
        let mut workspace = Self {
            focus_handle: cx.focus_handle(),
            search,
            collapsed: HashSet::new(),
            selected: 0,
            menu: None,
            sidebar_visible: true,
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

    fn query(&self, cx: &Context<Self>) -> String {
        self.search.read(cx).text().trim().to_string()
    }

    /// 当前查询下侧栏里看得见的主机，按显示顺序。
    fn visible_hosts(&self, query: &str) -> Vec<usize> {
        tree::rows(&self.hosts, query, &self.collapsed)
            .into_iter()
            .filter_map(|row| match row {
                Row::Host { index, .. } => Some(index),
                Row::Group { .. } => None,
            })
            .collect()
    }

    /// 查询能解析成一个地址、且列表里没有这个地址时，可以直接连接它。
    fn quick_target(&self, query: &str) -> Option<Host> {
        let target = search::quick_target(query)?;
        let address = target.address();
        (!self.hosts.iter().any(|host| host.address() == address)).then_some(target)
    }

    /// 在搜索框里按回车：打开选中的主机；一台都没匹配上就把输入当作地址连接。
    fn submit_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let visible = self.visible_hosts(&query);
        let host = match visible.get(self.selected.min(visible.len().saturating_sub(1))) {
            Some(&index) => self.hosts.get(index).cloned(),
            None => self.quick_target(&query),
        };
        if let Some(host) = host {
            self.search.update(cx, |input, cx| input.clear(cx));
            self.open_host(host, window, cx);
        }
    }

    /// 上下键在可见的主机之间移动选中项，到头后回绕。
    fn move_selection(&mut self, backwards: bool, cx: &mut Context<Self>) {
        let count = self.visible_hosts(&self.query(cx)).len();
        if count == 0 {
            return;
        }
        let current = self.selected.min(count - 1);
        self.selected = if backwards { (current + count - 1) % count } else { (current + 1) % count };
        cx.notify();
    }

    fn toggle_group(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_string());
        }
        cx.notify();
    }

    fn open_menu(&mut self, target: MenuTarget, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.menu = Some(Menu { position, target, confirming_delete: false });
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    fn copy_address(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(host) = self.hosts.get(index) {
            cx.write_to_clipboard(ClipboardItem::new_string(host.address()));
        }
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
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
    /// `group` 是新建时预先填好的分组。
    fn open_form(
        &mut self,
        original: Option<Host>,
        group: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.store.as_ref() else { return };
        let editing = original.as_ref().map(|host| host.name.as_str());
        // 名称在整个列表里唯一，~/.ssh/config 里的也算。
        let taken_names = self
            .hosts
            .iter()
            .map(|host| host.name.clone())
            .filter(|name| Some(name.as_str()) != editing)
            .collect();
        let groups = tree::group_paths(store.hosts());

        let form = cx.new(|cx| {
            let mut form = HostForm::new(original, taken_names, groups, window, cx);
            if let Some(group) = &group {
                form.set_group(group, cx);
            }
            form
        });
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

    /// 菜单的 Esc，以及搜索框里的回车、上下键和 Esc。对话框自己处理按键，其余按键归终端。
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if self.menu.is_some() && key == "escape" {
            self.close_menu(cx);
            cx.stop_propagation();
            return;
        }
        if !self.search.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        match key {
            "enter" => self.submit_search(window, cx),
            "down" => self.move_selection(false, cx),
            "up" => self.move_selection(true, cx),
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

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.iter().enumerate().map(|(index, view)| {
            let active = index == self.active;
            let title = view.read(cx).tab_title();
            let state = view.read(cx).state();
            let text_color = if active { theme::TEXT } else { theme::TEXT_MUTED };
            // 标签是悬在栏里的圆角块：当前的那个填实，其余的只在悬停时显形。
            div()
                .group("tab")
                .h(px(28.))
                .pl(px(10.))
                .pr(px(6.))
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .rounded(px(theme::RADIUS_ITEM))
                .cursor_pointer()
                .text_color(rgb(text_color))
                .when(active, |tab| tab.bg(rgb(theme::SELECTED_BG)))
                .when(!active, |tab| tab.hover(|style| style.bg(rgb(theme::HOVER_BG))))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.activate_tab(index, window, cx)
                    }),
                )
                .child(dot(Some(state_color(state))))
                .child(div().max_w(px(180.)).truncate().child(title))
                .child(
                    div()
                        .w(px(18.))
                        .h(px(18.))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        // 关闭按钮在当前标签上常显，其余标签悬停时才出现，栏里不至于满是叉。
                        .when(!active, |close| {
                            close.opacity(0.).group_hover("tab", |style| style.opacity(1.))
                        })
                        .hover(|style| style.bg(rgb(theme::BORDER)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                // 不让点击继续冒泡到标签本身，否则会去激活一个已关闭的标签。
                                cx.stop_propagation();
                                this.close_tab(index, window, cx);
                            }),
                        )
                        .child(icon(Icon::Close, 12., text_color)),
                )
        });

        let sidebar_toggle = div()
            .w(px(28.))
            .h(px(28.))
            .mr_1()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_ITEM))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(theme::HOVER_BG)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| this.toggle_sidebar(cx)),
            )
            .child(icon(Icon::Sidebar, 15., theme::TEXT_MUTED));

        div()
            .h(px(theme::BAR_HEIGHT))
            .w_full()
            .px_2()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            // 标签多到放不下时裁掉，不把窗口撑宽。
            .overflow_hidden()
            .bg(rgb(theme::SIDEBAR_BG))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .child(sidebar_toggle)
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
            .text_size(px(11.5))
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.tabs.get(self.active) {
            Some(view) => div().flex_1().min_h_0().w_full().child(view.clone()),
            None => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(rgb(theme::TEXT_FAINT))
                .child(icon(Icon::Terminal, 40., theme::BORDER))
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
            .when(self.sidebar_visible, |root| root.child(self.render_sidebar(window, cx)))
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
            .children(self.render_menu(window, cx))
    }
}
