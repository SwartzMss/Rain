import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { AccountPage } from '../src/features/auth/AccountPage';

vi.mock('../src/auth/AuthContext', () => ({
  useAuth: () => ({
    state: { status: 'AUTHENTICATED', user: { id: 'user-1', username: 'alice', role: 'USER' } },
    changePassword: vi.fn()
  })
}));

vi.mock('../src/features/skills/SkillsPage', () => ({
  SkillsPage: () => <div>skill management</div>
}));

describe('account skills navigation', () => {
  it('hides private skill management while keeping account security visible', () => {
    render(<MemoryRouter><AccountPage /></MemoryRouter>);

    expect(screen.queryByRole('tab', { name: '我的 Skills' })).not.toBeInTheDocument();
    expect(screen.queryByText('skill management')).not.toBeInTheDocument();
    expect(screen.getByText('账户安全')).toBeInTheDocument();
    expect(screen.getByLabelText('当前密码')).toBeInTheDocument();
  });
});
