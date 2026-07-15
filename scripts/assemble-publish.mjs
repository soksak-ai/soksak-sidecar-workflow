#!/usr/bin/env node
import path from "node:path";
import {
  ensureEmptyDirectory, parseOptions, readRegularFile, readTargetMatrix, releaseAssetName, writeRegularFile,
} from "./release-contract.mjs";

const options = parseOptions(process.argv.slice(2), ["artifacts", "release", "out"]);
const out = ensureEmptyDirectory(options.out);
const names = [
  ...readTargetMatrix().map(({ target }) => releaseAssetName(target)),
  "release.json",
  "conformance-interface.json",
  "conformance-release.json",
  "conformance-sidecar.json",
];
for (const name of names) {
  const source = name.endsWith(".tar.gz") ? options.artifacts : options.release;
  writeRegularFile(path.join(out, name), readRegularFile(path.join(source, name)), name.endsWith(".tar.gz") ? 0o644 : 0o644);
}
