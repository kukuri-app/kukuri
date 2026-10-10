package app.kukuri.android

import android.os.Bundle
import android.content.Intent
import android.content.res.Configuration
import android.provider.OpenableColumns
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.updatePadding
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.roundToInt

class MainActivity : TauriActivity() {
  private var webView: WebView? = null

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    this.webView = webView
    webView.settings.textZoom = (resources.configuration.fontScale * 100).roundToInt()
    webView.addJavascriptInterface(BackBridge(), "kukuriAndroid")
  }

  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    webView?.settings?.textZoom = (newConfig.fontScale * 100).roundToInt()
  }

  override fun onNewIntent(intent: Intent) {
    setIntent(intent)
    super.onNewIntent(intent)
  }

  // Wry の既存 picker と File callback はそのまま使う。本文を JavaBridge へ渡さず、
  // 選択結果の URI だけを File が届く前に接続する（#1197 AC-2b）。
  @Suppress("DEPRECATION")
  override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
    val files = JSONArray()
    if (resultCode == RESULT_OK) {
      val uris = data?.clipData?.let { clip ->
        (0 until clip.itemCount).map { clip.getItemAt(it).uri }
      } ?: listOfNotNull(data?.data)
      for (uri in uris.filter { it.scheme == "content" }) {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { cursor ->
          if (cursor.moveToFirst()) {
            files.put(JSONObject().put("uri", uri.toString()).put("name", cursor.getString(0))
              .put("size", if (cursor.isNull(1)) JSONObject.NULL else cursor.getLong(1)))
          }
        }
      }
    }
    webView?.evaluateJavascript("window.__KUKURI_SELECTED_FILES__ = $files", null)
    super.onActivityResult(requestCode, resultCode, data)
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // system bars・cutout・IME の外側へ主要内容と操作を収める（#1198 AC-2/3）。
    ViewCompat.setOnApplyWindowInsetsListener(findViewById<View>(android.R.id.content)) { view, insets ->
      val safe = insets.getInsets(WindowInsetsCompat.Type.systemBars() or
        WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime())
      view.updatePadding(left = safe.left, top = safe.top, right = safe.right, bottom = safe.bottom)
      insets
    }
  }

  // 最初の画面での戻るで、アプリを終了せず背景へ移す（frontend の `androidBackButton.ts` が呼ぶ。#1198 AC-2）。
  inner class BackBridge {
    @JavascriptInterface
    fun moveTaskToBack() {
      runOnUiThread { this@MainActivity.moveTaskToBack(true) }
    }
  }
}
