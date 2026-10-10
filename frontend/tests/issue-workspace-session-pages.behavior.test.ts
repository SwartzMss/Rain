import { afterEach, describe, expect, it, vi } from 'vitest';
import { openWorkspaceSessionPageLease } from '../src/features/files/hooks/useIssueWorkspaceSession';

class MockBroadcastChannel {
  static channels = new Map<string, Set<MockBroadcastChannel>>();
  onmessage: ((event: MessageEvent) => void) | null = null;
  private readonly peers: Set<MockBroadcastChannel>;

  constructor(private readonly name: string) {
    const peers = MockBroadcastChannel.channels.get(name) ?? new Set<MockBroadcastChannel>();
    peers.add(this);
    MockBroadcastChannel.channels.set(name, peers);
    this.peers = peers;
  }

  postMessage(data: unknown): void {
    for (const peer of this.peers) {
      if (peer === this) continue;
      window.setTimeout(() => peer.onmessage?.({ data } as MessageEvent), 0);
    }
  }

  close(): void {
    this.peers.delete(this);
    if (this.peers.size === 0) MockBroadcastChannel.channels.delete(this.name);
  }
}

describe('Issue workspace page sessions', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    MockBroadcastChannel.channels.clear();
  });

  it('forks a copied session when another page already owns it', async () => {
    vi.stubGlobal('navigator', {});
    vi.stubGlobal('BroadcastChannel', MockBroadcastChannel);

    const originalPage = await openWorkspaceSessionPageLease('session-a', 'page-a', false, 'document-a');
    const copiedPage = await openWorkspaceSessionPageLease('session-a', 'page-a', true, 'document-b');

    expect(originalPage.collision).toBe(false);
    expect(copiedPage.collision).toBe(true);

    const copiedPageSession = await openWorkspaceSessionPageLease('session-b', 'page-b', false, 'document-b');
    expect(copiedPageSession.collision).toBe(false);

    copiedPage.close();
    copiedPageSession.close();
    originalPage.close();
  });

  it('uses the browser lock manager to reject a second page before it can resume the session', async () => {
    const heldLocks = new Set<string>();
    const locks = {
      request: (
        name: string,
        _options: unknown,
        callback: (lock: { name: string } | null) => Promise<void>
      ) => {
        const lock = heldLocks.has(name) ? null : { name };
        if (lock) heldLocks.add(name);
        return Promise.resolve()
          .then(() => callback(lock))
          .finally(() => { if (lock) heldLocks.delete(name); });
      }
    };
    vi.stubGlobal('navigator', { locks });
    vi.stubGlobal('BroadcastChannel', undefined);

    const originalPage = await openWorkspaceSessionPageLease('session-lock', 'page-copy', false, 'document-a');
    vi.resetModules();
    const { openWorkspaceSessionPageLease: openCopiedPageLease } = await import('../src/features/files/hooks/useIssueWorkspaceSession');
    const copiedPage = await openCopiedPageLease('session-lock', 'page-copy', true, 'document-b');

    expect(originalPage.collision).toBe(false);
    expect(copiedPage.collision).toBe(true);

    originalPage.close();
    await vi.waitFor(() => expect(heldLocks.size).toBe(0));
  });
});
