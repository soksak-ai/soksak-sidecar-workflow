import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ID = "soksak-sidecar-workflow";
const VERSION = "0.0.1";
const COMMIT = "a".repeat(40);
const SPEC_SHA = "d7f54852754195527f125d1fc11362316157d19b";
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

test("release assembly is deterministic and binds manifest plus conformance to one source", () => {
  const scratch = fs.mkdtempSync(path.join(fs.realpathSync.native(os.tmpdir()), "soksak-workflow-release-"));
  try {
    const artifacts = path.join(scratch, "artifacts");
    const first = path.join(scratch, "first");
    const second = path.join(scratch, "second");
    fs.mkdirSync(artifacts);
    const matrix = JSON.parse(fs.readFileSync(path.join(root, "release/targets.json"), "utf8"));
    for (const { target } of matrix) {
      const asset = `${ID}-${VERSION}-${target}.tar.gz`;
      const bytes = Buffer.from(`fixture:${target}\n`);
      fs.writeFileSync(path.join(artifacts, asset), bytes);
      fs.writeFileSync(path.join(artifacts, `artifact-${target}.json`), `${JSON.stringify({
        id: ID,
        version: VERSION,
        releaseTag: `v${VERSION}`,
        target,
        asset,
        sha256: sha256(bytes),
        format: "tar.gz",
        entrypoint: `bin/${ID}${target.includes("windows") ? ".exe" : ""}`,
        interface: { id: "soksak-spec-sidecar-workflow", version: VERSION },
      }, null, 2)}\n`);
    }
    for (const out of [first, second]) {
      execFileSync(process.execPath, [
        "scripts/build-release.mjs",
        "--commit", COMMIT,
        "--tag", `v${VERSION}`,
        "--artifacts", artifacts,
        "--out", out,
      ], { cwd: root });
    }
    const files = fs.readdirSync(first).sort();
    assert.deepEqual(files, [
      "conformance-interface.json",
      "conformance-release.json",
      "conformance-sidecar.json",
      "release.json",
    ]);
    for (const file of files) {
      assert.deepEqual(fs.readFileSync(path.join(first, file)), fs.readFileSync(path.join(second, file)));
    }
    const release = JSON.parse(fs.readFileSync(path.join(first, "release.json"), "utf8"));
    assert.equal(release.spec, "soksak-spec-release@0.0.1");
    assert.equal(release.kind, "sidecar");
    assert.equal(release.id, ID);
    assert.equal(release.version, VERSION);
    assert.equal(release.source.commit, COMMIT);
    assert.equal(release.artifacts.length, 5);
    assert.equal("buildDependencies" in release, false);
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(root, "validation/spec-validator.json"), "utf8")), {
      repository: "https://github.com/soksak-ai/soksak-spec",
      commit: SPEC_SHA,
      validator: "packages/plugin-spec/bin/validate.mjs",
    });
    for (const name of files.filter((name) => name.startsWith("conformance-"))) {
      const report = JSON.parse(fs.readFileSync(path.join(first, name), "utf8"));
      assert.equal(report.spec, "soksak-spec-conformance@0.0.1");
      assert.equal(report.subject.manifestSha256.length, 64);
    }
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
});
