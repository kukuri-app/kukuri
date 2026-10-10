package app.kukuri.android

import android.app.Activity
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.os.Handler
import android.os.Looper
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@InvokeArg
class WatchNetworkArgs {
  lateinit var handler: Channel
}

@TauriPlugin
class NetworkPlugin(activity: Activity) : Plugin(activity) {
  private val manager = activity.getSystemService(ConnectivityManager::class.java)
  private var handler: Channel? = null
  private var current: Network? = null
  private val callback = object : ConnectivityManager.NetworkCallback() {
    override fun onAvailable(network: Network) {
      current = network
      handler?.sendObject(true)
    }

    override fun onLost(network: Network) {
      if (current == network) {
        current = null
        handler?.sendObject(false)
      }
    }

    override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
      if (current == network) handler?.sendObject(true)
    }
  }

  @Command
  fun watch(invoke: Invoke) {
    handler = invoke.parseArgs(WatchNetworkArgs::class.java).handler
    current = manager.activeNetwork
    manager.registerDefaultNetworkCallback(callback, Handler(Looper.getMainLooper()))
    handler?.sendObject(current != null)
    invoke.resolve()
  }

  override fun onDestroy(activity: AppCompatActivity) {
    manager.unregisterNetworkCallback(callback)
    handler = null
  }
}
