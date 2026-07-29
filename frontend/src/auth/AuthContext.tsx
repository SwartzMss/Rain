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
  logoutAll(): Promise<void>;
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

  useEffect(() => {
    const becomeGuest = () => setState({ status: 'GUEST' });
    window.addEventListener('rain:authentication-required', becomeGuest);
    return () => window.removeEventListener('rain:authentication-required', becomeGuest);
  }, []);

  const login = useCallback(async (credentials: Credentials) => {
    const user = await rainApi.login(credentials);
    setState({ status: 'AUTHENTICATED', user });
    return user;
  }, []);

  const register = useCallback((credentials: Credentials) => {
    return rainApi.register(credentials);
  }, []);

  const logout = useCallback(async () => {
    await rainApi.logout();
    setState({ status: 'GUEST' });
  }, []);

  const logoutAll = useCallback(async () => {
    await rainApi.logoutAll();
    setState({ status: 'GUEST' });
  }, []);

  const value = useMemo(
    () => ({ state, login, register, logout, logoutAll, refresh }),
    [state, login, register, logout, logoutAll, refresh]
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
