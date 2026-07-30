import type { User } from '../api/types';

export const isAdmin = (user: User | null): boolean => user?.role === 'ADMIN';
