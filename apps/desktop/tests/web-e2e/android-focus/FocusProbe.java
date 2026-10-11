package app.kukuri.focusprobe;

import android.app.Instrumentation;
import android.app.UiAutomation;
import android.graphics.Rect;
import android.os.Bundle;
import android.view.accessibility.AccessibilityNodeInfo;
import org.json.JSONObject;

// OSのTalkBackを抑制せず、その実アクセシビリティ焦点だけを読む。
public final class FocusProbe extends Instrumentation {
  @Override public void onCreate(Bundle arguments) {
    super.onCreate(arguments);
    start();
  }
  @Override public void onStart() {
    Bundle result = new Bundle();
    try {
      UiAutomation automation = getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
      AccessibilityNodeInfo root = automation.getRootInActiveWindow();
      AccessibilityNodeInfo node = root == null ? null : root.findFocus(AccessibilityNodeInfo.FOCUS_ACCESSIBILITY);
      if (node == null) throw new IllegalStateException("No accessibility focus in the active window");
      Rect rect = new Rect();
      node.getBoundsInScreen(rect);
      JSONObject focus = new JSONObject();
      focus.put("package", node.getPackageName());
      focus.put("class", node.getClassName());
      focus.put("text", node.getText());
      focus.put("description", node.getContentDescription());
      focus.put("left", rect.left);
      focus.put("top", rect.top);
      focus.put("right", rect.right);
      focus.put("bottom", rect.bottom);
      result.putString("focus", focus.toString());
      finish(-1, result);
    } catch (Exception error) {
      result.putString("error", error.toString());
      finish(0, result);
    }
  }
}
