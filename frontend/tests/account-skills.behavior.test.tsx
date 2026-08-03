import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
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
  it('opens private skill management from the account page', async () => {
    const user = userEvent.setup();
    render(<MemoryRouter><AccountPage /></MemoryRouter>);

    await user.click(screen.getByRole('tab', { name: '我的 Skills' }));

    expect(screen.getByText('skill management')).toBeInTheDocument();
    expect(screen.queryByLabelText('当前密码')).not.toBeInTheDocument();
  });
});
