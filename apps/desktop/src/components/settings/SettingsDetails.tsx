import { useState, type ReactNode } from 'react';

// 開いている間だけ中身を描く詳細。peer の一覧のページは、開いたときにだけ読む(#1221 R2-D)。
export function SettingsDetails({
  summary,
  hidden,
  children,
}: {
  summary: ReactNode;
  hidden?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <details
      className='min-w-0'
      hidden={hidden}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className='cursor-pointer py-2'>{summary}</summary>
      {open ? children : null}
    </details>
  );
}
