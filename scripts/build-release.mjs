#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import {
  CONFORMANCE_SPEC, ID, INTERFACE, RELEASE_SPEC, REPOSITORY, SIDECAR_SPEC, TAG, VERSION,
  assertBaseline, assertCommit, assertNoLinkPath, assertTag, ensureEmptyDirectory, jsonBytes,
  parseOptions, readRegularFile, readTargetMatrix, releaseAssetName, releaseIdentity, sha256, writeRegularFile,
} from "./release-contract.mjs";

const options = parseOptions(process.argv.slice(2), ["commit", "tag", "artifacts", "out"]);
assertBaseline();
assertCommit(options.commit);
assertTag(options.tag);
const artifactsDir = assertNoLinkPath(options.artifacts, "directory");
const out = ensureEmptyDirectory(options.out);
const expectedNames = [];
const artifacts = readTargetMatrix().map(({ target }) => {
  const metadataName = `artifact-${target}.json`;
  const metadata = JSON.parse(readRegularFile(path.join(artifactsDir, metadataName)).toString("utf8"));
  const asset = releaseAssetName(target);
  expectedNames.push(asset, metadataName);
  const bytes = readRegularFile(path.join(artifactsDir, asset));
  const expected = {
    id: ID, version: VERSION, releaseTag: TAG, target, asset,
    sha256: sha256(bytes), format: "tar.gz",
    entrypoint: `bin/${ID}${target.includes("windows") ? ".exe" : ""}`,
  };
  for (const [key, value] of Object.entries(expected)) {
    if (metadata[key] !== value) throw new Error(`${metadataName}: ${key} mismatch`);
  }
  if (JSON.stringify(metadata.interface) !== JSON.stringify(INTERFACE)) throw new Error(`${metadataName}: interface mismatch`);
  return {
    target,
    url: `${REPOSITORY}/releases/download/${TAG}/${asset}`,
    sha256: metadata.sha256,
    format: "tar.gz",
    entrypoint: {
      kind: "sidecar",
      interface: INTERFACE,
      process: [{ name: ID, path: metadata.entrypoint }],
    },
  };
});
const actualNames = fs.readdirSync(artifactsDir).sort((left, right) => Buffer.compare(Buffer.from(left), Buffer.from(right)));
expectedNames.sort((left, right) => Buffer.compare(Buffer.from(left), Buffer.from(right)));
if (JSON.stringify(actualNames) !== JSON.stringify(expectedNames)) throw new Error("artifact directory must contain exactly the declared release matrix");

const release = {
  ...releaseIdentity(options.commit),
  dependencies: [],
  artifacts,
};
const releaseBytes = jsonBytes(release);
const manifestSha256 = sha256(releaseBytes);
const evidence = artifacts.map(({ target, sha256: digest }) => ({ target, sha256: digest }));
const report = (contract) => ({
  spec: CONFORMANCE_SPEC,
  subject: { kind: "sidecar", id: ID, version: VERSION, manifestSha256 },
  contract,
  result: "passed",
  validator: { name: "soksak-validate", version: VERSION },
  artifacts: evidence,
});
writeRegularFile(path.join(out, "release.json"), releaseBytes);
writeRegularFile(path.join(out, "conformance-release.json"), jsonBytes(report(RELEASE_SPEC)));
writeRegularFile(path.join(out, "conformance-sidecar.json"), jsonBytes(report(SIDECAR_SPEC)));
writeRegularFile(path.join(out, "conformance-interface.json"), jsonBytes(report(INTERFACE)));
