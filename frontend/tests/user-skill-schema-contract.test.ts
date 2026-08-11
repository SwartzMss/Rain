import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

describe('UserSkill API type contract', () => {
  it('requires schema_version for every Skill response', () => {
    const source = readFileSync(join(process.cwd(), 'src/api/types.ts'), 'utf8');

    expect(source).toMatch(/schema_version: number;/);
    expect(source).not.toMatch(/schema_version: number \| null;/);
  });
});
