const API_VERSION = "2026-03-10";
const REPOSITORY_SLUG = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;

export function githubCliEnvironment(environment = process.env) {
  const token = environment.SOKSAK_RELEASE_TOKEN;
  if (typeof token !== "string" || token.length === 0) {
    throw new Error("SOKSAK_RELEASE_TOKEN is required");
  }
  const isolated = { ...environment, GH_TOKEN: token };
  delete isolated.GITHUB_TOKEN;
  return isolated;
}

export async function assertImmutableReleasePolicy({
  repository,
  token,
  fetchImpl = globalThis.fetch,
}) {
  if (!REPOSITORY_SLUG.test(repository)) throw new Error("repository must be an exact GitHub slug");
  if (typeof token !== "string" || token.length === 0) throw new Error("release installation token is required");
  if (typeof fetchImpl !== "function") throw new Error("fetch implementation is required");

  const response = await fetchImpl(
    `https://api.github.com/repos/${repository}/immutable-releases`,
    {
      method: "GET",
      redirect: "error",
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${token}`,
        "X-GitHub-Api-Version": API_VERSION,
      },
    },
  );
  if (response.status !== 200) {
    throw new Error(`immutable release policy must return HTTP 200, received ${response.status}`);
  }
  let settings;
  try {
    settings = await response.json();
  } catch {
    throw new Error("immutable release policy must return JSON");
  }
  if (
    !settings ||
    typeof settings !== "object" ||
    Array.isArray(settings) ||
    settings.enabled !== true ||
    settings.enforced_by_owner !== true
  ) {
    throw new Error("immutable releases must be enabled and owner-enforced");
  }
  return settings;
}
