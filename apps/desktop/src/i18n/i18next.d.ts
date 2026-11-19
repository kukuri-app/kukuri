import 'i18next';

declare module 'i18next' {
  // i18next 27 の型の既定（t() に補間の変数を必須にする）を v26 の型に戻す。
  // 訳語の呼出しは key を文字列で受ける薄い wrapper（Translate）を通すため。
  interface CustomTypeOptions {
    strictInterpolationChecks: false;
  }
}
