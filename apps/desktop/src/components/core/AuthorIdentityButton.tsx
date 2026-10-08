import { cn } from '@/lib/utils';

import { AuthorAvatar } from './AuthorAvatar';
import { VerifiedAuthorName } from './VerifiedAuthorName';

type AuthorIdentityButtonProps = {
  label: string;
  picture?: string | null;
  /// 渡したときだけ、確認できたドメインを名前の後ろに出す(ADR 0064 §5)。
  pubkey?: string;
  nip05?: string | null;
  onClick: () => void;
  avatarSize?: 'sm' | 'lg';
  avatarTestId?: string;
  className?: string;
  buttonClassName?: string;
};

export function AuthorIdentityButton({
  label,
  picture = null,
  pubkey,
  nip05,
  onClick,
  avatarSize = 'sm',
  avatarTestId,
  className,
  buttonClassName,
}: AuthorIdentityButtonProps) {
  return (
    <button
      className={cn('post-meta-author author-link', className, buttonClassName)}
      type='button'
      onClick={onClick}
    >
      <AuthorAvatar
        label={label}
        picture={picture}
        size={avatarSize}
        testId={avatarTestId}
      />
      {pubkey && nip05 ? (
        <VerifiedAuthorName label={label} pubkey={pubkey} nip05={nip05} />
      ) : (
        <span>{label}</span>
      )}
    </button>
  );
}
