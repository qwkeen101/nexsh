package dev.nexsh.mobile

import android.graphics.Color
import android.net.ConnectivityManager
import android.net.Network
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * 打开着的会话。放在进程级而不是界面状态里：界面重建（切换深色模式、分屏）时，
 * Rust 一侧的连接还活着，列表不能跟着丢。
 */
object Sessions {
    val open = mutableStateListOf<OpenSession>()
}

class MainActivity : ComponentActivity() {
    /** 换了网络（Wi-Fi 和移动数据之间切换、信号恢复）时，等着重连的会话不必再等退避时间。 */
    private val network = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            Bridge.networkAvailable()
        }
    }

    override fun onStart() {
        super.onStart()
        // 回到前台：后台期间排着的重连现在就做。注册回调时系统还会为当前网络再通知一次。
        Bridge.networkAvailable()
        runCatching {
            getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(network)
        }
    }

    override fun onStop() {
        runCatching {
            getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(network)
        }
        super.onStop()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // 界面只有深色一种，状态栏和导航栏的图标固定用浅色，不跟随系统的浅色模式。
        val bars = SystemBarStyle.dark(Color.TRANSPARENT)
        enableEdgeToEdge(statusBarStyle = bars, navigationBarStyle = bars)
        val startError = Bridge.start(filesDir.absolutePath)
        val secrets = Secrets(applicationContext)
        setContent {
            NexshTheme { App(startError, secrets) }
        }
    }
}

private sealed interface Screen {
    data object Hosts : Screen
    data class Edit(val original: HostItem?) : Screen
    data class Terminal(val session: OpenSession) : Screen
}

@Composable
private fun App(startError: String, secrets: Secrets) {
    var screen by remember { mutableStateOf<Screen>(Screen.Hosts) }
    var hosts by remember { mutableStateOf(loadHosts()) }
    var error by remember { mutableStateOf(startError) }

    // 内容避开状态栏、导航栏和软键盘；它们后面露出的是栏的颜色。
    Box(Modifier.fillMaxSize().background(Palette.Bar).safeDrawingPadding()) {
        when (val current = screen) {
            Screen.Hosts -> HostsScreen(
                hosts = hosts,
                sessions = Sessions.open,
                onConnect = { host ->
                    val useKey = host.auth == "key"
                    // 行列数先给个常见值，终端控件量出实际大小后会立刻改掉。
                    val id = Bridge.open(
                        host.name, 80, 24,
                        secrets.load(Secrets.password(host.name)),
                        if (useKey) secrets.load(Secrets.passphrase(host.name)) else null,
                        if (useKey) secrets.load(Secrets.key(host.name)) else null,
                    )
                    if (id != 0L) {
                        val session = OpenSession(id, host.name)
                        Sessions.open.add(session)
                        screen = Screen.Terminal(session)
                    }
                },
                onResume = { screen = Screen.Terminal(it) },
                onAdd = { screen = Screen.Edit(null) },
                onEdit = { screen = Screen.Edit(it) },
                onDelete = { host ->
                    error = Bridge.deleteHost(host.name)
                    if (error.isEmpty()) secrets.deleteHost(host.name)
                    hosts = loadHosts()
                },
            )

            is Screen.Edit -> {
                BackHandler { screen = Screen.Hosts }
                val original = current.original
                HostEditScreen(
                    original = original,
                    groups = hosts.map { it.group }.distinct(),
                    hasSavedPassword = original != null && secrets.load(Secrets.password(original.name)) != null,
                    savedKey = remember(original) {
                        original?.let { secrets.load(Secrets.key(it.name)) }?.let(::keySummary)
                    },
                    hasSavedPassphrase = original != null && secrets.load(Secrets.passphrase(original.name)) != null,
                    onCancel = { screen = Screen.Hosts },
                    onSaved = { name, saved ->
                        if (original != null && original.name != name) secrets.renameHost(original.name, name)
                        if (saved.useKey) {
                            if (saved.key.isNotEmpty()) {
                                secrets.store(Secrets.key(name), saved.key)
                                // 换了一把私钥，之前那把的口令不再适用。
                                secrets.delete(Secrets.passphrase(name))
                            }
                            if (saved.passphrase.isNotEmpty()) secrets.store(Secrets.passphrase(name), saved.passphrase)
                        } else {
                            // 改回密码登录：私钥不再用得到，不留在手机上。
                            secrets.delete(Secrets.key(name))
                            secrets.delete(Secrets.passphrase(name))
                            if (saved.password.isNotEmpty()) secrets.store(Secrets.password(name), saved.password)
                        }
                        hosts = loadHosts()
                        screen = Screen.Hosts
                    },
                )
            }

            is Screen.Terminal -> {
                BackHandler { screen = Screen.Hosts }
                // 换一个会话就换一个终端控件，不让上一个会话的画面留在里面。
                key(current.session.id) {
                    TerminalScreen(
                        session = current.session,
                        secrets = secrets,
                        onBack = { screen = Screen.Hosts },
                        onClose = {
                            Bridge.close(current.session.id)
                            Sessions.open.remove(current.session)
                            screen = Screen.Hosts
                        },
                    )
                }
            }
        }
        if (error.isNotEmpty()) {
            Text(
                error,
                color = Palette.Error,
                fontSize = 13.sp,
                modifier = Modifier
                    .align(Alignment.BottomCenter)
                    .fillMaxWidth()
                    .background(Palette.Surface)
                    .padding(horizontal = 16.dp, vertical = 10.dp),
            )
        }
    }
}
