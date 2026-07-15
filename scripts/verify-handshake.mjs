#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { INTERFACE, ID, VERSION, assertNoLinkPath, parseOptions } from "./release-contract.mjs";

// The handshake is a wire message with ONE canonical form: object keys sorted
// lexicographically, compact separators. That is exactly what serde_json emits
// for the binary (its default Map is a sorted BTreeMap), so the form is
// deterministic across every OS and run. The verifier serialises the expected
// identity the same way and compares byte-for-byte, which enforces that the
// binary keeps emitting the canonical form rather than tolerating any order.
function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

const { binary } = parseOptions(process.argv.slice(2), ["binary"]);
const executable = assertNoLinkPath(binary, "file");
const result = spawnSync(executable, ["--handshake"], { encoding: "utf8", timeout: 10_000, windowsHide: true });
if (result.error) throw result.error;
if (result.status !== 0 || result.stderr !== "") throw new Error(`handshake failed: ${result.stderr || result.status}`);

const expected = canonical({
  unit: { kind: "sidecar", id: ID, version: VERSION },
  interface: INTERFACE,
  transport: "stdio-json-lines",
  ops: ["run", "ping", "reconcile", "research", "next", "submit", "issuerize", "export"],
});
if (result.stdout.trim() !== expected) throw new Error(`handshake mismatch: ${result.stdout.trim()}`);
process.stdout.write(`${JSON.stringify({ ok: true, interface: INTERFACE })}\n`);
