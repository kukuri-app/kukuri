import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import { PwaShellView } from './PwaShellNotice';

const state = { ready: true, available: false, checking: false, applying: false, error: null, checkedAt: null };
test('announces a prepared update, preserves manual choice, and offers an explicit apply action', async () => {
  const onApply = vi.fn();
  render(<PwaShellView state={{ ...state, available: true }} onCheck={() => {}} onApply={onApply} />);
  expect(onApply).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Save drafts and update' }));
  expect(onApply).toHaveBeenCalledTimes(1);
});
test('acknowledges checks and prevents repeated clicks', async () => {
  const onCheck = vi.fn();
  render(<PwaShellView state={state} onCheck={onCheck} onApply={() => {}} />);
  await userEvent.click(screen.getByRole('button', { name: 'Check for updates' }));
  expect(screen.getByRole('button', { name: 'Checking…' })).toBeDisabled();
  expect(onCheck).toHaveBeenCalledTimes(1);
});
test('retains the prepared update and retry action after an apply failure', () => {
  render(<PwaShellView state={{ ...state, available: true, error: 'apply' }} onCheck={() => {}} onApply={() => {}} />);
  expect(screen.getByRole('alert')).toHaveTextContent('Your input and current page have been kept');
  expect(screen.getByRole('button', { name: 'Save drafts and update' })).toBeEnabled();
});
