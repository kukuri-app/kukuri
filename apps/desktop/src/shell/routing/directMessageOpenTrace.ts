import { normalizeInvokeError } from '@/lib/api/invoke/error';

let sequence = 0;

/** E2Eの3 APIを呼出単位で追う。通常buildでは元のPromiseをそのまま返す。 */
export function directMessageOpenTrace(peerPubkey: string) {
  const openId = import.meta.env.VITE_KUKURI_WEB_E2E === '1' ? ++sequence : 0;
  return <T>(method: string, request: Promise<T>): Promise<T> => {
    if (!openId) return request;
    const event = { openId, peerPubkey, method };
    console.info('[kukuri.e2e.dm]', { ...event, phase: 'api-start' });
    return request.then(
      (value) => {
        console.info('[kukuri.e2e.dm]', { ...event, phase: 'api-fulfilled' });
        return value;
      },
      (error: unknown) => {
        console.info('[kukuri.e2e.dm]', {
          ...event, phase: 'api-rejected', code: normalizeInvokeError(error).code,
        });
        throw error;
      }
    );
  };
}
