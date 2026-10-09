import { save } from '@tauri-apps/plugin-dialog';

import { invokeDesktop } from '@/lib/api/invoke/desktop';
import { IS_ANDROID } from '@/lib/platform';

// 診断レポート(#958 系)とアプリ内ログ(#978)が共有する、利用者操作による text 書き出し。
// Blob + <a download> は WebKitGTK / WebView2 の両方で「保存先を利用者が選ぶ」既存経路。
// Android の WebView は download を扱わないので、保存の画面で選んだ場所へ書く（#1197）。
// 自動保存・自動送信には使わない。保存の画面を取り消したときは false を返す。
export async function downloadTextFile(fileName: string, text: string): Promise<boolean> {
  if (IS_ANDROID) {
    const path = await save({ defaultPath: fileName });
    if (path === null) return false;
    await invokeDesktop<void>('write_text_document', { path, text });
    return true;
  }
  const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = fileName;
  link.click();
  URL.revokeObjectURL(url);
  return true;
}
