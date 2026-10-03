package dev.lancast.sender

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder

class CaptureService : Service() {
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "STOP") { SenderRuntime.stop(); stopSelf(); return START_NOT_STICKY }
        val grant = intent?.getParcelableExtra<Intent>("grant") ?: run { stopSelf(); return START_NOT_STICKY }
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel("capture", "屏幕分享", NotificationManager.IMPORTANCE_LOW))
        val stop = PendingIntent.getService(this, 1, Intent(this, CaptureService::class.java).setAction("STOP"), PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, "capture").setContentTitle("LanCast 正在分享屏幕")
            .setContentText("点停止可立即结束屏幕与内部声音采集").setSmallIcon(android.R.drawable.ic_menu_share)
            .setOngoing(true).addAction(Notification.Action.Builder(null, "停止", stop).build()).build()
        startForeground(10, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        runCatching { SenderRuntime.beginCapture(this, grant, intent.getBooleanExtra("audio", false)) { stopSelf() } }
            .onFailure { stopSelf() }
        return START_NOT_STICKY
    }
    override fun onDestroy() { SenderRuntime.stop(); super.onDestroy() }
    override fun onBind(intent: Intent?): IBinder? = null
}
