//! 添加 / 编辑主机的对话框。
//!
//! 这里只管界面：收集文字、显示错误、切换焦点。字段怎么校验在
//! `nexsh_core::draft` 里，保存到哪里由打开对话框的一方决定（见 [`HostFormEvent`]）。

use gpui::{
    Context, Div, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, SharedString, Task, Window, div, prelude::*, px, rgb, rgba,
};
use nexsh_core::secrets::{self, SecretKind};
use nexsh_core::{
    AuthMethod, ConnectOptions, Field, FieldError, Host, HostInput, ProbeReport, SshError,
    spawn_probe,
};

use crate::text_input::TextInput;
use crate::theme;

/// 对钥匙串里一条秘密要做的事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretChange {
    Keep,
    Set(String),
    Clear,
}

/// 通过校验、等待保存的表单内容。
#[derive(Clone, Debug)]
pub struct HostDraft {
    pub host: Host,
    /// 编辑已有主机时它原来的名称；新建时为 None。
    pub original_name: Option<String>,
    pub password: SecretChange,
    pub passphrase: SecretChange,
}

pub enum HostFormEvent {
    Submit { draft: HostDraft, connect: bool },
    Cancel,
}

/// 测试连接的结论。
enum Verdict {
    Success,
    /// 服务器能连上，但没能验证登录（缺密码或口令）。
    Reachable,
    Failure,
}

enum Probe {
    Idle,
    /// 丢掉任务即取消等待。
    Running(#[allow(dead_code)] Task<()>),
    /// 结论只对当时的表单内容有效，内容改了就不再显示。
    Done { input: HostInput, verdict: Verdict, message: String },
}

pub struct HostForm {
    focus_handle: FocusHandle,
    original: Option<Host>,
    /// 其他主机已经占用的名称。
    taken_names: Vec<String>,
    /// 已有的分组，显示成可以点选的标签。
    groups: Vec<String>,
    address: Entity<TextInput>,
    port: Entity<TextInput>,
    user: Entity<TextInput>,
    name: Entity<TextInput>,
    password: Entity<TextInput>,
    identity_file: Entity<TextInput>,
    passphrase: Entity<TextInput>,
    group: Entity<TextInput>,
    note: Entity<TextInput>,
    auth: AuthMethod,
    errors: Vec<FieldError>,
    /// 保存失败的原因，由打开对话框的一方填入。
    failure: Option<String>,
    /// 钥匙串里是否已经存着这台主机的密码、口令。
    saved_password: bool,
    saved_passphrase: bool,
    probe: Probe,
}

impl EventEmitter<HostFormEvent> for HostForm {}

impl HostForm {
    pub fn new(
        original: Option<Host>,
        taken_names: Vec<String>,
        groups: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = original.as_ref().map(HostInput::from_host).unwrap_or_default();
        let default_user = nexsh_core::host::default_user();
        let mut input = |placeholder: String, text: &str, masked: bool| {
            let text = text.to_string();
            cx.new(|cx| {
                let mut input = TextInput::new(placeholder, cx);
                if masked {
                    input = input.masked();
                }
                input.set_text(&text, cx);
                input
            })
        };
        let address = input("主机名或 IP，也可以粘贴 user@host:端口".into(), &initial.address, false);
        let port = input("22".into(), &initial.port, false);
        // 留空时会用什么用户名，直接写在占位文字里，不让默认值变成一个意外。
        let user = input(format!("留空则用 {default_user}"), &initial.user, false);
        let name = input("留空则用地址".into(), &initial.name, false);
        let password = input(String::new(), "", true);
        let identity_file = input("例如 ~/.ssh/id_ed25519".into(), &initial.identity_file, false);
        let passphrase = input(String::new(), "", true);
        let group = input("输入新分组，或点下面已有的".into(), &initial.group, false);
        let note = input("可留空".into(), &initial.note, false);

        // 改动一个字段后，它的错误提示随即消失；测试结论是否过期也要重新判断。
        let watched = [
            (&address, Some(Field::Address)),
            (&port, Some(Field::Port)),
            (&user, Some(Field::User)),
            (&name, Some(Field::Name)),
            (&identity_file, Some(Field::IdentityFile)),
            (&password, None),
            (&passphrase, None),
            (&group, None),
            (&note, None),
        ];
        for (input, field) in watched {
            // 输入框重绘、标红时也会通知；只有文字真的变了才处理。
            let mut seen = input.read(cx).revision();
            cx.observe(input, move |form: &mut Self, input, cx| {
                let revision = input.read(cx).revision();
                if revision == seen {
                    return;
                }
                seen = revision;
                if let Some(field) = field {
                    form.errors.retain(|error| error.field != field);
                    input.update(cx, |input, cx| input.set_invalid(false, cx));
                }
                form.failure = None;
                cx.notify();
            })
            .detach();
        }
        // 往地址栏粘贴一整个目标时，拆到各个字段里。只在粘贴时做：
        // 逐字输入时拆分会把还没打完的内容拆错。
        cx.observe(&address, |form: &mut Self, input, cx| {
            if input.update(cx, |input, _| input.take_pasted()) {
                form.split_address(cx);
            }
        })
        .detach();

        let mut form = Self {
            focus_handle: cx.focus_handle(),
            auth: initial.auth,
            original,
            taken_names,
            groups,
            address,
            port,
            user,
            name,
            password,
            identity_file,
            passphrase,
            group,
            note,
            errors: Vec::new(),
            failure: None,
            saved_password: false,
            saved_passphrase: false,
            probe: Probe::Idle,
        };
        form.refresh_secret_placeholders(cx);
        form.look_up_saved_secrets(cx);
        let first = form.address.read(cx).focus_handle(cx);
        window.focus(&first);
        form
    }

    /// 编辑时查一下钥匙串里有没有这台主机的密码。只问有没有，不把内容读进界面。
    fn look_up_saved_secrets(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.original.as_ref().map(|host| host.name.clone()) else { return };
        let lookup = nexsh_core::spawn_blocking(move || {
            let exists = |kind| matches!(secrets::load(kind, &name), Ok(Some(_)));
            (exists(SecretKind::Password), exists(SecretKind::Passphrase))
        });
        cx.spawn(async move |this, cx| {
            if let Ok((password, passphrase)) = lookup.await {
                let _ = this.update(cx, |form, cx| {
                    form.saved_password = password;
                    form.saved_passphrase = passphrase;
                    form.refresh_secret_placeholders(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn refresh_secret_placeholders(&mut self, cx: &mut Context<Self>) {
        let password = if self.saved_password {
            "已保存，留空则保持不变"
        } else {
            "留空则每次连接时询问"
        };
        let passphrase = if self.saved_passphrase {
            "已保存，留空则保持不变"
        } else {
            "私钥没有口令就留空"
        };
        self.password.update(cx, |input, cx| input.set_placeholder(password, cx));
        self.passphrase.update(cx, |input, cx| input.set_placeholder(passphrase, cx));
    }

    /// 预先填好分组（从某个分组的菜单里新建主机时）。
    pub fn set_group(&mut self, group: &str, cx: &mut Context<Self>) {
        self.group.update(cx, |input, cx| input.set_text(group, cx));
    }

    pub fn set_failure(&mut self, message: String, cx: &mut Context<Self>) {
        self.failure = Some(message);
        cx.notify();
    }

    fn input(&self, cx: &Context<Self>) -> HostInput {
        let text = |input: &Entity<TextInput>| input.read(cx).text().to_string();
        HostInput {
            address: text(&self.address),
            port: text(&self.port),
            user: text(&self.user),
            name: text(&self.name),
            group: text(&self.group),
            note: text(&self.note),
            identity_file: text(&self.identity_file),
            auth: self.auth,
        }
    }

    fn input_for(&self, field: Field) -> &Entity<TextInput> {
        match field {
            Field::Address => &self.address,
            Field::Port => &self.port,
            Field::User => &self.user,
            Field::Name => &self.name,
            Field::IdentityFile => &self.identity_file,
        }
    }

    /// 地址栏里是 `user@host:port` 或 ssh 命令时，拆到各字段。
    fn split_address(&mut self, cx: &mut Context<Self>) {
        let mut input = self.input(cx);
        if !input.split_address() {
            return;
        }
        self.address.update(cx, |field, cx| field.set_text(&input.address, cx));
        self.port.update(cx, |field, cx| field.set_text(&input.port, cx));
        self.user.update(cx, |field, cx| field.set_text(&input.user, cx));
        self.identity_file.update(cx, |field, cx| field.set_text(&input.identity_file, cx));
        self.auth = input.auth;
        cx.notify();
    }

    /// 校验表单。有错时在各字段下显示，并把焦点放到第一个出错的字段上。
    fn validate(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Host> {
        self.split_address(cx);
        match self.input(cx).build(&self.taken_names) {
            Ok(host) => {
                self.errors.clear();
                Some(host)
            }
            Err(errors) => {
                for error in &errors {
                    self.input_for(error.field).update(cx, |input, cx| input.set_invalid(true, cx));
                }
                if let Some(first) = errors.first() {
                    let handle = self.input_for(first.field).read(cx).focus_handle(cx);
                    window.focus(&handle);
                }
                self.errors = errors;
                cx.notify();
                None
            }
        }
    }

    /// 这一种秘密在保存时该怎么处理。`relevant` 表示当前的认证方式用得到它。
    fn secret_change(&self, input: &Entity<TextInput>, relevant: bool, cx: &Context<Self>) -> SecretChange {
        let typed = input.read(cx).text();
        if !relevant {
            // 换了认证方式后，用不到的旧秘密不留在钥匙串里。
            return if self.original.is_some() { SecretChange::Clear } else { SecretChange::Keep };
        }
        if typed.is_empty() { SecretChange::Keep } else { SecretChange::Set(typed.to_string()) }
    }

    fn submit(&mut self, connect: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.validate(window, cx) else { return };
        let draft = HostDraft {
            original_name: self.original.as_ref().map(|host| host.name.clone()),
            password: self.secret_change(&self.password, host.auth == AuthMethod::Password, cx),
            passphrase: self.secret_change(&self.passphrase, host.auth == AuthMethod::Key, cx),
            host,
        };
        cx.emit(HostFormEvent::Submit { draft, connect });
    }

    /// 不再使用已保存的密码或口令：保存时从钥匙串里删掉。
    fn forget_secret(&mut self, kind: SecretKind, cx: &mut Context<Self>) {
        let Some(name) = self.original.as_ref().map(|host| host.name.clone()) else { return };
        match kind {
            SecretKind::Password => self.saved_password = false,
            SecretKind::Passphrase => self.saved_passphrase = false,
        }
        // 结果不影响界面：删不掉时钥匙串本来就不可用。
        drop(nexsh_core::spawn_blocking(move || secrets::delete(kind, &name)));
        self.refresh_secret_placeholders(cx);
        cx.notify();
    }

    /// 用表单当前的内容登录一次，不保存。再点一次取消。
    fn toggle_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.probe, Probe::Running(_)) {
            self.probe = Probe::Idle;
            cx.notify();
            return;
        }
        let Some(host) = self.validate(window, cx) else { return };
        let input = self.input(cx);
        let typed = |field: &Entity<TextInput>| {
            Some(field.read(cx).text().to_string()).filter(|text| !text.is_empty())
        };
        let mut options = ConnectOptions::new(host, 80, 24);
        options.password = typed(&self.password).filter(|_| self.auth == AuthMethod::Password);
        options.key_passphrase = typed(&self.passphrase).filter(|_| self.auth == AuthMethod::Key);
        // 没填的秘密用这台主机已经保存的；新主机还没有保存过任何东西。
        options.saved_secrets = self.original.as_ref().map(|host| host.name.clone());

        let outcome = spawn_probe(options);
        let task = cx.spawn(async move |this, cx| {
            let (verdict, message) = match outcome.await {
                Ok(result) => describe_probe(result),
                Err(_) => (Verdict::Failure, "测试没有完成".to_string()),
            };
            let _ = this.update(cx, |form, cx| {
                form.probe = Probe::Done { input, verdict, message };
                cx.notify();
            });
        });
        self.probe = Probe::Running(task);
        cx.notify();
    }

    /// 当前显示的输入框，按从上到下、从左到右的顺序。Tab 依此切换。
    fn visible_inputs(&self) -> Vec<&Entity<TextInput>> {
        let mut inputs = vec![&self.address, &self.port, &self.user, &self.name];
        match self.auth {
            AuthMethod::Auto => {}
            AuthMethod::Password => inputs.push(&self.password),
            AuthMethod::Key => inputs.extend([&self.identity_file, &self.passphrase]),
        }
        inputs.extend([&self.group, &self.note]);
        inputs
    }

    fn focus_next(&self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let handles: Vec<FocusHandle> =
            self.visible_inputs().into_iter().map(|input| input.read(cx).focus_handle(cx)).collect();
        let count = handles.len();
        let next = match handles.iter().position(|handle| handle.is_focused(window)) {
            Some(index) if backwards => (index + count - 1) % count,
            Some(index) => (index + 1) % count,
            None => 0,
        };
        window.focus(&handles[next]);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            // 回车执行主按钮：新建时保存并连接，编辑时保存。
            "enter" => self.submit(self.original.is_none(), window, cx),
            "escape" => cx.emit(HostFormEvent::Cancel),
            "tab" => {
                self.focus_next(event.keystroke.modifiers.shift, window, cx);
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn set_auth(&mut self, auth: AuthMethod, cx: &mut Context<Self>) {
        self.auth = auth;
        self.errors.retain(|error| error.field != Field::IdentityFile);
        self.identity_file.update(cx, |input, cx| input.set_invalid(false, cx));
        cx.notify();
    }

    /// 标签 + 输入框 + 这个字段的错误。
    fn field(&self, label: &'static str, input: &Entity<TextInput>, field: Option<Field>) -> Div {
        let error = field
            .and_then(|field| self.errors.iter().find(|error| error.field == field))
            .map(|error| error.message.clone());
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_color(rgb(theme::TEXT_MUTED)).child(label))
            .child(input.clone())
            .children(error.map(|message| {
                div().text_xs().text_color(rgb(theme::ERROR)).child(message)
            }))
    }

    fn render_auth(&self, cx: &mut Context<Self>) -> Div {
        let options = [
            (AuthMethod::Auto, "自动"),
            (AuthMethod::Password, "密码"),
            (AuthMethod::Key, "私钥"),
        ];
        let segments = options.into_iter().map(|(method, label)| {
            let selected = self.auth == method;
            div()
                .flex_1()
                .py_1()
                .flex()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .when(selected, |segment| {
                    segment.bg(rgb(theme::HOVER_BG)).text_color(rgb(theme::TEXT))
                })
                .when(!selected, |segment| {
                    segment
                        .text_color(rgb(theme::TEXT_MUTED))
                        .hover(|style| style.text_color(rgb(theme::TEXT)))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        this.set_auth(method, cx)
                    }),
                )
                .child(label)
        });
        let explanation = match self.auth {
            AuthMethod::Auto => "依次尝试 ssh-agent 和 ~/.ssh 下的默认私钥，都不行再询问密码。",
            AuthMethod::Password => "密码保存在系统钥匙串里，不写入配置文件。",
            AuthMethod::Key => "只用这把私钥登录，失败时不会改用密码。口令保存在系统钥匙串里。",
        };

        let forget = |kind: SecretKind, label: &'static str, cx: &mut Context<Self>| {
            div()
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(theme::TEXT_MUTED))
                .hover(|style| style.text_color(rgb(theme::ERROR)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        this.forget_secret(kind, cx)
                    }),
                )
                .child(label)
        };

        let section = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_color(rgb(theme::TEXT_MUTED)).child("认证方式"))
            .child(
                div()
                    .p_0p5()
                    .flex()
                    .gap_1()
                    .rounded_sm()
                    .bg(rgb(theme::INPUT_BG))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .children(segments),
            )
            .child(div().text_xs().text_color(rgb(theme::TEXT_MUTED)).child(explanation));
        match self.auth {
            AuthMethod::Auto => section,
            AuthMethod::Password => section
                .child(self.field("密码", &self.password, None))
                .when(self.saved_password, |section| {
                    section.child(forget(SecretKind::Password, "删除已保存的密码", cx))
                }),
            AuthMethod::Key => section
                .child(self.field("私钥文件", &self.identity_file, Some(Field::IdentityFile)))
                .child(self.field("私钥口令", &self.passphrase, None))
                .when(self.saved_passphrase, |section| {
                    section.child(forget(SecretKind::Passphrase, "删除已保存的口令", cx))
                }),
        }
    }

    fn render_group(&self, cx: &mut Context<Self>) -> Div {
        let current = self.group.read(cx).text().trim().to_string();
        let chips: Vec<Div> = self
            .groups
            .iter()
            .filter(|group| **group != current)
            .take(8)
            .map(|group| {
                let name = group.clone();
                div()
                    .px_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_MUTED))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .hover(|style| style.text_color(rgb(theme::TEXT)).bg(rgb(theme::HOVER_BG)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            this.group.update(cx, |input, cx| input.set_text(&name, cx));
                        }),
                    )
                    .child(group.clone())
            })
            .collect();
        self.field("分组", &self.group, None)
            .when(!chips.is_empty(), |field| {
                field.child(div().flex().flex_wrap().gap_1().children(chips))
            })
    }

    /// 测试结论。只在它对应的表单内容没变时显示。
    fn render_probe(&self, cx: &Context<Self>) -> Option<Div> {
        let (color, message) = match &self.probe {
            Probe::Idle => return None,
            Probe::Running(_) => (theme::TEXT_MUTED, "正在测试连接…".to_string()),
            Probe::Done { input, verdict, message } => {
                if *input != self.input(cx) {
                    return None;
                }
                let color = match verdict {
                    Verdict::Success => theme::SUCCESS,
                    Verdict::Reachable => theme::NOTICE,
                    Verdict::Failure => theme::ERROR,
                };
                (color, message.clone())
            }
        };
        Some(div().text_xs().text_color(rgb(color)).child(message))
    }
}

fn describe_probe(result: Result<ProbeReport, SshError>) -> (Verdict, String) {
    match result {
        Ok(report) => {
            let mut message = format!("登录成功，用时 {} ms。", report.elapsed.as_millis());
            if let Some(fingerprint) = report.new_host_key {
                message.push_str(&format!("这是第一次连接这台主机，密钥指纹 {fingerprint}"));
            }
            (Verdict::Success, message)
        }
        Err(SshError::PasswordRequired) => (
            Verdict::Reachable,
            "能连上服务器，但登录需要密码。可以在上面填写，或留到连接时输入。".to_string(),
        ),
        Err(SshError::PassphraseRequired(path)) => (
            Verdict::Reachable,
            format!("能连上服务器，但私钥 {} 需要口令。", path.display()),
        ),
        Err(error) => (Verdict::Failure, error.to_string()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Secondary,
    /// 没有边框的次要操作。
    Quiet,
}

/// 按钮不随对话框变窄而压缩或换行。
fn button(label: impl Into<SharedString>, kind: ButtonKind) -> Div {
    let button = div()
        .px_3()
        .py_1()
        .flex_shrink_0()
        .whitespace_nowrap()
        .rounded_sm()
        .cursor_pointer()
        .border_1()
        .child(label.into());
    match kind {
        ButtonKind::Primary => button
            .bg(rgb(theme::ACCENT))
            .border_color(rgb(theme::ACCENT))
            .text_color(rgb(theme::WINDOW_BG))
            .hover(|style| style.opacity(0.9)),
        ButtonKind::Secondary => button
            .border_color(rgb(theme::BORDER))
            .text_color(rgb(theme::TEXT))
            .hover(|style| style.bg(rgb(theme::HOVER_BG))),
        ButtonKind::Quiet => button
            .border_color(rgba(0x00000000))
            .text_color(rgb(theme::TEXT_MUTED))
            .hover(|style| style.text_color(rgb(theme::TEXT)).bg(rgb(theme::HOVER_BG))),
    }
}

impl Focusable for HostForm {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HostForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editing = self.original.is_some();
        let probing = matches!(self.probe, Probe::Running(_));

        let actions = div()
            .flex()
            .gap_2()
            .child(button("取消", ButtonKind::Quiet).on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _window, cx| cx.emit(HostFormEvent::Cancel)),
            ))
            // 新建时主操作是"保存并连接"，只想存下来的用旁边的"保存"。
            .when(!editing, |actions| {
                actions.child(
                    button("保存", ButtonKind::Secondary).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.submit(false, window, cx)
                        }),
                    ),
                )
            })
            .child(
                button(if editing { "保存" } else { "保存并连接" }, ButtonKind::Primary)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.submit(!editing, window, cx)
                        }),
                    ),
            );

        let card = div()
            .id("host-form")
            .w(px(480.))
            .max_w_full()
            // 窗口比对话框矮时，对话框内部滚动，按钮始终够得着。
            .max_h_full()
            .overflow_y_scroll()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(theme::SIDEBAR_BG))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .rounded_md()
            .child(div().text_size(px(15.)).child(if editing { "编辑主机" } else { "添加主机" }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        self.field("地址", &self.address, Some(Field::Address)).flex_1(),
                    )
                    .child(self.field("端口", &self.port, Some(Field::Port)).w(px(88.))),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.field("用户名", &self.user, Some(Field::User)).flex_1())
                    .child(self.field("名称", &self.name, Some(Field::Name)).flex_1()),
            )
            .child(self.render_auth(cx))
            .child(self.render_group(cx))
            .child(self.field("备注", &self.note, None))
            .children(self.render_probe(cx))
            .children(self.failure.clone().map(|message| {
                div().text_xs().text_color(rgb(theme::ERROR)).child(message)
            }))
            .child(
                div()
                    .pt_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        button(if probing { "取消测试" } else { "测试连接" }, ButtonKind::Secondary)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.toggle_probe(window, cx)
                                }),
                            ),
                    )
                    .child(actions),
            );

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            // 窗口很小时对话框贴着这圈留白，不会顶到窗口边缘。
            .p_4()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            // 挡住遮罩下面的主机列表和终端，鼠标点不穿。
            .occlude()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(card)
    }
}
