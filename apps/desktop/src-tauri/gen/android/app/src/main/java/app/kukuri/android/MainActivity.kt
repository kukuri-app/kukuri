package app.kukuri.android

import android.os.Bundle
import android.content.Intent
import android.provider.OpenableColumns
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : TauriActivity() {
  private var webView: WebView? = null

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    this.webView = webView
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
  }
}
