package app.tauri.nativeaudio

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/// Keeps a sync (or the loudness sweep) alive with the screen off (Fase 1.3).
///
/// Without it, a sync that runs with no music playing lives only as long as
/// the system leaves the process alone: with the screen off, Doze cuts the
/// network and the process is fair game. A foreground service of type
/// `dataSync` is the sanctioned way to say "this is doing work the user asked
/// for"; it has to show a notification, which doubles as the progress the user
/// wanted to see. While it runs it also holds a partial wakelock (the CPU keeps
/// going) and a Wi-Fi lock (the radio doesn't drop to power save mid-transfer).
///
/// Driven from Rust through `NativeAudioPlugin.showSyncNotice` /
/// `hideSyncNotice`: the first call starts the service, later ones only
/// repost the notification.
class SyncService : Service() {
    companion object {
        private const val TAG = "TrakSync"
        private const val CHANNEL_ID = "trak_sync"
        private const val NOTIFICATION_ID = 0x5359
        private const val EXTRA_TITLE = "title"
        private const val EXTRA_TEXT = "text"
        private const val EXTRA_DONE = "done"
        private const val EXTRA_TOTAL = "total"
        private const val EXTRA_ICON = "icon"

        /// Whether the service is up. Updates skip the service round trip.
        @Volatile
        var running = false
            private set

        fun show(context: Context, title: String, text: String, done: Int, total: Int, icon: String) {
            val ctx = context.applicationContext
            if (running) {
                val manager = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
                manager.notify(NOTIFICATION_ID, build(ctx, title, text, done, total, icon))
                return
            }
            val intent = Intent(ctx, SyncService::class.java)
                .putExtra(EXTRA_TITLE, title)
                .putExtra(EXTRA_TEXT, text)
                .putExtra(EXTRA_DONE, done)
                .putExtra(EXTRA_TOTAL, total)
                .putExtra(EXTRA_ICON, icon)
            try {
                ContextCompat.startForegroundService(ctx, intent)
            } catch (e: Exception) {
                // Android 12+ refuses to start one from the background unless
                // the app is exempt (battery optimization off, or it's already
                // playing). The sync still runs; it just isn't protected.
                Log.w(TAG, "could not start the sync service: ${e.message}")
            }
        }

        fun hide(context: Context) {
            val ctx = context.applicationContext
            if (running) ctx.stopService(Intent(ctx, SyncService::class.java))
        }

        private fun ensureChannel(ctx: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val manager = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (manager.getNotificationChannel(CHANNEL_ID) != null) return
            val channel = NotificationChannel(CHANNEL_ID, "Sync", NotificationManager.IMPORTANCE_LOW)
            channel.description = "Music moving between your devices"
            channel.setShowBadge(false)
            manager.createNotificationChannel(channel)
        }

        private fun build(ctx: Context, title: String, text: String, done: Int, total: Int, icon: String): Notification {
            ensureChannel(ctx)
            val open = ctx.packageManager.getLaunchIntentForPackage(ctx.packageName)?.let {
                PendingIntent.getActivity(ctx, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            }
            return NotificationCompat.Builder(ctx, CHANNEL_ID)
                .setSmallIcon(
                    when (icon) {
                        "upload" -> android.R.drawable.stat_sys_upload
                        "analyze" -> android.R.drawable.stat_notify_sync
                        else -> android.R.drawable.stat_sys_download
                    }
                )
                .setContentTitle(title)
                .setContentText(text)
                .setProgress(total.coerceAtLeast(0), done.coerceIn(0, total.coerceAtLeast(0)), total <= 0)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setSilent(true)
                .setCategory(NotificationCompat.CATEGORY_PROGRESS)
                .setContentIntent(open)
                .build()
        }
    }

    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification = build(
            this,
            intent?.getStringExtra(EXTRA_TITLE) ?: "Syncing",
            intent?.getStringExtra(EXTRA_TEXT) ?: "",
            intent?.getIntExtra(EXTRA_DONE, 0) ?: 0,
            intent?.getIntExtra(EXTRA_TOTAL, 0) ?: 0,
            intent?.getStringExtra(EXTRA_ICON) ?: "download",
        )
        try {
            val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
            } else {
                0
            }
            ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, type)
        } catch (e: Exception) {
            Log.w(TAG, "startForeground refused: ${e.message}")
            stopSelf()
            return START_NOT_STICKY
        }
        running = true
        acquireLocks()
        // A sync is started by the app; if the process dies, the next one
        // starts it again. Restarting an empty service would only show a
        // notification for nothing.
        return START_NOT_STICKY
    }

    private fun acquireLocks() {
        if (wakeLock == null) {
            val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
            wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "trak:sync").apply {
                setReferenceCounted(false)
                // Upper bound in case nobody calls hide (a crash in Rust): a
                // sync that takes longer renews it on the next start.
                acquire(60 * 60 * 1000L)
            }
        }
        if (wifiLock == null) {
            val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            @Suppress("DEPRECATION")
            wifiLock = wm?.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "trak:sync")?.apply {
                setReferenceCounted(false)
                acquire()
            }
        }
    }

    private fun releaseLocks() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        wifiLock?.let { if (it.isHeld) it.release() }
        wifiLock = null
    }

    // Android 15 caps `dataSync` services at 6 hours a day and calls
    // `onTimeout(Int, Int)`; not overridden because the plugin still compiles
    // against SDK 34. No sync gets near that; if one does, hide() stops it.

    override fun onDestroy() {
        running = false
        releaseLocks()
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }
}
