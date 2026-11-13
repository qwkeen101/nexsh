package dev.nexsh.mobile

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/** 私钥文件只有几 KB。限制读取量，选错了大文件也不会卡住界面。 */
private const val MAX_KEY_BYTES = 64 * 1024

/** 一把私钥的一行描述，如 "ED25519 · SHA256:…"；这段文字不是私钥时返回 null。 */
fun keySummary(text: String, passphrase: String? = null): String? {
    val reply = JSONObject(Bridge.inspectKey(text, passphrase))
    return when {
        !reply.optBoolean("ok") -> null
        !reply.has("fingerprint") -> "有口令保护"
        else -> reply.optString("algorithm").removePrefix("ssh-").uppercase() + " · " + reply.optString("fingerprint")
    }
}

/** 刚读进来、还没起名保存的私钥。suggestedName 来自文件名，从剪贴板来的没有。 */
private data class Draft(val content: String, val encrypted: Boolean, val suggestedName: String)

/**
 * 选择私钥。列出私钥库里保存着的，点一下选中；也可以从剪贴板或文件导入一把新的——
 * 导入的先起名保存进库里，再被选中，之后别的主机可以直接选它。
 */
@Composable
fun KeyPickerDialog(
    secrets: Secrets,
    selected: String?,
    onSelect: (String) -> Unit,
    /** 删掉的正是当前选中的那一把时通知调用方。 */
    onDeleted: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    var keys by remember { mutableStateOf(secrets.keys()) }
    var problem by remember { mutableStateOf("") }
    var draft by remember { mutableStateOf<Draft?>(null) }
    // 点了一次删除、等着确认的那一把。
    var deleting by remember { mutableStateOf<String?>(null) }

    // 读进来就检查，不等到保存：拿错了内容（最常见的是 .pub 公钥）当场就能看到。
    fun received(text: String?, suggestedName: String) {
        if (text.isNullOrBlank()) {
            problem = if (suggestedName.isEmpty()) "剪贴板里没有文字" else "读不了这个文件"
            return
        }
        val reply = JSONObject(Bridge.inspectKey(text, null))
        if (!reply.optBoolean("ok")) {
            problem = reply.optString("message")
            return
        }
        problem = ""
        draft = Draft(text, reply.optBoolean("encrypted"), suggestedName)
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) {
            val text = runCatching {
                context.contentResolver.openInputStream(uri)?.use { stream ->
                    val buffer = ByteArray(MAX_KEY_BYTES)
                    var size = 0
                    while (size < buffer.size) {
                        val read = stream.read(buffer, size, buffer.size - size)
                        if (read < 0) break
                        size += read
                    }
                    String(buffer, 0, size, Charsets.UTF_8)
                }
            }.getOrNull()
            // 文件名当作默认的名称：id_ed25519、prod.pem 这样的名字本来就是用户起的。
            val fileName = uri.lastPathSegment.orEmpty().substringAfterLast('/').substringAfterLast(':')
            received(text, fileName.ifEmpty { "私钥" })
        }
    }

    val pending = draft
    if (pending != null) {
        SaveKeyDialog(
            draft = pending,
            taken = keys.map { it.name },
            onSave = { name, summary, passphrase ->
                if (secrets.saveKey(name, pending.content, summary, passphrase)) {
                    draft = null
                    onSelect(name)
                } else {
                    draft = null
                    problem = "保存失败：系统密钥库不可用"
                }
            },
            onCancel = { draft = null },
        )
        return
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Palette.Surface,
        title = { Text("选择私钥") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                if (keys.isEmpty()) {
                    Text(
                        "还没有保存过私钥。复制私钥文件的全部内容后点「从剪贴板粘贴」，或者直接选择私钥文件。",
                        color = Palette.Muted,
                        fontSize = 14.sp,
                    )
                }
                Column(Modifier.heightIn(max = 280.dp).verticalScroll(rememberScrollState())) {
                    keys.forEach { key ->
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(8.dp))
                                .background(if (key.name == selected) Palette.Selected else Palette.Surface)
                                .clickable { onSelect(key.name) }
                                .padding(start = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
                                Text(key.name, color = Palette.Text, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                val users = secrets.hostsUsing(key.name)
                                Text(
                                    when {
                                        deleting != key.name -> key.summary
                                        users.isEmpty() -> "再点一次删除"
                                        else -> "有 " + users.size + " 台主机在用，再点一次删除"
                                    },
                                    color = if (deleting == key.name) Palette.Error else Palette.Faint,
                                    fontSize = 12.sp,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                            if (key.name == selected && deleting != key.name) {
                                Icon(Icons.Filled.Check, contentDescription = "已选中", tint = Palette.Accent, modifier = Modifier.size(18.dp))
                            }
                            IconButton(onClick = {
                                if (deleting == key.name) {
                                    secrets.deleteKey(key.name)
                                    deleting = null
                                    keys = secrets.keys()
                                    if (key.name == selected) onDeleted(key.name)
                                } else {
                                    deleting = key.name
                                }
                            }) {
                                Icon(
                                    Icons.Filled.Delete,
                                    contentDescription = "删除这把私钥",
                                    tint = if (deleting == key.name) Palette.Error else Palette.Faint,
                                    modifier = Modifier.size(18.dp),
                                )
                            }
                        }
                    }
                }
                if (problem.isNotEmpty()) Text(problem, color = Palette.Error, fontSize = 13.sp)
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    TextButton(onClick = { received(clipboard.getText()?.text, "") }) { Text("从剪贴板粘贴") }
                    TextButton(onClick = { picker.launch(arrayOf("*/*")) }) { Text("选择文件") }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("关闭", color = Palette.Muted) } },
    )
}

/** 给刚导入的私钥起名，有口令保护的可以顺便填上口令。 */
@Composable
private fun SaveKeyDialog(
    draft: Draft,
    taken: List<String>,
    onSave: (name: String, summary: String, passphrase: String) -> Unit,
    onCancel: () -> Unit,
) {
    // 默认名称：文件名，或者第一个没被占用的"私钥 N"。
    val suggested = remember {
        draft.suggestedName.takeIf { it.isNotEmpty() && it !in taken }
            ?: generateSequence(1) { it + 1 }.map { "私钥 " + it }.first { it !in taken }
    }
    var name by remember { mutableStateOf(suggested) }
    var passphrase by remember { mutableStateOf("") }
    var nameError by remember { mutableStateOf("") }
    var passphraseError by remember { mutableStateOf("") }

    AlertDialog(
        onDismissRequest = onCancel,
        containerColor = Palette.Surface,
        title = { Text("保存私钥") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    keySummary(draft.content) ?: "",
                    color = Palette.Muted,
                    fontSize = 13.sp,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it; nameError = "" },
                    label = { Text("名称") },
                    singleLine = true,
                    isError = nameError.isNotEmpty(),
                    supportingText = if (nameError.isEmpty()) null else ({ Text(nameError) }),
                )
                if (draft.encrypted) {
                    OutlinedTextField(
                        value = passphrase,
                        onValueChange = { passphrase = it; passphraseError = "" },
                        label = { Text("口令") },
                        singleLine = true,
                        isError = passphraseError.isNotEmpty(),
                        supportingText = {
                            Text(passphraseError.ifEmpty { "这把私钥有口令保护。可以现在填，也可以留到连接时再输入。" })
                        },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                    )
                }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                val trimmed = name.trim()
                nameError = when {
                    trimmed.isEmpty() -> "请起一个名称"
                    trimmed in taken -> "已经有一把叫这个名字的私钥"
                    else -> ""
                }
                // 口令当场验证：存下一个错的口令，要到连接失败时才会发现。
                val reply = JSONObject(Bridge.inspectKey(draft.content, passphrase.ifEmpty { null }))
                passphraseError = if (reply.optBoolean("ok")) "" else reply.optString("message")
                if (nameError.isEmpty() && passphraseError.isEmpty()) {
                    onSave(trimmed, keySummary(draft.content, passphrase.ifEmpty { null }).orEmpty(), passphrase)
                }
            }) { Text("保存") }
        },
        dismissButton = { TextButton(onClick = onCancel) { Text("取消", color = Palette.Muted) } },
    )
}
