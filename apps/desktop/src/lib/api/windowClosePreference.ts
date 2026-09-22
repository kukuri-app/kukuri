import { invokeDesktop } from './invoke/desktop';
import { isDesktopMockActive } from './invoke/dispatch';

export type WindowCloseBehavior = 'quit' | 'tray';

export type WindowClosePreference = {
  behavior: WindowCloseBehavior | null;
};

export type WindowClosePrompt = {
  request_id: number;
};

export async function getWindowClosePreference(): Promise<WindowClosePreference> {
  if (isDesktopMockActive()) return { behavior: null };
  return invokeDesktop<WindowClosePreference>('get_window_close_preference');
}

export async function setWindowClosePreference(
  preference: WindowClosePreference
): Promise<WindowClosePreference> {
  if (isDesktopMockActive()) return preference;
  return invokeDesktop<WindowClosePreference>('set_window_close_preference', { preference });
}

export async function getPendingWindowCloseRequest(): Promise<WindowClosePrompt | null> {
  if (isDesktopMockActive()) return null;
  return invokeDesktop<WindowClosePrompt | null>('get_pending_window_close_request');
}

export async function respondWindowCloseRequest(
  requestId: number,
  behavior: WindowCloseBehavior | null,
  remember: boolean
): Promise<void> {
  if (isDesktopMockActive()) return;
  return invokeDesktop<void>('respond_window_close_request', {
    requestId,
    behavior,
    remember,
  });
}
