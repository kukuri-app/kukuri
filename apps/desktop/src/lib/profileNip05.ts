export type ProfileNip05 = {
  identifier: string;
  name: string;
  domain: string;
};

const NAME = /^[a-z0-9._-]{1,64}$/;
const DOMAIN_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

/// NIP-05 の識別子 `name@domain` の前後の空白を除いて英大文字を小文字にし、名前とドメインに分ける。
/// 形は crates/core の `normalize_profile_nip05` と同じ(ADR 0064 §1)。合わなければ null。
export function parseProfileNip05(value: string): ProfileNip05 | null {
  const identifier = value.trim().replace(/[A-Z]+/g, (letters) => letters.toLowerCase());
  const at = identifier.indexOf('@');
  if (at < 0) return null;
  const name = identifier.slice(0, at);
  const domain = identifier.slice(at + 1);
  const labels = domain.split('.');
  const valid =
    NAME.test(name) &&
    domain.length <= 253 &&
    labels.length >= 2 &&
    labels.every((label) => DOMAIN_LABEL.test(label)) &&
    /[a-z]/.test(labels[labels.length - 1]);
  return valid ? { identifier, name, domain } : null;
}
