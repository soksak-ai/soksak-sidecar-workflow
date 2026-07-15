import assert from "node:assert/strict";
import { test } from "node:test";

import {
  assertImmutableReleasePolicy,
  githubCliEnvironment,
} from "../scripts/github-release-policy.mjs";

function reply(status, value, { jsonError = null } = {}) {
  return {
    status,
    async json() {
      if (jsonError) throw jsonError;
      return value;
    },
    async text() {
      return value === undefined ? "" : JSON.stringify(value);
    },
  };
}

test("immutable release policy requires an exact owner-enforced GitHub response", async () => {
  const calls = [];
  const result = await assertImmutableReleasePolicy({
    repository: "soksak-ai/soksak-sidecar-workflow",
    token: "installation-token",
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      return reply(200, { enabled: true, enforced_by_owner: true });
    },
  });
  assert.deepEqual(result, { enabled: true, enforced_by_owner: true });
  assert.equal(calls[0].url, "https://api.github.com/repos/soksak-ai/soksak-sidecar-workflow/immutable-releases");
  assert.equal(calls[0].options.method, "GET");
  assert.equal(calls[0].options.redirect, "error");
  assert.equal(calls[0].options.headers.Authorization, "Bearer installation-token");
  assert.equal(calls[0].options.headers["X-GitHub-Api-Version"], "2026-03-10");
});

test("immutable release policy fails closed on status, JSON, and either policy bit", async () => {
  const base = {
    repository: "soksak-ai/soksak-sidecar-workflow",
    token: "installation-token",
  };
  await assert.rejects(
    assertImmutableReleasePolicy({ ...base, fetchImpl: async () => reply(404, { message: "Not Found" }) }),
    /must return HTTP 200/,
  );
  await assert.rejects(
    assertImmutableReleasePolicy({ ...base, fetchImpl: async () => reply(200, undefined, { jsonError: new Error("bad JSON") }) }),
    /must return JSON/,
  );
  await assert.rejects(
    assertImmutableReleasePolicy({ ...base, fetchImpl: async () => reply(200, { enabled: false, enforced_by_owner: true }) }),
    /enabled and owner-enforced/,
  );
  await assert.rejects(
    assertImmutableReleasePolicy({ ...base, fetchImpl: async () => reply(200, { enabled: true, enforced_by_owner: false }) }),
    /enabled and owner-enforced/,
  );
});

test("GitHub CLI receives only the dedicated installation token", () => {
  assert.throws(() => githubCliEnvironment({ GITHUB_TOKEN: "default" }), /SOKSAK_RELEASE_TOKEN is required/);
  const env = githubCliEnvironment({
    SOKSAK_RELEASE_TOKEN: "installation-token",
    GITHUB_TOKEN: "default",
    PATH: "/usr/bin",
  });
  assert.equal(env.GH_TOKEN, "installation-token");
  assert.equal(env.GITHUB_TOKEN, undefined);
  assert.equal(env.PATH, "/usr/bin");
});
