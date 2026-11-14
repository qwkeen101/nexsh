package dev.nexsh.mobile

import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.FileProvider
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private const val RELEASES = "https://github.com/qwkeen101/nexsh/releases"

/**
 * 应用内更新：自己从发布页下载新版安装包，再交给系统安装。
 * 用浏览器下载 APK 时，浏览器对任何安装包都会弹"文件可能有害"；在应用里下载就没有这一步。
 */
object Updater {
    fun installed(context: Context): String =
        runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }
            .getOrNull().orEmpty()

    /** "v0.1.20" 或 "0.1.20" 里的三个数；不是这个样子（比如开发版的 "dev"）时为 null。 */
    private fun numbers(version: String): List<Int>? {
        val parts = version.removePrefix("v").split('.')
        if (parts.size != 3) return null
        return parts.map { it.toIntOrNull() ?: return null }
    }

    fun isNewer(candidate: String, installed: String): Boolean {
        val theirs = numbers(candidate) ?: return false
        val ours = numbers(installed) ?: return false
        for (index in 0..2) {
            if (theirs[index] != ours[index]) return theirs[index] > ours[index]
        }
        return false
    }

    /**
     * 最新版本的标签。发布页的 /latest 会重定向到最新版本的页面，标签就在重定向的地址里，
     * 不必调用有次数限制的接口，也不用解析网页。查不到（没网、被墙）时为 null。
     */
    fun latestTag(): String? {
        val connection = URL(RELEASES + "/latest").openConnection() as HttpURLConnection
        return try {
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 8000
            connection.readTimeout = 8000
            connection.requestMethod = "HEAD"
            connection.getHeaderField("Location")
                ?.substringAfterLast("/tag/", "")
                ?.takeIf { numbers(it) != null }
        } catch (error: IOException) {
            null
        } finally {
            connection.disconnect()
        }
    }

    /** 下载这个版本的安装包到缓存目录，沿途报告进度（0 到 1）。 */
    fun download(context: Context, tag: String, onProgress: (Float) -> Unit): File {
        // 以前下载过的旧包不留着。
        val folder = File(context.cacheDir, "updates")
        folder.deleteRecursively()
        folder.mkdirs()
        val target = File(folder, "nexsh-" + tag + ".apk")
        val address = RELEASES + "/download/" + tag + "/nexsh-" + tag + "-android-arm64.apk"
        val connection = URL(address).openConnection() as HttpURLConnection
        try {
            connection.connectTimeout = 15000
            connection.readTimeout = 30000
            if (connection.responseCode != HttpURLConnection.HTTP_OK) {
                throw IOException("服务器返回 " + connection.responseCode)
            }
            val total = connection.contentLengthLong
            connection.inputStream.use { input ->
                target.outputStream().use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var done = 0L
                    while (true) {
                        val read = input.read(buffer)
                        if (read < 0) break
                        output.write(buffer, 0, read)
                        done += read
                        if (total > 0) onProgress(done.toFloat() / total)
                    }
                }
            }
        } finally {
            connection.disconnect()
        }
        return target
    }

    /** 交给系统的安装程序。新旧版本签名相同，是覆盖安装，数据保留。 */
    fun install(context: Context, apk: File) {
        val uri = FileProvider.getUriForFile(context, context.packageName + ".files", apk)
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
    }
}

/** 启动时查一次有没有新版本，有就在主机列表上提示。visible 为 false 时（正在用终端）不打扰。 */
@Composable
fun UpdatePrompt(visible: Boolean) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val installed = remember { Updater.installed(context) }
    var tag by remember { mutableStateOf<String?>(null) }
    var dismissed by remember { mutableStateOf(false) }
    var progress by remember { mutableStateOf<Float?>(null) }
    var problem by remember { mutableStateOf("") }

    LaunchedEffect(Unit) {
        // 发布的安装包只有 arm64 的。
        if (!Build.SUPPORTED_ABIS.contains("arm64-v8a")) return@LaunchedEffect
        val latest = withContext(Dispatchers.IO) { Updater.latestTag() }
        if (latest != null && Updater.isNewer(latest, installed)) tag = latest
    }

    val found = tag
    if (!visible || found == null || dismissed) return
    val downloading = progress != null
    AlertDialog(
        onDismissRequest = { if (!downloading) dismissed = true },
        containerColor = Palette.Surface,
        title = { Text("有新版本 " + found) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    "当前是 " + installed + "。新版在应用里直接下载安装，已保存的主机、密码和私钥都会保留。",
                    color = Palette.Muted,
                    fontSize = 14.sp,
                )
                progress?.let { done -> LinearProgressIndicator(progress = { done }, modifier = Modifier.fillMaxWidth()) }
                if (problem.isNotEmpty()) Text(problem, color = Palette.Error, fontSize = 13.sp)
            }
        },
        confirmButton = {
            TextButton(
                enabled = !downloading,
                onClick = {
                    progress = 0f
                    problem = ""
                    scope.launch {
                        val result = withContext(Dispatchers.IO) {
                            runCatching { Updater.download(context, found) { progress = it } }
                        }
                        progress = null
                        result.mapCatching { Updater.install(context, it) }
                            .onFailure { problem = "更新失败：" + (it.message ?: "网络错误") }
                    }
                },
            ) { Text(if (downloading) "正在下载" else "更新") }
        },
        dismissButton = {
            TextButton(enabled = !downloading, onClick = { dismissed = true }) {
                Text("以后再说", color = Palette.Muted)
            }
        },
    )
}
