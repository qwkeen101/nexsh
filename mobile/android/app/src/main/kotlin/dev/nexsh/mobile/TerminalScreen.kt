package dev.nexsh.mobile

import android.os.SystemClock
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.MutableIntState
import androidx.compose.runtime.MutableLongState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView

/** 粘滞修饰键：点一下只对下一个键生效，快速连点两下锁定，再点一下松开。 */
private const val OFF = 0
private const val ONCE = 1
private const val LOCKED = 2
private const val DOUBLE_TAP_MS = 350L

private fun cycle(state: MutableIntState, lastTap: MutableLongState) {
    val now = SystemClock.uptimeMillis()
    state.intValue = when (state.intValue) {
        OFF -> ONCE
        ONCE -> if (now - lastTap.longValue < DOUBLE_TAP_MS) LOCKED else OFF
        else -> OFF
    }
    lastTap.longValue = now
}

@Composable
fun TerminalScreen(session: OpenSession, secrets: Secrets, onBack: () -> Unit, onClose: () -> Unit) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val state = remember { mutableIntStateOf(Frame.CONNECTING) }
    val latency = remember { mutableIntStateOf(-1) }
    var message by remember { mutableStateOf("") }
    val ctrl = remember { mutableIntStateOf(OFF) }
    val alt = remember { mutableIntStateOf(OFF) }
    val ctrlTap = remember { mutableLongStateOf(0L) }
    val altTap = remember { mutableLongStateOf(0L) }
    // 这次会话里输入过的密码，断线重连时接着用。
    var typedPassword by remember { mutableStateOf<String?>(null) }

    val view = remember {
        TerminalView(context).apply {
            modifiers = {
                (if (ctrl.intValue != OFF) Bridge.CTRL else 0) or (if (alt.intValue != OFF) Bridge.ALT else 0)
            }
            onModifiersUsed = {
                if (ctrl.intValue == ONCE) ctrl.intValue = OFF
                if (alt.intValue == ONCE) alt.intValue = OFF
            }
            onFrame = { frame ->
                state.intValue = frame.state
                latency.intValue = frame.latencyMs
                message = frame.message
            }
            sessionId = session.id
        }
    }

    fun reconnect() {
        val password = typedPassword ?: secrets.load(Secrets.password(session.host))
        Bridge.reconnect(session.id, password, null)
    }

    Column(Modifier.fillMaxSize().background(Palette.Background)) {
        Row(
            Modifier.fillMaxWidth().background(Palette.Bar).padding(horizontal = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "返回主机列表", tint = Palette.Muted)
            }
            Column(Modifier.weight(1f)) {
                Text(
                    session.host,
                    color = Palette.Text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    val (label, color) = when (state.intValue) {
                        Frame.CONNECTED -> "已连接" to Palette.Success
                        Frame.CONNECTING -> "正在连接" to Palette.Notice
                        Frame.NEEDS_PASSWORD, Frame.NEEDS_PASSPHRASE -> "等待输入凭据" to Palette.Notice
                        Frame.EXITED -> "已断开" to Palette.Faint
                        else -> "连接失败" to Palette.Error
                    }
                    Box(Modifier.size(7.dp).clip(CircleShape).background(color))
                    Spacer(Modifier.width(6.dp))
                    Text(label, color = Palette.Muted, fontSize = 12.sp)
                    if (state.intValue == Frame.CONNECTED && latency.intValue >= 0) {
                        Text("  " + latency.intValue + " ms", color = Palette.Faint, fontSize = 12.sp)
                    }
                }
            }
            IconButton(onClick = onClose) {
                Icon(Icons.Filled.Close, contentDescription = "关闭会话", tint = Palette.Muted)
            }
        }

        if (state.intValue == Frame.EXITED || state.intValue == Frame.FAILED) {
            Row(
                Modifier.fillMaxWidth().background(Palette.Surface).padding(start = 16.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    message.ifEmpty { "连接已断开" },
                    color = if (state.intValue == Frame.FAILED) Palette.Error else Palette.Muted,
                    fontSize = 13.sp,
                    modifier = Modifier.weight(1f).padding(vertical = 8.dp),
                )
                TextButton(onClick = { reconnect() }) { Text("重新连接") }
            }
        }

        AndroidView(
            factory = { view },
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 6.dp, vertical = 4.dp),
        )

        Row(
            Modifier
                .fillMaxWidth()
                .background(Palette.Bar)
                .horizontalScroll(rememberScrollState())
                .padding(horizontal = 8.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Key("Esc") { view.sendKey("escape") }
            Key("Tab") { view.sendKey("tab") }
            Key("Ctrl", ctrl.intValue) { cycle(ctrl, ctrlTap) }
            Key("Alt", alt.intValue) { cycle(alt, altTap) }
            Key("↑") { view.sendKey("up") }
            Key("↓") { view.sendKey("down") }
            Key("←") { view.sendKey("left") }
            Key("→") { view.sendKey("right") }
            Key("Home") { view.sendKey("home") }
            Key("End") { view.sendKey("end") }
            Key("PgUp") { view.sendKey("pageup") }
            Key("PgDn") { view.sendKey("pagedown") }
            for (symbol in listOf("-", "/", "|", "~", ":")) {
                Key(symbol) { view.sendText(symbol) }
            }
            Key("粘贴") { clipboard.getText()?.text?.let(view::paste) }
        }
    }

    if (state.intValue == Frame.NEEDS_PASSWORD || state.intValue == Frame.NEEDS_PASSPHRASE) {
        val forKey = state.intValue == Frame.NEEDS_PASSPHRASE
        var secret by remember { mutableStateOf("") }
        var keep by remember { mutableStateOf(true) }
        AlertDialog(
            onDismissRequest = {},
            containerColor = Palette.Surface,
            title = { Text(if (forKey) "输入私钥口令" else "输入密码") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (message.isNotEmpty()) Text(message, color = Palette.Notice, fontSize = 13.sp)
                    OutlinedTextField(
                        value = secret,
                        onValueChange = { secret = it },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                    )
                    if (!forKey) {
                        Row(
                            Modifier.clickable { keep = !keep },
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Checkbox(checked = keep, onCheckedChange = { keep = it })
                            Text("记住密码", color = Palette.Muted, fontSize = 14.sp)
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    // 先改状态再重连：核心回报新状态之前，对话框不该再闪一下。
                    state.intValue = Frame.CONNECTING
                    if (forKey) {
                        Bridge.reconnect(session.id, typedPassword, secret)
                    } else {
                        typedPassword = secret
                        if (keep) secrets.store(Secrets.password(session.host), secret)
                        Bridge.reconnect(session.id, secret, null)
                    }
                }) { Text("连接") }
            },
            dismissButton = { TextButton(onClick = onClose) { Text("取消") } },
        )
    }
}

/** 按键栏上的一个键。active 是粘滞修饰键的状态。 */
@Composable
private fun Key(label: String, active: Int = OFF, onClick: () -> Unit) {
    val background = when (active) {
        LOCKED -> Palette.Accent
        ONCE -> Palette.Selected
        else -> Palette.Surface
    }
    Box(
        Modifier
            .height(38.dp)
            .widthIn(min = 44.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(background)
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            color = if (active == LOCKED) Color(0xFF0B1220) else Palette.Text,
            fontSize = 14.sp,
            fontWeight = if (active != OFF) FontWeight.SemiBold else FontWeight.Normal,
        )
    }
}
