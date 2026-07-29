import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode
} from 'react';
import { rainApi } from '../api/client';
import type { Credentials, User } from '../api/types';
import { toAuthState, type AuthState } from './authState';

interface AuthContextValue {
  state: AuthState;
  login(credentials: Credentials): Promise<User>;
  register(credentials: Credentials): Promise<User>;
  logout(): Promise<void>;
  refresh(): Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AuthState>({ status: 'LOADING' });

  const refresh = useCallback(async () => {
    try {
      setState(toAuthState(await rainApi.me()));
    } catch {
      setState({ status: 'GUEST' });
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const login = useCallback(async (credentials: Credentials) => {
    const user = await rainApi.login(credentials);
    setState({ status: 'AUTHENTICATED', user });
    return user;
  }, []);

  const register = useCallback((credentials: Credentials) => {
    return rainApi.register(credentials);
  }, []);

  const logout = useCallback(async () => {
    try {
      await rainApi.logout();
    } finally {
      setState({ status: 'GUEST' });
    }
  }, []);

  const value = useMemo(
    () => ({ state, login, register, logout, refresh }),
    [state, login, register, logout, refresh]
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const value = useContext(AuthContext);
  if (!value) {
    throw new Error('useAuth must be used inside AuthProvider');
  }
  return value;
}
