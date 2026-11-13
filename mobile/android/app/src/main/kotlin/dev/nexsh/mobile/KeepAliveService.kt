package dev.nexsh.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * 有会话开着的时候运行的前台服务。它自己什么都不做，只是让系统知道这个应用还有事在忙：
 * 没有它，应用退到后台几分钟后进程会被冻结或回收，SSH 连接跟着断。
 * 代价是通知栏里常驻一条通知，这是系统对前台服务的要求，也让用户随时看得到有连接开着。
 */
class KeepAliveService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_CLOSE_ALL) {
            Sessions.closeAll()
            stopSelf()
            return START_NOT_STICKY
        }
        val notification = notification(intent?.getIntExtra(EXTRA_COUNT, 1) ?: 1)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        // 进程被杀后会话已经不在了，重启服务没有意义。
        return START_NOT_STICKY
    }

    private fun notification(count: Int): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        // 低重要度：不响、不弹出，只在通知栏里待着。
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "打开着的会话", NotificationManager.IMPORTANCE_LOW),
        )
        val open = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val closeAll = PendingIntent.getService(
            this, 1,
            Intent(this, KeepAliveService::class.java).setAction(ACTION_CLOSE_ALL),
            PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_terminal)
            .setContentTitle("nexsh")
            .setContentText(count.toString() + " 个会话保持连接中")
            .setContentIntent(open)
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, "全部断开", closeAll).build())
            .build()
    }

    companion object {
        private const val CHANNEL = "sessions"
        private const val NOTIFICATION_ID = 1
        private const val ACTION_CLOSE_ALL = "dev.nexsh.mobile.CLOSE_ALL"
        private const val EXTRA_COUNT = "count"

        /** 会话数变化时调用：有会话就让服务跑着（并更新通知上的数字），没有了就停掉。 */
        fun update(context: Context, count: Int) {
            val intent = Intent(context, KeepAliveService::class.java)
            if (count > 0) {
                // 界面不在前台时系统不允许启动前台服务；那时服务本来就在运行，失败了也无妨。
                runCatching { context.startForegroundService(intent.putExtra(EXTRA_COUNT, count)) }
            } else {
                context.stopService(intent)
            }
        }
    }
}
