import { normalizeInvokeError } from '@/lib/api/invoke/error';

let sequence = 0;

/** E2Eの3 APIを呼出単位で追う。元のPromiseを返し、待機の段を増やさない。 */
export function directMessageOpenTrace(peerPubkey: string) {
  const openId = import.meta.env.VITE_KUKURI_WEB_E2E === '1' ? ++sequence : 0;
  return <T>(method: string, request: Promise<T>): Promise<T> => {
    if (!openId) return request;
    const event = { openId, peerPubkey, method };
    console.info('[kukuri.e2e.dm]', { ...event, phase: 'api-start' });
    void request.then(
      () => console.info('[kukuri.e2e.dm]', { ...event, phase: 'api-fulfilled' }),
      (error: unknown) => console.info('[kukuri.e2e.dm]', {
        ...event, phase: 'api-rejected', code: normalizeInvokeError(error).code,
      })
    );
    return request;
  };
}
