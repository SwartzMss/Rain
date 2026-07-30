export type CursorHistory = Array<string | undefined>;

export function currentCursor(history: CursorHistory): string | undefined {
  return history[history.length - 1];
}

export function advanceCursor(history: CursorHistory, next: string): CursorHistory {
  return [...history, next];
}

export function retreatCursor(history: CursorHistory): CursorHistory {
  return history.length > 1 ? history.slice(0, -1) : history;
}

export async function runAdminAction(options: {
  action: () => Promise<unknown>;
  reload: () => Promise<unknown>;
  refreshAuth: () => Promise<unknown>;
  selfRevocation: boolean;
}): Promise<void> {
  await options.action();
  if (options.selfRevocation) {
    await options.refreshAuth();
    return;
  }
  await options.reload();
  await options.refreshAuth();
}
