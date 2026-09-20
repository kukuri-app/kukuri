/// #1192: community node の法務文書の役割(kind)。
///
/// slug は operator config で自由に決められるため、文書がどの役割かは slug ではなく
/// 公開 policy カタログの `policy_kind` で判別する。値は Rust 側
/// `kukuri_cn_operator::LegalDocumentKind::wire_name()` と同じ。
export const POLICY_KIND_TERMS = 'terms';
export const POLICY_KIND_PRIVACY = 'privacy';
/// 権利侵害申出ポリシー。一括同意の一覧には出さず、権利侵害申請モーダルで提示する
/// （ADR 0033 の申出画面へ進む前の対応範囲確認）。
export const POLICY_KIND_RIGHTS_INFRINGEMENT = 'rights_infringement';

/// 同意ダイアログの表示順。ここに無い kind は後ろへ回し、既存の並び（slug 昇順）を保つ。
const CONSENT_DISPLAY_ORDER = [POLICY_KIND_TERMS, POLICY_KIND_PRIVACY];

/// 一覧内の表示順位。小さいほど前。
export function consentPolicyOrder(policyKind: string | null | undefined): number {
  const index = policyKind ? CONSENT_DISPLAY_ORDER.indexOf(policyKind) : -1;
  return index < 0 ? CONSENT_DISPLAY_ORDER.length : index;
}

/// 一括同意の一覧から外す文書か。
///
/// `required` な文書は kind に関わらず外さない。外すと `allRequiredAccepted` が
/// 到達不能になり、node へ接続できなくなるため。
export function isConsentDialogHiddenPolicy(
  policyKind: string | null | undefined,
  required: boolean
): boolean {
  return !required && policyKind === POLICY_KIND_RIGHTS_INFRINGEMENT;
}
