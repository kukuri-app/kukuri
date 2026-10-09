package app.kukuri.android

import android.os.Bundle
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.updatePadding

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // edge-to-edge では OS がキーボードの分だけ画面を縮めないので、キーボードの上までに縮めて入力欄と送信を見せる（#1198 AC-2）。
    ViewCompat.setOnApplyWindowInsetsListener(findViewById<View>(android.R.id.content)) { view, insets ->
      view.updatePadding(bottom = insets.getInsets(WindowInsetsCompat.Type.ime()).bottom)
      insets
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    webView.addJavascriptInterface(BackBridge(), "kukuriAndroid")
  }

  // 最初の画面での戻るで、アプリを終了せず背景へ移す（frontend の `androidBackButton.ts` が呼ぶ。#1198 AC-2）。
  inner class BackBridge {
    @JavascriptInterface
    fun moveTaskToBack() {
      runOnUiThread { this@MainActivity.moveTaskToBack(true) }
    }
  }
}
