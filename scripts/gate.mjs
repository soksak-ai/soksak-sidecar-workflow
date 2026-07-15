#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT, SPEC_SHA, assertBaseline, assertNoLinkPath } from "./release-contract.mjs";

function run(command, args, env = process.env) {
  const result = spawnSync(command, args, { cwd: ROOT, env, stdio: "inherit", windowsHide: true });
  if (result.error) throw result.error;
  if (result.signal) throw new Error(`${command} ended by signal ${result.signal}`);
  if (result.status !== 0) throw new Error(`${command} exited with status ${result.status}`);
}

function cargoEnvironment() {
  const mirror = process.env.SOKSAK_SPEC_GIT_MIRROR;
  if (!mirror) return process.env;
  const root = assertNoLinkPath(mirror, "directory");
  const commit = spawnSync("git", ["-C", root, "cat-file", "-e", `${SPEC_SHA}^{commit}`], { encoding: "utf8", windowsHide: true });
  if (commit.error) throw commit.error;
  if (commit.status !== 0) throw new Error(`SOKSAK_SPEC_GIT_MIRROR must contain ${SPEC_SHA}`);
  return {
    ...process.env,
    CARGO_NET_GIT_FETCH_WITH_CLI: "true",
    GIT_CONFIG_COUNT: "1",
    GIT_CONFIG_KEY_0: `url.${pathToFileURL(root).href}.insteadOf`,
    GIT_CONFIG_VALUE_0: "https://github.com/soksak-ai/soksak-spec.git",
  };
}

assertBaseline();
const toolchain = fs.readFileSync(path.join(ROOT, "rust-toolchain.toml"), "utf8").match(/channel = "([^"]+)"/)?.[1];
const rustc = spawnSync("rustc", ["--version"], { cwd: ROOT, encoding: "utf8", windowsHide: true });
if (rustc.error) throw rustc.error;
if (rustc.status !== 0 || !rustc.stdout.startsWith(`rustc ${toolchain} `)) throw new Error(`rustc must equal ${toolchain}: ${rustc.stdout}`);
const env = cargoEnvironment();
run("cargo", ["fmt", "--all", "--", "--check"], env);
run("cargo", ["test", "--locked"], env);
const nodeTests = fs.readdirSync(path.join(ROOT, "tests"))
  .filter((name) => name.endsWith(".test.mjs"))
  .sort()
  .map((name) => path.join("tests", name));
if (nodeTests.length === 0) throw new Error("repository gate requires Node contract tests");
run(process.execPath, ["--test", ...nodeTests]);
