#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  TAG, assertCommit, assertNoLinkPath, parseOptions, readRegularFile, sha256,
} from "./release-contract.mjs";
import {
  assertImmutableReleasePolicy,
  githubCliEnvironment,
} from "./github-release-policy.mjs";

const options = parseOptions(process.argv.slice(2), ["repository", "commit", "assets"]);
assertCommit(options.commit);
const assets = assertNoLinkPath(options.assets, "directory");
const files = fs.readdirSync(assets)
  .sort((left, right) => Buffer.compare(Buffer.from(left), Buffer.from(right)))
  .map((name) => ({ name, path: assertNoLinkPath(path.join(assets, name), "file") }));
if (files.length !== 9) throw new Error("release directory must contain five archives, release.json, and three reports");
const githubEnvironment = githubCliEnvironment();
await assertImmutableReleasePolicy({
  repository: options.repository,
  token: githubEnvironment.GH_TOKEN,
});

function gh(args, { allowMissing = false } = {}) {
  const result = spawnSync("gh", args, {
    encoding: "utf8",
    windowsHide: true,
    env: githubEnvironment,
  });
  if (result.error) throw result.error;
  if (result.status !== 0 && !allowMissing) throw new Error(`gh ${args.join(" ")} failed:\n${result.stderr}`);
  return result;
}

const viewArgs = ["release", "view", TAG, "--repo", options.repository, "--json", "isDraft,targetCommitish,assets"];
let view = gh(viewArgs, { allowMissing: true });
if (view.status !== 0) {
  gh(["release", "create", TAG, "--repo", options.repository, "--draft", "--verify-tag", "--target", options.commit, "--title", TAG]);
  view = gh(viewArgs);
}
let state = JSON.parse(view.stdout);
if (!state.isDraft) {
  if (state.targetCommitish !== options.commit) throw new Error("published release targets a different commit");
} else if (state.targetCommitish !== options.commit) {
  throw new Error("draft release targets a different commit");
}

const existing = new Map(state.assets.map((asset) => [asset.name, asset]));
if (!state.isDraft) {
  const actual = [...existing.keys()].sort((left, right) => Buffer.compare(Buffer.from(left), Buffer.from(right)));
  const expected = files.map(({ name }) => name);
  if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error("published release asset closure differs from the owner release");
} else {
  for (const file of files) {
    if (!existing.has(file.name)) gh(["release", "upload", TAG, file.path, "--repo", options.repository]);
  }
}

const verifyRoot = fs.mkdtempSync(path.join(fs.realpathSync.native(os.tmpdir()), "soksak-workflow-publish-"));
try {
  for (const file of files) {
    gh(["release", "download", TAG, "--repo", options.repository, "--pattern", file.name, "--dir", verifyRoot]);
    const downloaded = assertNoLinkPath(path.join(verifyRoot, file.name), "file");
    if (sha256(readRegularFile(downloaded)) !== sha256(readRegularFile(file.path))) throw new Error(`published asset digest mismatch: ${file.name}`);
  }
} finally {
  fs.rmSync(verifyRoot, { recursive: true, force: true });
}
if (state.isDraft) gh(["release", "edit", TAG, "--repo", options.repository, "--draft=false", "--latest=false"]);
