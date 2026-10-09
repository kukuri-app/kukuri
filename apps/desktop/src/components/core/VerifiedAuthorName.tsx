import { useEffect, useRef, useState } from 'react';

import { verifyProfileNip05 } from '@/lib/profileNip05';

import { useOnScreen } from './useOnScreen';

/// 名前の後ろに、著者のドメインが同じ公開鍵を示したときだけ `@domain` を出す(ADR 0064 §5)。照会は画面内にある間だけ行う。
/// 識別子を持つ著者にだけ使う(持たない著者は名前だけを描き、観測しない)。
export function VerifiedAuthorName({
  label,
  pubkey,
  nip05,
}: {
  label: string;
  pubkey: string;
  nip05: string;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const onScreen = useOnScreen(ref);
  const [verified, setVerified] = useState<{ key: string; domain: string | null } | null>(null);

  useEffect(() => {
    if (!onScreen) return;
    let current = true;
    void verifyProfileNip05(pubkey, nip05).then((domain) => {
      if (current) setVerified({ key: `${pubkey}\n${nip05}`, domain });
    });
    return () => {
      current = false;
    };
  }, [nip05, onScreen, pubkey]);

  const domain = verified?.key === `${pubkey}\n${nip05}` ? verified.domain : null;
  return (
    <span ref={ref}>
      {label}
      {domain ? <span className='font-normal text-[var(--muted-foreground)]'> @{domain}</span> : null}
    </span>
  );
}
