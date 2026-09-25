import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, test, vi } from 'vitest';

import type { DesktopApi } from '@/lib/api';
import { InvokeError } from '@/lib/api/invoke/error';

import { CommunityIndexingRequestDialog } from './CommunityIndexingRequestDialog';

const EMPTY_STATUS = { requests: [], target: null };

// #975: dialog は開いた時点で索引状況を読む。既存 test は「申請なし・未確認」を既定にして申請の挙動を固定する。
function dialogApi(
  submitCommunityNodeIndexingRequest: ReturnType<typeof vi.fn>,
  readCommunityNodeIndexingStatus: ReturnType<typeof vi.fn> = vi.fn().mockResolvedValue(EMPTY_STATUS)
): DesktopApi {
  return { submitCommunityNodeIndexingRequest, readCommunityNodeIndexingStatus } as unknown as DesktopApi;
}

test('public topic request omits private channel confirmation and capability', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn().mockResolvedValue({
    request_id: 'request-1',
    status: 'pending',
  });
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(submitCommunityNodeIndexingRequest)}
      target={{ kind: 'public_topic', topicId: 'kukuri:topic:demo' }}
      eligibleNodeBaseUrls={['https://index.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );
  expect(screen.getByText(/A request does not guarantee indexing/)).toBeInTheDocument();

  fireEvent.click(screen.getByRole('button', { name: 'Submit request' }));

  await waitFor(() => expect(submitCommunityNodeIndexingRequest).toHaveBeenCalledTimes(1));
  expect(submitCommunityNodeIndexingRequest).toHaveBeenCalledWith({
    base_url: 'https://index.example',
    scope_kind: 'public_topic',
    topic_id: 'kukuri:topic:demo',
    channel_id: null,
    confirm_private_channel_secret_disclosure: false,
  });
  expect(await screen.findByText('The request is pending review.')).toBeInTheDocument();
});

test('private channel request stays disabled until explicit disclosure confirmation', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn().mockResolvedValue({
    request_id: 'request-2',
    status: 'approved',
  });
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(submitCommunityNodeIndexingRequest)}
      target={{
        kind: 'private_channel',
        topicId: 'kukuri:topic:demo',
        channelId: 'channel-1',
        channelLabel: 'Core',
      }}
      eligibleNodeBaseUrls={['https://index.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );

  const submit = screen.getByRole('button', { name: 'Submit request' });
  expect(submit).toBeDisabled();
  fireEvent.click(
    screen.getByRole('checkbox', {
      name: /I agree to disclose this channel's read capability/,
    })
  );
  expect(submit).toBeEnabled();
  fireEvent.click(submit);

  await waitFor(() => expect(submitCommunityNodeIndexingRequest).toHaveBeenCalledTimes(1));
  expect(submitCommunityNodeIndexingRequest).toHaveBeenCalledWith(
    expect.objectContaining({
      scope_kind: 'private_channel',
      channel_id: 'channel-1',
      confirm_private_channel_secret_disclosure: true,
    })
  );
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  expect(submit).toBeDisabled();
});

test('private indexing grant can be withdrawn without disclosing the secret again', async () => {
  const submit = vi.fn().mockResolvedValue({ request_id: 'request-2', status: 'approved' });
  const revoke = vi.fn().mockResolvedValue(undefined);
  const api = {
    ...dialogApi(submit),
    revokeCommunityNodeIndexingRequest: revoke,
  };
  render(
    <CommunityIndexingRequestDialog
      api={api}
      target={{ kind: 'private_channel', topicId: 'kukuri:topic:demo',
        channelId: 'channel-1', channelLabel: 'Core' }}
      eligibleNodeBaseUrls={['https://index.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );
  fireEvent.click(screen.getByRole('checkbox'));
  fireEvent.click(screen.getByRole('button', { name: 'Submit request' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Withdraw my indexing request' }));
  await waitFor(() => expect(revoke).toHaveBeenCalledWith(expect.objectContaining({
    base_url: 'https://index.example', scope_kind: 'private_channel',
    channel_id: 'channel-1', confirm_private_channel_secret_disclosure: false,
  })));
  expect(screen.queryByRole('button', { name: 'Withdraw my indexing request' })).not.toBeInTheDocument();
});

test('private confirmation and status reset when the selected node changes', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn().mockResolvedValue({
    request_id: 'request-3',
    status: 'pending',
  });
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(submitCommunityNodeIndexingRequest)}
      target={{
        kind: 'private_channel',
        topicId: 'kukuri:topic:demo',
        channelId: 'channel-1',
        channelLabel: 'Core',
      }}
      eligibleNodeBaseUrls={['https://index-a.example', 'https://index-b.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );

  const confirmation = screen.getByRole('checkbox');
  const submit = screen.getByRole('button', { name: 'Submit request' });
  fireEvent.click(confirmation);
  fireEvent.click(submit);

  expect(await screen.findByText('The request is pending review.')).toBeInTheDocument();
  expect(confirmation).not.toBeChecked();
  expect(submit).toBeDisabled();

  fireEvent.click(confirmation);
  fireEvent.change(screen.getByLabelText('Community Node'), {
    target: { value: 'https://index-b.example' },
  });

  expect(confirmation).not.toBeChecked();
  expect(submit).toBeDisabled();
  expect(screen.queryByText('The request is pending review.')).not.toBeInTheDocument();

  fireEvent.click(confirmation);
  fireEvent.click(submit);
  await waitFor(() => expect(submitCommunityNodeIndexingRequest).toHaveBeenCalledTimes(2));
  expect(submitCommunityNodeIndexingRequest).toHaveBeenLastCalledWith(
    expect.objectContaining({ base_url: 'https://index-b.example' })
  );
});

test('failed private request consumes confirmation and node change clears the error', async () => {
  const submitCommunityNodeIndexingRequest = vi
    .fn()
    .mockRejectedValueOnce(new Error('request failed'))
    .mockResolvedValueOnce({ request_id: 'request-4', status: 'approved' });
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(submitCommunityNodeIndexingRequest)}
      target={{
        kind: 'private_channel',
        topicId: 'kukuri:topic:demo',
        channelId: 'channel-1',
        channelLabel: 'Core',
      }}
      eligibleNodeBaseUrls={['https://index-a.example', 'https://index-b.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );

  const confirmation = screen.getByRole('checkbox');
  const submit = screen.getByRole('button', { name: 'Submit request' });
  fireEvent.click(confirmation);
  fireEvent.click(submit);

  expect(await screen.findByText('The indexing request failed. Please try again later.')).toBeInTheDocument();
  expect(confirmation).not.toBeChecked();
  expect(submit).toBeDisabled();

  fireEvent.click(confirmation);
  fireEvent.change(screen.getByLabelText('Community Node'), {
    target: { value: 'https://index-b.example' },
  });
  expect(confirmation).not.toBeChecked();
  expect(submit).toBeDisabled();
  expect(
    screen.queryByText('The indexing request failed. Please try again later.')
  ).not.toBeInTheDocument();
});

test('private confirmation and status reset when the request target changes', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn().mockResolvedValue({
    request_id: 'request-5',
    status: 'approved',
  });
  const eligibleNodeBaseUrls = ['https://index.example'];
  const props = {
    api: dialogApi(submitCommunityNodeIndexingRequest),
    eligibleNodeBaseUrls,
    onOpenChange: vi.fn(),
    onOpenCommunityNodeSettings: vi.fn(),
  };
  const { rerender } = render(
    <CommunityIndexingRequestDialog
      {...props}
      target={{
        kind: 'private_channel',
        topicId: 'kukuri:topic:demo',
        channelId: 'channel-1',
        channelLabel: 'Core',
      }}
    />
  );

  const confirmation = screen.getByRole('checkbox');
  fireEvent.click(confirmation);
  fireEvent.click(screen.getByRole('button', { name: 'Submit request' }));
  expect(await screen.findByText('Indexing is approved.')).toBeInTheDocument();
  fireEvent.click(confirmation);

  rerender(
    <CommunityIndexingRequestDialog
      {...props}
      target={{
        kind: 'private_channel',
        topicId: 'kukuri:topic:demo',
        channelId: 'channel-2',
        channelLabel: 'Review',
      }}
    />
  );

  expect(screen.getByRole('checkbox')).not.toBeChecked();
  expect(screen.getByRole('button', { name: 'Submit request' })).toBeDisabled();
  expect(screen.queryByText('Indexing is approved.')).not.toBeInTheDocument();
});

test('stale private request result does not overwrite a changed target', async () => {
  let resolveRequest:
    | ((value: { request_id: string; status: 'approved' }) => void)
    | undefined;
  const response = new Promise<{ request_id: string; status: 'approved' }>((resolve) => {
    resolveRequest = resolve;
  });
  const submitCommunityNodeIndexingRequest = vi.fn().mockReturnValue(response);
  const eligibleNodeBaseUrls = ['https://index.example'];
  const props = {
    api: dialogApi(submitCommunityNodeIndexingRequest),
    eligibleNodeBaseUrls,
    onOpenChange: vi.fn(),
    onOpenCommunityNodeSettings: vi.fn(),
  };
  const { rerender } = render(
    <CommunityIndexingRequestDialog
      {...props}
      target={{
        kind: 'private_channel',
        topicId: 'kukuri:topic:demo',
        channelId: 'channel-1',
        channelLabel: 'Core',
      }}
    />
  );

  fireEvent.click(screen.getByRole('checkbox'));
  fireEvent.click(screen.getByRole('button', { name: 'Submit request' }));
  rerender(
    <CommunityIndexingRequestDialog
      {...props}
      target={{
        kind: 'private_channel',
        topicId: 'kukuri:topic:demo',
        channelId: 'channel-2',
        channelLabel: 'Review',
      }}
    />
  );

  await act(async () => {
    resolveRequest?.({ request_id: 'request-6', status: 'approved' });
    await response;
  });

  expect(screen.getByText('Target: Review')).toBeInTheDocument();
  expect(screen.queryByText('Indexing is approved.')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Submit request' })).toBeDisabled();
});

// #698: 適格一覧の参照が変わるだけ(内容不変)では確認状態を消さない。
test('an equal eligible list rendered as a new array keeps the private confirmation', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn().mockResolvedValue({
    request_id: 'request-1',
    status: 'pending',
  });
  const api = dialogApi(submitCommunityNodeIndexingRequest);
  const target = { kind: 'private_channel' as const, topicId: 'kukuri:topic:demo', channelId: 'ch-1', channelLabel: 'demo' };
  const { rerender } = render(
    <CommunityIndexingRequestDialog
      api={api}
      target={target}
      eligibleNodeBaseUrls={['https://index.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );
  fireEvent.click(screen.getByRole('checkbox'));
  expect(screen.getByRole('checkbox')).toBeChecked();

  rerender(
    <CommunityIndexingRequestDialog
      api={api}
      target={target}
      eligibleNodeBaseUrls={['https://index.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );
  expect(screen.getByRole('checkbox')).toBeChecked();
  expect(screen.getByRole('button', { name: 'Submit request' })).toBeEnabled();
});

// #698: 選択ノードが適格一覧から外れると、確認済みでも申請(秘密値)を送らない。
test('a selected node dropped from the eligible list cannot receive a private request', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn().mockResolvedValue({
    request_id: 'request-1',
    status: 'pending',
  });
  const api = dialogApi(submitCommunityNodeIndexingRequest);
  const target = { kind: 'private_channel' as const, topicId: 'kukuri:topic:demo', channelId: 'ch-1', channelLabel: 'demo' };
  const { rerender } = render(
    <CommunityIndexingRequestDialog
      api={api}
      target={target}
      eligibleNodeBaseUrls={['https://index-a.example', 'https://index-b.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );
  fireEvent.click(screen.getByRole('checkbox'));
  expect(screen.getByRole('button', { name: 'Submit request' })).toBeEnabled();

  // A の同意/能力が失効し、適格一覧が [B] だけになる。
  rerender(
    <CommunityIndexingRequestDialog
      api={api}
      target={target}
      eligibleNodeBaseUrls={['https://index-b.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );
  // 内容が変わったので確認は消え、送信は無効。改めて確認しても送信先は B になる。
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  expect(screen.getByRole('button', { name: 'Submit request' })).toBeDisabled();
  fireEvent.click(screen.getByRole('checkbox'));
  fireEvent.click(screen.getByRole('button', { name: 'Submit request' }));
  await waitFor(() => expect(submitCommunityNodeIndexingRequest).toHaveBeenCalledTimes(1));
  expect(submitCommunityNodeIndexingRequest).toHaveBeenCalledWith(
    expect.objectContaining({ base_url: 'https://index-b.example' })
  );
  expect(submitCommunityNodeIndexingRequest).not.toHaveBeenCalledWith(
    expect.objectContaining({ base_url: 'https://index-a.example' })
  );
});

test('explains the server-side indexing request gate with stable codes', async () => {
  // #713: 索引未提供・有効化失効のノードは申請を受け付けない。安定コードで案内する。
  const submitCommunityNodeIndexingRequest = vi
    .fn()
    .mockRejectedValueOnce(new InvokeError('INDEXING_REQUEST_NOT_CONFIGURED', 'gate'))
    .mockRejectedValueOnce(new InvokeError('INDEXING_REQUEST_NOT_ACTIVATED', 'gate'));
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(submitCommunityNodeIndexingRequest)}
      target={{ kind: 'public_topic', topicId: 'kukuri:topic:demo' }}
      eligibleNodeBaseUrls={['https://index-a.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );

  const submit = screen.getByRole('button', { name: 'Submit request' });
  fireEvent.click(submit);
  expect(
    await screen.findByText(
      'This Community Node does not provide indexing, so it does not accept requests.'
    )
  ).toBeInTheDocument();

  fireEvent.click(submit);
  expect(
    await screen.findByText(
      'Indexing on this Community Node is temporarily unavailable, so it does not accept requests.'
    )
  ).toBeInTheDocument();
});

// #975 AC-3 / AC-4: 公開トピックは開いた時点で対象付きの状態読取りを行い、確定した状態を表示する。
test('public target reads its indexing status on open and reflects the submitted request', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn().mockResolvedValue({
    request_id: 'request-7',
    status: 'pending',
  });
  const readCommunityNodeIndexingStatus = vi.fn().mockResolvedValue({
    requests: [],
    target: { scope_kind: 'public_topic', scope_id: 'kukuri:topic:demo', supported: false },
  });
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(submitCommunityNodeIndexingRequest, readCommunityNodeIndexingStatus)}
      target={{ kind: 'public_topic', topicId: 'kukuri:topic:demo' }}
      eligibleNodeBaseUrls={['https://index.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );

  expect(
    await screen.findByText('This target is not indexed by this Community Node and has no request.')
  ).toBeInTheDocument();
  expect(readCommunityNodeIndexingStatus).toHaveBeenCalledTimes(1);
  expect(readCommunityNodeIndexingStatus).toHaveBeenCalledWith({
    base_url: 'https://index.example',
    scope_kind: 'public_topic',
    topic_id: 'kukuri:topic:demo',
    channel_id: null,
    confirm_private_channel_secret_disclosure: false,
  });
  const submit = screen.getByRole('button', { name: 'Submit request' });
  expect(submit).toBeEnabled();
  fireEvent.click(submit);

  expect(await screen.findByText('The request is pending review.')).toBeInTheDocument();
  expect(
    screen.queryByText('This target is not indexed by this Community Node and has no request.')
  ).not.toBeInTheDocument();
  expect(
    screen.getByText('A request already exists for this target, so it cannot be submitted again.')
  ).toBeInTheDocument();
  expect(submit).toBeDisabled();
  // 申請応答をそのまま反映し、追加の読取りは行わない。
  expect(readCommunityNodeIndexingStatus).toHaveBeenCalledTimes(1);
});

test('an already indexed or already requested target cannot be submitted again', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn();
  const readCommunityNodeIndexingStatus = vi
    .fn()
    .mockResolvedValueOnce({
      requests: [],
      target: { scope_kind: 'public_topic', scope_id: 'kukuri:topic:demo', supported: true },
    })
    .mockResolvedValueOnce({
      requests: [
        {
          request_id: 'request-8',
          scope_kind: 'public_topic',
          target_id: 'kukuri:topic:demo',
          status: 'rejected',
          created_at: 1000,
          decided_at: 2000,
        },
      ],
      target: { scope_kind: 'public_topic', scope_id: 'kukuri:topic:demo', supported: false },
    });
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(submitCommunityNodeIndexingRequest, readCommunityNodeIndexingStatus)}
      target={{ kind: 'public_topic', topicId: 'kukuri:topic:demo' }}
      eligibleNodeBaseUrls={['https://index-a.example', 'https://index-b.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );

  expect(await screen.findByText(/This target is indexed by this Community Node\./)).toBeInTheDocument();
  expect(screen.getByText('This target is already indexed, so no request is needed.')).toBeInTheDocument();
  const submit = screen.getByRole('button', { name: 'Submit request' });
  expect(submit).toBeDisabled();
  fireEvent.click(submit);
  expect(submitCommunityNodeIndexingRequest).not.toHaveBeenCalled();

  // 別ノードでは却下済み: 状態を断定表示し、再申請は塞ぐ。
  fireEvent.change(screen.getByLabelText('Community Node'), {
    target: { value: 'https://index-b.example' },
  });
  expect(await screen.findByText('Indexing was rejected.')).toBeInTheDocument();
  expect(
    screen.getByText('A request already exists for this target, so it cannot be submitted again.')
  ).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Submit request' })).toBeDisabled();
  expect(readCommunityNodeIndexingStatus).toHaveBeenLastCalledWith(
    expect.objectContaining({ base_url: 'https://index-b.example', scope_kind: 'public_topic' })
  );
});

// #975 INVAR-3: 非公開チャンネルは開いた時点では一覧だけを読み(秘密値なし)、対象付きの読取りは明示確認後だけ。
test('private target reads only own requests until the disclosure is confirmed for a status check', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn();
  const readCommunityNodeIndexingStatus = vi
    .fn()
    .mockResolvedValueOnce(EMPTY_STATUS)
    .mockResolvedValueOnce({
      requests: [],
      target: { scope_kind: 'private_channel', scope_id: 'channel-1', supported: true },
    });
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(submitCommunityNodeIndexingRequest, readCommunityNodeIndexingStatus)}
      target={{
        kind: 'private_channel',
        topicId: 'kukuri:topic:demo',
        channelId: 'channel-1',
        channelLabel: 'Core',
      }}
      eligibleNodeBaseUrls={['https://index.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );

  expect(await screen.findByText('You have no request on this Community Node.')).toBeInTheDocument();
  expect(readCommunityNodeIndexingStatus).toHaveBeenCalledTimes(1);
  expect(readCommunityNodeIndexingStatus).toHaveBeenCalledWith({
    base_url: 'https://index.example',
    scope_kind: null,
    topic_id: null,
    channel_id: null,
    confirm_private_channel_secret_disclosure: false,
  });
  const check = screen.getByRole('button', { name: 'Check indexing status' });
  expect(check).toBeDisabled();
  fireEvent.click(check);
  expect(readCommunityNodeIndexingStatus).toHaveBeenCalledTimes(1);

  const confirmation = screen.getByRole('checkbox');
  fireEvent.click(confirmation);
  expect(check).toBeEnabled();
  fireEvent.click(check);
  await waitFor(() => expect(readCommunityNodeIndexingStatus).toHaveBeenCalledTimes(2));
  expect(readCommunityNodeIndexingStatus).toHaveBeenLastCalledWith({
    base_url: 'https://index.example',
    scope_kind: 'private_channel',
    topic_id: 'kukuri:topic:demo',
    channel_id: 'channel-1',
    confirm_private_channel_secret_disclosure: true,
  });
  // 確認は 1 回で消費され、確定した状態を表示し、既に索引対象なら申請を塞ぐ。
  expect(confirmation).not.toBeChecked();
  expect(await screen.findByText(/This target is indexed by this Community Node\./)).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Check indexing status' })).not.toBeInTheDocument();
  fireEvent.click(confirmation);
  expect(screen.getByRole('button', { name: 'Submit request' })).toBeDisabled();
  expect(submitCommunityNodeIndexingRequest).not.toHaveBeenCalled();
});

test('a failed status read is shown as unknown with its reason and does not block submission', async () => {
  const submitCommunityNodeIndexingRequest = vi.fn().mockResolvedValue({
    request_id: 'request-9',
    status: 'pending',
  });
  const readCommunityNodeIndexingStatus = vi
    .fn()
    .mockRejectedValueOnce(new InvokeError('INDEXING_REQUEST_NOT_ACTIVATED', 'gate'))
    .mockResolvedValueOnce({
      requests: [],
      target: { scope_kind: 'public_topic', scope_id: 'kukuri:topic:demo', supported: false },
    });
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(submitCommunityNodeIndexingRequest, readCommunityNodeIndexingStatus)}
      target={{ kind: 'public_topic', topicId: 'kukuri:topic:demo' }}
      eligibleNodeBaseUrls={['https://index.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );

  expect(
    await screen.findByText(
      'Indexing status could not be checked. This does not mean the target is not indexed.'
    )
  ).toBeInTheDocument();
  expect(
    screen.getByText(
      'Indexing on this Community Node is temporarily unavailable, so it does not accept requests.'
    )
  ).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Submit request' })).toBeEnabled();

  fireEvent.click(screen.getByRole('button', { name: 'Check status again' }));
  expect(
    await screen.findByText('This target is not indexed by this Community Node and has no request.')
  ).toBeInTheDocument();
  expect(readCommunityNodeIndexingStatus).toHaveBeenCalledTimes(2);
});

test('a stale status response does not overwrite the status of a newly selected node', async () => {
  let resolveFirst: ((value: unknown) => void) | undefined;
  const first = new Promise((resolve) => {
    resolveFirst = resolve;
  });
  const readCommunityNodeIndexingStatus = vi
    .fn()
    .mockReturnValueOnce(first)
    .mockResolvedValueOnce({
      requests: [],
      target: { scope_kind: 'public_topic', scope_id: 'kukuri:topic:demo', supported: false },
    });
  render(
    <CommunityIndexingRequestDialog
      api={dialogApi(vi.fn(), readCommunityNodeIndexingStatus)}
      target={{ kind: 'public_topic', topicId: 'kukuri:topic:demo' }}
      eligibleNodeBaseUrls={['https://index-a.example', 'https://index-b.example']}
      onOpenChange={vi.fn()}
      onOpenCommunityNodeSettings={vi.fn()}
    />
  );
  expect(screen.getByText('Checking indexing status…')).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Community Node'), {
    target: { value: 'https://index-b.example' },
  });
  expect(
    await screen.findByText('This target is not indexed by this Community Node and has no request.')
  ).toBeInTheDocument();

  await act(async () => {
    resolveFirst?.({
      requests: [],
      target: { scope_kind: 'public_topic', scope_id: 'kukuri:topic:demo', supported: true },
    });
    await first;
  });
  expect(screen.queryByText(/This target is indexed by this Community Node\./)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Submit request' })).toBeEnabled();
});
