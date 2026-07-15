import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const BLOCK = 512;

function octal(value, width) {
  const digits = value.toString(8);
  if (digits.length > width - 1) throw new Error(`tar numeric field overflow: ${value}`);
  return `${"0".repeat(width - digits.length - 1)}${digits}\0`;
}

function field(buffer, offset, width, value) {
  const bytes = Buffer.from(value, "utf8");
  if (bytes.length > width) throw new Error(`tar field too long: ${value}`);
  bytes.copy(buffer, offset);
}

function header(name, size, mode) {
  if (Buffer.byteLength(name) > 100) throw new Error(`tar path exceeds 100 bytes: ${name}`);
  const out = Buffer.alloc(BLOCK);
  field(out, 0, 100, name);
  field(out, 100, 8, octal(mode, 8));
  field(out, 108, 8, octal(0, 8));
  field(out, 116, 8, octal(0, 8));
  field(out, 124, 12, octal(size, 12));
  field(out, 136, 12, octal(0, 12));
  out.fill(0x20, 148, 156);
  out[156] = 0x30;
  field(out, 257, 6, "ustar\0");
  field(out, 263, 2, "00");
  field(out, 265, 32, "root");
  field(out, 297, 32, "root");
  const checksum = out.reduce((sum, byte) => sum + byte, 0);
  field(out, 148, 8, `${checksum.toString(8).padStart(6, "0")}\0 `);
  return out;
}

function safeRelative(relative) {
  const value = relative.split(path.sep).join("/");
  const parts = value.split("/");
  if (
    value.length === 0 || value.length > 512 || value.startsWith("/") ||
    !/^[\x20-\x7e]+$/.test(value) || /[<>:"\\|?*]/.test(value) ||
    parts.some((part) => part === "" || part === "." || part === ".." || Buffer.byteLength(part) > 255 || part.endsWith(" ") || part.endsWith("."))
  ) throw new Error(`unsafe archive path: ${relative}`);
  return value;
}

function openedBytes(absolute, before) {
  const fd = fs.openSync(absolute, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0));
  try {
    const after = fs.fstatSync(fd);
    if (!after.isFile() || before.dev !== after.dev || before.ino !== after.ino) throw new Error(`archive input changed while opening: ${absolute}`);
    return fs.readFileSync(fd);
  } finally {
    fs.closeSync(fd);
  }
}

export function createRegularFileArchive({ root, files }) {
  const absoluteRoot = path.resolve(root);
  const rootStat = fs.lstatSync(absoluteRoot);
  if (rootStat.isSymbolicLink() || !rootStat.isDirectory()) throw new Error("archive root must be a regular directory");
  const entries = files.map(({ path: relative, mode }) => {
    if (![0o644, 0o755].includes(mode)) throw new Error(`explicit archive mode required: ${relative}`);
    const name = safeRelative(relative);
    const absolute = path.join(absoluteRoot, relative);
    const stat = fs.lstatSync(absolute);
    if (stat.isSymbolicLink() || !stat.isFile()) throw new Error(`only regular files may be archived: ${name}`);
    return { name, mode, bytes: openedBytes(absolute, stat) };
  }).sort((left, right) => Buffer.compare(Buffer.from(left.name), Buffer.from(right.name)));
  if (entries.length === 0 || new Set(entries.map(({ name }) => name)).size !== entries.length) throw new Error("archive requires unique regular files");
  const blocks = [];
  for (const entry of entries) {
    blocks.push(header(entry.name, entry.bytes.length, entry.mode), entry.bytes);
    const padding = (BLOCK - (entry.bytes.length % BLOCK)) % BLOCK;
    if (padding) blocks.push(Buffer.alloc(padding));
  }
  blocks.push(Buffer.alloc(BLOCK * 2));
  return zlib.gzipSync(Buffer.concat(blocks), { level: 9, mtime: 0 });
}

export function inspectRegularFileArchive(bytes) {
  const tar = zlib.gunzipSync(bytes);
  const entries = [];
  const seen = new Set();
  let ended = false;
  for (let offset = 0; offset + BLOCK <= tar.length;) {
    const block = tar.subarray(offset, offset + BLOCK);
    if (block.every((byte) => byte === 0)) {
      if (offset + BLOCK * 2 !== tar.length || !tar.subarray(offset + BLOCK).every((byte) => byte === 0)) throw new Error("tar must end with exactly two zero blocks");
      ended = true;
      break;
    }
    if (block.subarray(157, 257).some((byte) => byte !== 0)) throw new Error("tar link fields are forbidden");
    const name = safeRelative(block.subarray(0, 100).toString("utf8").replace(/\0.*$/, ""));
    const size = Number.parseInt(block.subarray(124, 136).toString("ascii").replace(/\0.*$/, "").trim() || "0", 8);
    const type = String.fromCharCode(block[156]);
    if (type !== "0" || seen.has(name) || !Number.isSafeInteger(size)) throw new Error(`invalid regular archive entry: ${name}`);
    seen.add(name);
    entries.push({ name, size, type });
    offset += BLOCK + Math.ceil(size / BLOCK) * BLOCK;
  }
  if (!ended || entries.length === 0) throw new Error("invalid or empty tar archive");
  return entries;
}
