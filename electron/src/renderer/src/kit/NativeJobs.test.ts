import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';

it('releases native job callbacks on start rejection and terminal completion', () => {
  const script = fileURLToPath(new URL('../../../../scripts/job-lifecycle.mjs', import.meta.url));
  const output = execFileSync(process.execPath, [script], { encoding: 'utf8', timeout: 10_000 });
  expect(output).toContain('native job lifecycle: passed');
}, 15_000);
