//! nexsh 桌面界面入口。

// Windows 上发布版不弹出控制台窗口。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod terminal_view;
mod theme;
mod workspace;

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};

use workspace::Workspace;

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("nexsh".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(Workspace::new),
        )
        .expect("无法创建窗口");
        cx.activate(true);
    });
}
