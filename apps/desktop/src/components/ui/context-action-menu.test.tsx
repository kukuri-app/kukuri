import { useState } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import {
  ContextActionMenu,
  contextActionMenuPositionFromKeyboard,
  contextActionMenuPositionFromPointer,
  type ContextActionMenuPosition,
} from './context-action-menu';

function MenuHarness({ onSelect = vi.fn() }: { onSelect?: () => void }) {
  const [position, setPosition] = useState<ContextActionMenuPosition | null>(null);
  return (
    <>
      <button
        type='button'
        onContextMenu={(event) => setPosition(contextActionMenuPositionFromPointer(event))}
        onKeyDown={(event) => {
          const next = contextActionMenuPositionFromKeyboard(event);
          if (next) setPosition(next);
        }}
      >
        <span>Open actions</span>
      </button>
      <ContextActionMenu
        open={position !== null}
        position={position}
        items={[
          { id: 'first', label: 'First action', onSelect },
          { id: 'disabled', label: 'Disabled action', disabled: true, onSelect: vi.fn() },
          { id: 'last', label: 'Last action', tone: 'danger', onSelect: vi.fn() },
        ]}
        onClose={() => setPosition(null)}
      />
    </>
  );
}

function NestedActionHarness() {
  const [position, setPosition] = useState<ContextActionMenuPosition | null>(null);
  return (
    <div
      role='group'
      tabIndex={0}
      onContextMenu={(event) => setPosition(contextActionMenuPositionFromPointer(event))}
      onKeyDown={(event) => setPosition(contextActionMenuPositionFromKeyboard(event))}
    >
      <button type='button'>Nested action</button>
      <ContextActionMenu
        open={position !== null}
        position={position}
        items={[{ id: 'copy', label: 'Copy identifier', onSelect: vi.fn() }]}
        onClose={() => setPosition(null)}
      />
    </div>
  );
}

test('pointer context menu focuses the first action and restores focus after selection', async () => {
  const user = userEvent.setup();
  const onSelect = vi.fn();
  render(<MenuHarness onSelect={onSelect} />);

  const trigger = screen.getByRole('button', { name: 'Open actions' });
  fireEvent.contextMenu(screen.getByText('Open actions'), { clientX: 40, clientY: 60 });
  const first = screen.getByRole('menuitem', { name: 'First action' });
  expect(first).toHaveFocus();

  await user.click(first);
  expect(onSelect).toHaveBeenCalledOnce();
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();
});

test('keyboard context menu skips disabled actions, keeps focus across owner re-renders and Escape restores focus', () => {
  const { rerender } = render(<MenuHarness />);
  const trigger = screen.getByRole('button', { name: 'Open actions' });
  trigger.focus();

  fireEvent.keyDown(trigger, { key: 'F10', shiftKey: true });
  const first = screen.getByRole('menuitem', { name: 'First action' });
  const last = screen.getByRole('menuitem', { name: 'Last action' });
  expect(first).toHaveFocus();

  fireEvent.keyDown(first, { key: 'ArrowDown' });
  expect(last).toHaveFocus();
  // 開いている間の再描画で items が作り直されても、移した focus を先頭へ戻さない。
  rerender(<MenuHarness />);
  expect(last).toHaveFocus();
  fireEvent.keyDown(last, { key: 'Home' });
  expect(first).toHaveFocus();
  fireEvent.keyDown(first, { key: 'End' });
  expect(last).toHaveFocus();
  fireEvent.keyDown(last, { key: 'Escape' });

  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();
});

test('Context Menu key opens the same menu and Tab closes it', () => {
  render(<MenuHarness />);
  const trigger = screen.getByRole('button', { name: 'Open actions' });
  trigger.focus();

  fireEvent.keyDown(trigger, { key: 'ContextMenu' });
  const first = screen.getByRole('menuitem', { name: 'First action' });
  expect(first).toHaveFocus();
  fireEvent.keyDown(first, { key: 'Tab' });

  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();
});

test('nested interactive controls do not open the parent context menu', () => {
  render(<NestedActionHarness />);
  const nestedAction = screen.getByRole('button', { name: 'Nested action' });

  fireEvent.contextMenu(nestedAction);
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();

  nestedAction.focus();
  fireEvent.keyDown(nestedAction, { key: 'F10', shiftKey: true });
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
});
