//! 侧栏的主机树。分组名里的 `/` 表示层级：`生产/Web` 是"生产"下面的"Web"。
//!
//! 界面每次重绘都把树摊平成一列行，这里负责决定有哪些行、各缩进几层；
//! 界面只管把每一行画出来。

use std::collections::HashSet;

use crate::host::Host;
use crate::search;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Group {
        /// 从根到这个分组的完整路径，用作折叠状态的键。
        path: String,
        /// 这一级的名称。
        name: String,
        depth: usize,
        /// 分组里（含子分组）符合当前查询的主机数。
        count: usize,
        collapsed: bool,
    },
    Host {
        /// 在传入的主机列表里的下标。
        index: usize,
        depth: usize,
    },
}

#[derive(Default)]
struct Node {
    name: String,
    path: String,
    children: Vec<Node>,
    hosts: Vec<usize>,
}

impl Node {
    fn count(&self) -> usize {
        self.hosts.len() + self.children.iter().map(Node::count).sum::<usize>()
    }

    /// 沿路径找到（或新建）子分组。分组按第一次出现的顺序排列。
    fn descend(&mut self, segment: &str) -> &mut Node {
        let position = match self.children.iter().position(|child| child.name == segment) {
            Some(position) => position,
            None => {
                let path = if self.path.is_empty() {
                    segment.to_string()
                } else {
                    format!("{}/{segment}", self.path)
                };
                self.children.push(Node { name: segment.to_string(), path, ..Default::default() });
                self.children.len() - 1
            }
        };
        &mut self.children[position]
    }

    fn flatten(&self, depth: usize, expand_all: bool, collapsed: &HashSet<String>, rows: &mut Vec<Row>) {
        for child in &self.children {
            let is_collapsed = !expand_all && collapsed.contains(&child.path);
            rows.push(Row::Group {
                path: child.path.clone(),
                name: child.name.clone(),
                depth,
                count: child.count(),
                collapsed: is_collapsed,
            });
            if !is_collapsed {
                child.flatten(depth + 1, expand_all, collapsed, rows);
            }
        }
        rows.extend(self.hosts.iter().map(|&index| Row::Host { index, depth }));
    }
}

/// 分组路径里的各级名称；空白的段被忽略，所以 `a//b`、`/a/b/` 都等于 `a/b`。
fn segments(group: &str) -> impl Iterator<Item = &str> {
    group.split('/').map(str::trim).filter(|segment| !segment.is_empty())
}

/// 当前要显示的行。
///
/// 有查询时只留下匹配的主机和它们所在的分组，并且全部展开：
/// 搜到的东西不应该藏在折叠的分组里。没有分组的主机排在最后，不缩进。
pub fn rows(hosts: &[Host], query: &str, collapsed: &HashSet<String>) -> Vec<Row> {
    let mut root = Node::default();
    for (index, host) in hosts.iter().enumerate() {
        if !search::matches(host, query) {
            continue;
        }
        let mut node = &mut root;
        for segment in segments(host.group.as_deref().unwrap_or_default()) {
            node = node.descend(segment);
        }
        node.hosts.push(index);
    }
    let mut rows = Vec::new();
    root.flatten(0, !query.trim().is_empty(), collapsed, &mut rows);
    rows
}

/// 已有的全部分组路径（含中间各级），按出现顺序。
pub fn group_paths(hosts: &[Host]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for host in hosts {
        let mut path = String::new();
        for segment in segments(host.group.as_deref().unwrap_or_default()) {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(segment);
            if !paths.contains(&path) {
                paths.push(path.clone());
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, group: Option<&str>) -> Host {
        let mut host = Host::parse_target(&format!("root@{name}.example.com")).unwrap();
        host.name = name.to_string();
        host.group = group.map(str::to_string);
        host
    }

    /// 把行写成一眼能看懂的文字：分组是 "名称(数量)"，折叠的加 "+"，缩进用空格。
    fn outline(hosts: &[Host], query: &str, collapsed: &[&str]) -> Vec<String> {
        let collapsed = collapsed.iter().map(|path| path.to_string()).collect();
        rows(hosts, query, &collapsed)
            .into_iter()
            .map(|row| match row {
                Row::Group { name, depth, count, collapsed, .. } => {
                    format!("{}{name}({count}){}", " ".repeat(depth), if collapsed { "+" } else { "" })
                }
                Row::Host { index, depth } => format!("{}{}", " ".repeat(depth), hosts[index].name),
            })
            .collect()
    }

    fn sample() -> Vec<Host> {
        vec![
            host("web-01", Some("生产/Web")),
            host("solo", None),
            host("db-01", Some("生产/数据库")),
            host("web-02", Some("生产 / Web/")),
            host("bastion", Some("生产")),
            host("dev", Some("开发")),
        ]
    }

    #[test]
    fn nests_groups_and_counts_descendants() {
        assert_eq!(
            outline(&sample(), "", &[]),
            [
                "生产(4)", " Web(2)", "  web-01", "  web-02", " 数据库(1)", "  db-01", " bastion",
                "开发(1)", " dev", "solo",
            ]
        );
    }

    #[test]
    fn collapsed_groups_hide_their_subtree() {
        assert_eq!(
            outline(&sample(), "", &["生产/Web", "开发"]),
            ["生产(4)", " Web(2)+", " 数据库(1)", "  db-01", " bastion", "开发(1)+", "solo"]
        );
        assert_eq!(outline(&sample(), "", &["生产"]), ["生产(4)+", "开发(1)", " dev", "solo"]);
    }

    #[test]
    fn searching_filters_and_expands() {
        assert_eq!(
            outline(&sample(), "web", &["生产", "生产/Web"]),
            ["生产(2)", " Web(2)", "  web-01", "  web-02"]
        );
        assert_eq!(outline(&sample(), "solo", &[]), ["solo"]);
        assert!(outline(&sample(), "nothing", &[]).is_empty());
    }

    #[test]
    fn lists_every_level_of_existing_groups() {
        assert_eq!(group_paths(&sample()), ["生产", "生产/Web", "生产/数据库", "开发"]);
    }
}
