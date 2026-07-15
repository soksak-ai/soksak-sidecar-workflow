import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  parseUnitMetadata, readUnitMetadata, releaseAssetName, releaseIdentity,
} from "../scripts/release-contract.mjs";

test("the release engine derives a later release solely from owner metadata", () => {
  const current = readUnitMetadata();
  const scratch = fs.mkdtempSync(path.join(fs.realpathSync.native(os.tmpdir()), "soksak-workflow-release-metadata-"));
  try {
    const filename = path.join(scratch, "unit.json");
    const nextRaw = {
      ...current,
      version: "0.0.2",
      releaseTag: "v0.0.2",
      interface: { ...current.interface, version: "0.0.2" },
    };
    fs.writeFileSync(filename, `${JSON.stringify(nextRaw, null, 2)}\n`);
    const next = readUnitMetadata(filename);
    assert.deepEqual(next, parseUnitMetadata(nextRaw));
    assert.equal(releaseAssetName("x86_64-unknown-linux-gnu", next), `${next.id}-0.0.2-x86_64-unknown-linux-gnu.tar.gz`);
    assert.deepEqual(releaseIdentity("a".repeat(40), next), {
      spec: "soksak-spec-release@0.0.1",
      kind: "sidecar",
      id: next.id,
      version: "0.0.2",
      source: { repository: next.repository, commit: "a".repeat(40) },
      releaseTag: "v0.0.2",
    });
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
});
