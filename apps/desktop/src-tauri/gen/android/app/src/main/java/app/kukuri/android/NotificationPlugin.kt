package app.kukuri.android

import android.Manifest
import android.app.Activity
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import app.tauri.PermissionState
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@InvokeArg
class InboxNotificationArgs {
  lateinit var id: String
  lateinit var title: String
  var body: String? = null
  var silent: Boolean = false
}

@TauriPlugin(permissions = [Permission(alias = "notifications", strings = [Manifest.permission.POST_NOTIFICATIONS])])
class NotificationPlugin(private val activity: Activity) : Plugin(activity) {
  private val manager = activity.getSystemService(NotificationManager::class.java)
  private val channelId = "kukuri.inbox"

  private fun state(): String {
    if (Build.VERSION.SDK_INT >= 33) {
      when (getPermissionState("notifications")) {
        PermissionState.PROMPT -> return "prompt"
        PermissionState.PROMPT_WITH_RATIONALE, PermissionState.DENIED -> return "denied"
        PermissionState.GRANTED -> {}
        null -> return "unavailable"
      }
    }
    return if (NotificationManagerCompat.from(activity).areNotificationsEnabled() &&
      manager.getNotificationChannel(channelId)?.importance != NotificationManager.IMPORTANCE_NONE) "granted" else "denied"
  }

  @Command
  @PermissionCallback
  fun permission(invoke: Invoke) {
    invoke.resolveObject(state())
  }

  @Command
  fun request(invoke: Invoke) {
    if (Build.VERSION.SDK_INT >= 33 && getPermissionState("notifications") != PermissionState.GRANTED) {
      requestPermissionForAlias("notifications", invoke, "permission")
    } else permission(invoke)
  }

  @Command
  fun show(invoke: Invoke) {
    if (state() != "granted") {
      invoke.reject("os_notification_unavailable")
      return
    }
    val args = invoke.parseArgs(InboxNotificationArgs::class.java)
    manager.createNotificationChannel(NotificationChannel(channelId, "kukuri", NotificationManager.IMPORTANCE_DEFAULT))
    val intent = Intent(Intent.ACTION_VIEW, Uri.parse("kukuri://notification/?id=" + Uri.encode(args.id)), activity, activity.javaClass)
      .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
    val pending = PendingIntent.getActivity(activity, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    val notification = NotificationCompat.Builder(activity, channelId)
      .setSmallIcon(activity.applicationInfo.icon)
      .setContentTitle(args.title)
      .setContentIntent(pending)
      .setAutoCancel(true)
      .setOnlyAlertOnce(true)
      .setSilent(args.silent)
      .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
    args.body?.let { notification.setContentText(it).setStyle(NotificationCompat.BigTextStyle().bigText(it)) }
    manager.notify(args.id, 0, notification.build())
    invoke.resolve()
  }
}
