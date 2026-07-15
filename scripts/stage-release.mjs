#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import {
  ROOT, assertNoLinkPath, binaryName, ensureEmptyDirectory, parseOptions, readRegularFile, targetEntry, writeRegularFile,
} from "./release-contract.mjs";

const options = parseOptions(process.argv.slice(2), ["target", "binary", "out"]);
targetEntry(options.target);
const input = assertNoLinkPath(options.binary, "file");
const out = ensureEmptyDirectory(options.out);
const binDir = path.join(out, "bin");
fs.mkdirSync(binDir);
const staged = path.join(binDir, binaryName(options.target));
writeRegularFile(staged, readRegularFile(input), 0o755);
for (const name of ["INTERFACE.md", "LICENSE", "THIRD-PARTY-NOTICES"]) {
  writeRegularFile(path.join(out, name), readRegularFile(path.join(ROOT, name)));
}
const verify = spawnSync(process.execPath, [path.join(ROOT, "scripts", "verify-handshake.mjs"), "--binary", staged], {
  cwd: ROOT, encoding: "utf8", windowsHide: true,
});
if (verify.error) throw verify.error;
if (verify.status !== 0) throw new Error(`staged handshake failed: ${verify.stderr}`);
process.stdout.write(`${JSON.stringify({ target: options.target, staged, handshake: JSON.parse(verify.stdout) })}\n`);
