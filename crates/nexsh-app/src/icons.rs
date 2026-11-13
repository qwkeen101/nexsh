//! 界面图标。每个图标是一小段 SVG，编译进程序里；GPUI 把它当作蒙版，用文字颜色上色。

use std::borrow::Cow;

use gpui::{AssetSource, SharedString, Svg, prelude::*, px, rgb, svg};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    ChevronRight,
    ChevronDown,
    Folder,
    Plus,
    Search,
    Pencil,
    Trash,
    Close,
    Sidebar,
    Copy,
    Terminal,
    More,
}

impl Icon {
    const ALL: [Icon; 12] = [
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::Folder,
        Icon::Plus,
        Icon::Search,
        Icon::Pencil,
        Icon::Trash,
        Icon::Close,
        Icon::Sidebar,
        Icon::Copy,
        Icon::Terminal,
        Icon::More,
    ];

    fn path(self) -> &'static str {
        match self {
            Icon::ChevronRight => "icons/chevron-right.svg",
            Icon::ChevronDown => "icons/chevron-down.svg",
            Icon::Folder => "icons/folder.svg",
            Icon::Plus => "icons/plus.svg",
            Icon::Search => "icons/search.svg",
            Icon::Pencil => "icons/pencil.svg",
            Icon::Trash => "icons/trash.svg",
            Icon::Close => "icons/close.svg",
            Icon::Sidebar => "icons/sidebar.svg",
            Icon::Copy => "icons/copy.svg",
            Icon::Terminal => "icons/terminal.svg",
            Icon::More => "icons/more.svg",
        }
    }

    /// 图标的线条，画在 24×24 的画布上。
    fn shapes(self) -> &'static str {
        match self {
            Icon::ChevronRight => r#"<path d="M9 6l6 6-6 6"/>"#,
            Icon::ChevronDown => r#"<path d="M6 9l6 6 6-6"/>"#,
            Icon::Folder => {
                r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#
            }
            Icon::Plus => r#"<path d="M12 5v14M5 12h14"/>"#,
            Icon::Search => r#"<circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/>"#,
            Icon::Pencil => r#"<path d="M4 20h4L19 9l-4-4L4 16z"/>"#,
            Icon::Trash => r#"<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>"#,
            Icon::Close => r#"<path d="M6 6l12 12M18 6L6 18"/>"#,
            Icon::Sidebar => r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/>"#,
            Icon::Copy => {
                r#"<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a1 1 0 0 1 1-1h10"/>"#
            }
            Icon::Terminal => r#"<path d="M5 7l5 5-5 5M12 17h7"/>"#,
            Icon::More => {
                r#"<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>"#
            }
        }
    }

    fn document(self) -> String {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{}</svg>"##,
            self.shapes()
        )
    }
}

/// 一个图标元素，边长 `size` 像素。
pub fn icon(icon: Icon, size: f32, color: u32) -> Svg {
    svg().path(icon.path()).w(px(size)).h(px(size)).flex_shrink_0().text_color(rgb(color))
}

/// 把内置图标提供给 GPUI 的资源加载器。
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::ALL
            .into_iter()
            .find(|icon| icon.path() == path)
            .map(|icon| Cow::Owned(icon.document().into_bytes())))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Icon::ALL.into_iter().map(|icon| SharedString::from(icon.path())).collect())
    }
}
