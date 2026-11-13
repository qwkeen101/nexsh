package dev.nexsh.mobile

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/**
 * 添加或编辑主机。字段的校验规则在 Rust 核心里（和桌面端是同一份），
 * 这里只把它返回的错误显示到对应的输入框下面。
 */
@Composable
fun HostEditScreen(
    original: HostItem?,
    groups: List<String>,
    hasSavedPassword: Boolean,
    onCancel: () -> Unit,
    onSaved: (name: String, password: String) -> Unit,
) {
    var address by remember { mutableStateOf(original?.hostname ?: "") }
    var port by remember { mutableStateOf(original?.port?.toString() ?: "") }
    var user by remember { mutableStateOf(original?.user ?: "") }
    var name by remember { mutableStateOf(original?.name ?: "") }
    var group by remember { mutableStateOf(original?.group ?: "") }
    var note by remember { mutableStateOf(original?.note ?: "") }
    var password by remember { mutableStateOf("") }
    var errors by remember { mutableStateOf(mapOf<String, String>()) }

    fun save() {
        val form = JSONObject()
            .put("address", address)
            .put("port", port)
            .put("user", user)
            .put("name", name)
            .put("group", group)
            .put("note", note)
            .put("auth", "auto")
        val reply = JSONObject(Bridge.saveHost(form.toString(), original?.name))
        if (reply.optBoolean("ok")) {
            onSaved(reply.getString("name"), password)
            return
        }
        val list = reply.optJSONArray("errors")
        val found = mutableMapOf<String, String>()
        for (index in 0 until (list?.length() ?: 0)) {
            val item = list!!.getJSONObject(index)
            // 同一个字段只留第一条。
            found.putIfAbsent(item.optString("field"), item.optString("message"))
        }
        errors = found
    }

    Column(Modifier.fillMaxSize().background(Palette.Background)) {
        Row(
            Modifier.fillMaxWidth().background(Palette.Bar).padding(horizontal = 4.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onCancel) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "返回", tint = Palette.Muted)
            }
            Text(
                if (original == null) "添加主机" else "编辑主机",
                color = Palette.Text,
                fontSize = 18.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = { save() }) { Text("保存", fontSize = 16.sp) }
        }
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Field(
                    "地址", address, { address = it; errors = errors - "address" },
                    placeholder = "主机名或 IP，也可以写 user@host:端口",
                    error = errors["address"],
                    keyboard = KeyboardType.Uri,
                    modifier = Modifier.weight(1f),
                )
                Field(
                    "端口", port, { port = it; errors = errors - "port" },
                    placeholder = "22",
                    error = errors["port"],
                    keyboard = KeyboardType.Number,
                    modifier = Modifier.width(88.dp),
                )
            }
            Field(
                "用户名", user, { user = it; errors = errors - "user" },
                placeholder = "例如 root",
                error = errors["user"],
                keyboard = KeyboardType.Ascii,
            )
            Field(
                "密码", password, { password = it },
                placeholder = if (hasSavedPassword) "已保存，留空则保持不变" else "留空则每次连接时询问",
                hint = "密码用系统密钥库加密后保存在这台手机上。",
                keyboard = KeyboardType.Password,
                secret = true,
            )
            Field(
                "名称", name, { name = it; errors = errors - "name" },
                placeholder = "留空则用地址",
                error = errors["name"],
            )
            Field("分组", group, { group = it }, placeholder = "例如 生产")
            val others = groups.filter { it.isNotEmpty() && it != group.trim() }
            if (others.isNotEmpty()) {
                Row(
                    Modifier.horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    others.forEach { existing ->
                        Text(
                            existing,
                            color = Palette.Muted,
                            fontSize = 13.sp,
                            modifier = Modifier
                                .clip(RoundedCornerShape(50))
                                .border(1.dp, Palette.Border, RoundedCornerShape(50))
                                .clickable { group = existing }
                                .padding(horizontal = 12.dp, vertical = 5.dp),
                        )
                    }
                }
            }
            Field("备注", note, { note = it }, placeholder = "可留空")
            errors[""]?.let { Text(it, color = Palette.Error, fontSize = 13.sp) }
            Spacer(Modifier.padding(bottom = 24.dp))
        }
    }
}

/** 标签 + 输入框 + 这个字段的错误或说明。 */
@Composable
private fun Field(
    label: String,
    value: String,
    onChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "",
    error: String? = null,
    hint: String? = null,
    keyboard: KeyboardType = KeyboardType.Text,
    secret: Boolean = false,
) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(label, color = Palette.Muted, fontSize = 13.sp)
        Box(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(10.dp))
                .background(Palette.Bar)
                .border(1.dp, if (error != null) Palette.Error else Palette.Border, RoundedCornerShape(10.dp))
                .padding(horizontal = 12.dp, vertical = 12.dp),
        ) {
            if (value.isEmpty()) {
                Text(placeholder, color = Palette.Faint, fontSize = 15.sp, maxLines = 1)
            }
            BasicTextField(
                value = value,
                onValueChange = onChange,
                singleLine = true,
                textStyle = TextStyle(color = Palette.Text, fontSize = 15.sp),
                cursorBrush = SolidColor(Palette.Accent),
                keyboardOptions = KeyboardOptions(keyboardType = keyboard, autoCorrectEnabled = false),
                visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
                modifier = Modifier.fillMaxWidth(),
            )
        }
        val below = error ?: hint
        if (below != null) {
            Text(below, color = if (error != null) Palette.Error else Palette.Faint, fontSize = 12.sp)
        }
    }
}
