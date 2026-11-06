// The invite panels' rules that need no page: `node --test web/invites.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { privateAllowed } from './invites.js';

test("a joining page ignores ?lan: only the invite's server allows private addresses", () => {
  assert.equal(privateAllowed('lan', '', true), true);
  assert.equal(privateAllowed('cloudflare', '?lan=1', true), false, 'a link the host sent with ?lan=1');
  assert.equal(privateAllowed('google', '?nolock=1&lan', true), false);
  // the host's own page may choose it
  assert.equal(privateAllowed('cloudflare', '?lan=1', false), true);
  assert.equal(privateAllowed('cloudflare', '', false), false);
  assert.equal(privateAllowed('lan', '', false), true);
});
