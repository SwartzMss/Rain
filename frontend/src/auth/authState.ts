import type { AuthMeResponse, User } from '../api/types';

export type AuthState =
  | { status: 'LOADING' }
  | { status: 'GUEST' }
  | { status: 'AUTHENTICATED'; user: User };

export function toAuthState(response: AuthMeResponse): AuthState {
  if (response.authenticated && response.user) {
    return { status: 'AUTHENTICATED', user: response.user };
  }
  return { status: 'GUEST' };
}

export function authStateAfterRefreshFailure(state: AuthState): AuthState {
  return state.status === 'LOADING' ? { status: 'GUEST' } : state;
}

export function safeReturnPath(value: unknown): string {
  return typeof value === 'string' &&
    value.startsWith('/') &&
    !value.startsWith('//')
    ? value
    : '/';
}
