import assert from 'node:assert/strict';
import { isAdmin } from '../src/auth/permissions.ts';

assert.equal(isAdmin(null), false);
assert.equal(isAdmin({ id: 'u', username: 'user', role: 'USER' }), false);
assert.equal(isAdmin({ id: 'a', username: 'admin', role: 'ADMIN' }), true);
console.log('admin permission tests passed');
