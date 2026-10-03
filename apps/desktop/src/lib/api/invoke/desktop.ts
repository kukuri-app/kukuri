import { invoke } from '@tauri-apps/api/core';

import { IS_WEB_RUNTIME, invokeWebRuntime } from '../../webRuntime';
import { normalizeInvokeError } from './error';

export async function invokeDesktop<T>(
  command: string,
  args?: Record<string, unknown>
): Promise<T> {
  try {
    return await (IS_WEB_RUNTIME ? invokeWebRuntime<T>(command, args) : invoke<T>(command, args));
  } catch (error) {
    throw normalizeInvokeError(error);
  }
}
