#!/usr/bin/env node
// Diagnostic probe: how does this OS report a file's identity across lstat and
// an open handle, and which identity check both ACCEPTS an unchanged file and
// REJECTS a file swapped under the path? Runs on every OS in CI so the answer
// is measured, not guessed. Not part of the release; removed once the correct
// readRegularFile identity check is chosen.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const NOFOLLOW = fs.constants.O_NOFOLLOW ?? 0;

function identity(pathname) {
  const before = fs.lstatSync(pathname);
  const fd = fs.openSync(pathname, fs.constants.O_RDONLY | NOFOLLOW);
  try {
    const after = fs.fstatSync(fd);
    // second independent handle, to see whether handle-based ids are stable
    const fd2 = fs.openSync(pathname, fs.constants.O_RDONLY | NOFOLLOW);
    const after2 = fs.fstatSync(fd2);
    fs.closeSync(fd2);
    return { before, after, after2 };
  } finally {
    fs.closeSync(fd);
  }
}

const candidates = {
  lstat_vs_fstat_strict: (b, a) => b.dev === a.dev && b.ino === a.ino,
  lstat_vs_fstat_ino_only: (b, a) => b.ino === a.ino,
  fstat_vs_fstat_strict: (_b, a, a2) => a.dev === a2.dev && a.ino === a2.ino,
  fstat_isfile_only: (_b, a) => a.isFile(),
  tolerant_zero_ino: (b, a) =>
    b.ino === 0n || a.ino === 0n ? a.isFile() : b.dev === a.dev && b.ino === a.ino,
};

function score(label, b, a, a2) {
  const row = {};
  for (const [name, fn] of Object.entries(candidates)) row[name] = Boolean(fn(b, a, a2));
  return { label, ...row };
}

const dir = fs.mkdtempSync(path.join(os.tmpdir(), "soksak-fsid-"));
try {
  // Case 1: unchanged regular file — every candidate SHOULD accept (true).
  const unchanged = path.join(dir, "unchanged.bin");
  fs.writeFileSync(unchanged, "payload");
  const u = identity(unchanged);

  // Case 2: swapped under the path — a correct check SHOULD reject (false).
  // lstat sees fileA; the path is then replaced with fileB before the open.
  const swapped = path.join(dir, "swapped.bin");
  const other = path.join(dir, "other.bin");
  fs.writeFileSync(swapped, "A");
  fs.writeFileSync(other, "BB");
  const beforeSwap = fs.lstatSync(swapped);
  fs.rmSync(swapped);
  fs.renameSync(other, swapped);
  const fdS = fs.openSync(swapped, fs.constants.O_RDONLY | NOFOLLOW);
  const afterSwap = fs.fstatSync(fdS);
  const fdS2 = fs.openSync(swapped, fs.constants.O_RDONLY | NOFOLLOW);
  const afterSwap2 = fs.fstatSync(fdS2);
  fs.closeSync(fdS2);
  fs.closeSync(fdS);

  const report = {
    platform: process.platform,
    node: process.version,
    O_NOFOLLOW: fs.constants.O_NOFOLLOW ?? null,
    unchanged_values: {
      before: { dev: String(u.before.dev), ino: String(u.before.ino) },
      after: { dev: String(u.after.dev), ino: String(u.after.ino) },
      after2: { dev: String(u.after2.dev), ino: String(u.after2.ino) },
    },
    swapped_values: {
      before: { dev: String(beforeSwap.dev), ino: String(beforeSwap.ino) },
      after: { dev: String(afterSwap.dev), ino: String(afterSwap.ino) },
    },
    accepts_unchanged: score("unchanged", u.before, u.after, u.after2),
    rejects_swapped: score("swapped", beforeSwap, afterSwap, afterSwap2),
  };
  console.log(JSON.stringify(report, null, 2));

  // A candidate is CORRECT iff it accepts the unchanged file AND rejects the swap.
  const correct = Object.keys(candidates).filter(
    (name) => report.accepts_unchanged[name] === true && report.rejects_swapped[name] === false,
  );
  console.log(`CORRECT_CANDIDATES=${correct.join(",")}`);
} finally {
  fs.rmSync(dir, { recursive: true, force: true });
}
