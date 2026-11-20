package dev.lancast.sender

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder

/** A connected-device service keeps the LAN file server owned while the Activity is hidden. */
class FileShareService : Service() {
    private var request: String? = null
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "STOP") { SenderRuntime.stop(); stopSelf(); return START_NOT_STICKY }
        val id = intent?.getStringExtra("requestId") ?: run { stopSelf(startId); return START_NOT_STICKY }
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel("file-share", "视频文件分享", NotificationManager.IMPORTANCE_LOW))
        val stop = PendingIntent.getService(this, 2, Intent(this, FileShareService::class.java).setAction("STOP"), PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, "file-share").setContentTitle("LanCast 正在分享视频文件")
            .setContentText("保持与电视的连接，点停止结束分享").setSmallIcon(android.R.drawable.ic_menu_share)
            .setOngoing(true).addAction(Notification.Action.Builder(null, "停止", stop).build()).build()
        runCatching {
            startForeground(11, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            if (SenderRuntime.openFile(this, id) { stopSelf(startId) }) request = id else stopSelf(startId)
        }.onFailure { SenderRuntime.fileFailure(id, it.message ?: "文件服务无法启动"); stopSelf(startId) }
        return START_NOT_STICKY
    }
    override fun onDestroy() { request?.let { SenderRuntime.fileServiceStopped(it) }; super.onDestroy() }
    override fun onBind(intent: Intent?): IBinder? = null
}
