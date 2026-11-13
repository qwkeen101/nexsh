package dev.nexsh.mobile

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray

/** 列表里的一台主机。字段和 Rust 一侧 hosts 模块输出的 JSON 一一对应。 */
data class HostItem(
    val name: String,
    val hostname: String,
    val port: Int,
    val user: String,
    val group: String,
    val note: String,
    val address: String,
    /** "key" 表示只用私钥登录；其余按密码处理。 */
    val auth: String,
)

fun loadHosts(): List<HostItem> {
    val array = JSONArray(Bridge.hosts())
    return List(array.length()) { index ->
        val item = array.getJSONObject(index)
        HostItem(
            name = item.getString("name"),
            hostname = item.getString("hostname"),
            port = item.getInt("port"),
            user = item.getString("user"),
            group = item.optString("group"),
            note = item.optString("note"),
            address = item.getString("address"),
            auth = item.optString("auth"),
        )
    }
}

/** 一个打开着的会话。 */
data class OpenSession(val id: Long, val host: String)

/** 所有搜索词都要出现在名称、地址或分组里，不分大小写。 */
private fun HostItem.matches(query: String): Boolean {
    val haystack = (name + " " + address + " " + group).lowercase()
    return query.lowercase().split(' ').filter { it.isNotBlank() }.all { haystack.contains(it) }
}

@Composable
fun HostsScreen(
    hosts: List<HostItem>,
    sessions: List<OpenSession>,
    onConnect: (HostItem) -> Unit,
    onResume: (OpenSession) -> Unit,
    onAdd: () -> Unit,
    onEdit: (HostItem) -> Unit,
    onDelete: (HostItem) -> Unit,
) {
    var query by remember { mutableStateOf("") }
    val visible = hosts.filter { it.matches(query) }
    // 分组按第一次出现的顺序排，没有分组的主机放在最后。
    val grouped = visible.groupBy { it.group }.entries.sortedBy { it.key.isEmpty() }

    Box(Modifier.fillMaxSize().background(Palette.Background)) {
        Column(Modifier.fillMaxSize()) {
            Row(
                Modifier.fillMaxWidth().background(Palette.Bar).padding(horizontal = 16.dp, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("主机", color = Palette.Text, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
            }
            SearchBox(query, onChange = { query = it })
            LazyColumn(Modifier.fillMaxSize()) {
                if (sessions.isNotEmpty() && query.isBlank()) {
                    item { SectionHeader("打开的会话", sessions.size) }
                    items(sessions, key = { "session-" + it.id }) { session ->
                        SessionRow(session, onClick = { onResume(session) })
                    }
                }
                grouped.forEach { (group, members) ->
                    if (group.isNotEmpty()) {
                        item(key = "group-$group") { SectionHeader(group, members.size) }
                    } else if (grouped.size > 1 || sessions.isNotEmpty()) {
                        item(key = "ungrouped") { SectionHeader("未分组", members.size) }
                    }
                    items(members, key = { "host-" + it.name }) { host ->
                        HostRow(host, onConnect, onEdit, onDelete)
                    }
                }
                if (visible.isEmpty() && query.isNotBlank()) {
                    item {
                        Text(
                            "没有匹配的主机",
                            color = Palette.Faint,
                            fontSize = 13.sp,
                            modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp),
                        )
                    }
                }
                item { Spacer(Modifier.height(88.dp)) }
            }
        }
        FloatingActionButton(
            onClick = onAdd,
            containerColor = Palette.Accent,
            contentColor = androidx.compose.ui.graphics.Color(0xFF0B1220),
            modifier = Modifier.align(Alignment.BottomEnd).padding(20.dp),
        ) {
            Icon(Icons.Filled.Add, contentDescription = "添加主机")
        }
    }
}

@Composable
private fun SearchBox(query: String, onChange: (String) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(Palette.Bar)
            .padding(start = 16.dp, end = 16.dp, bottom = 12.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(Palette.Background)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Filled.Search, contentDescription = null, tint = Palette.Faint, modifier = Modifier.size(18.dp))
        Spacer(Modifier.width(8.dp))
        Box(Modifier.weight(1f)) {
            if (query.isEmpty()) Text("搜索名称、地址或分组", color = Palette.Faint, fontSize = 15.sp)
            BasicTextField(
                value = query,
                onValueChange = onChange,
                singleLine = true,
                textStyle = TextStyle(color = Palette.Text, fontSize = 15.sp),
                cursorBrush = SolidColor(Palette.Accent),
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

@Composable
private fun SectionHeader(title: String, count: Int) {
    Row(
        Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 16.dp, bottom = 6.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(title, color = Palette.Muted, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        Text(count.toString(), color = Palette.Faint, fontSize = 12.sp)
    }
}

@Composable
private fun Avatar(name: String) {
    // 取高位，名字只差一两个字符的主机也能分到不同的颜色。
    val hash = name.fold(0x811c9dc5.toInt()) { hash, char -> (hash xor char.code) * 0x01000193 }
    val color = Palette.Avatars[((hash ushr 13) % Palette.Avatars.size)]
    val letter = name.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?"
    Box(
        Modifier.size(36.dp).clip(RoundedCornerShape(10.dp)).background(color.copy(alpha = 0.18f)),
        contentAlignment = Alignment.Center,
    ) {
        Text(letter, color = color, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun HostRow(
    host: HostItem,
    onConnect: (HostItem) -> Unit,
    onEdit: (HostItem) -> Unit,
    onDelete: (HostItem) -> Unit,
) {
    var menuOpen by remember { mutableStateOf(false) }
    // 删除不可撤销：第一次点只是把这一项变成确认。
    var confirming by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .fillMaxWidth()
                .combinedClickable(
                    onClick = { onConnect(host) },
                    onLongClick = {
                        confirming = false
                        menuOpen = true
                    },
                )
                .padding(horizontal = 16.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Avatar(host.name)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(host.name, color = Palette.Text, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (host.address != host.name) {
                    Text(host.address, color = Palette.Faint, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
        DropdownMenu(
            expanded = menuOpen,
            onDismissRequest = { menuOpen = false },
            containerColor = Palette.Surface,
        ) {
            DropdownMenuItem(text = { Text("连接") }, onClick = {
                menuOpen = false
                onConnect(host)
            })
            DropdownMenuItem(text = { Text("编辑") }, onClick = {
                menuOpen = false
                onEdit(host)
            })
            DropdownMenuItem(
                text = { Text(if (confirming) "再点一次确认删除" else "删除", color = Palette.Error) },
                onClick = {
                    if (confirming) {
                        menuOpen = false
                        onDelete(host)
                    } else {
                        confirming = true
                    }
                },
            )
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun SessionRow(session: OpenSession, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .combinedClickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(36.dp), contentAlignment = Alignment.Center) {
            Box(Modifier.size(9.dp).clip(CircleShape).background(Palette.Success))
        }
        Spacer(Modifier.width(12.dp))
        Text(session.host, color = Palette.Text, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}
