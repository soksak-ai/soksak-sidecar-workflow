#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { createRegularFileArchive, inspectRegularFileArchive } from "./archive.mjs";
import {
  ID, INTERFACE, TAG, VERSION, assertNoLinkPath, assertTag, binaryName, ensureEmptyDirectory, jsonBytes, parseOptions, releaseAssetName, sha256, targetEntry, writeRegularFile,
} from "./release-contract.mjs";

const options = parseOptions(process.argv.slice(2), ["target", "tag", "input", "out"]);
targetEntry(options.target);
assertTag(options.tag);
const input = assertNoLinkPath(options.input, "directory");
const out = ensureEmptyDirectory(options.out);
const executable = binaryName(options.target);
const files = [
  { path: "INTERFACE.md", mode: 0o644 },
  { path: "LICENSE", mode: 0o644 },
  { path: "THIRD-PARTY-NOTICES", mode: 0o644 },
  { path: `bin/${executable}`, mode: 0o755 },
];
for (const { path: relative } of files) assertNoLinkPath(path.join(input, relative), "file");
const archive = createRegularFileArchive({ root: input, files });
const entries = inspectRegularFileArchive(archive).map(({ name }) => name);
if (JSON.stringify(entries) !== JSON.stringify(files.map(({ path: relative }) => relative).sort())) throw new Error("archive entry closure mismatch");
const asset = releaseAssetName(options.target);
const metadata = {
  id: ID,
  version: VERSION,
  releaseTag: TAG,
  target: options.target,
  asset,
  sha256: sha256(archive),
  format: "tar.gz",
  entrypoint: `bin/${executable}`,
  interface: INTERFACE,
};
writeRegularFile(path.join(out, asset), archive);
writeRegularFile(path.join(out, `artifact-${options.target}.json`), jsonBytes(metadata));
process.stdout.write(`${JSON.stringify(metadata)}\n`);
