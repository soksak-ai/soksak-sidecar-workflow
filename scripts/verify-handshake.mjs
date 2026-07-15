#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { INTERFACE, ID, VERSION, assertNoLinkPath, parseOptions } from "./release-contract.mjs";

const { binary } = parseOptions(process.argv.slice(2), ["binary"]);
const executable = assertNoLinkPath(binary, "file");
const result = spawnSync(executable, ["--handshake"], { encoding: "utf8", timeout: 10_000, windowsHide: true });
if (result.error) throw result.error;
if (result.status !== 0 || result.stderr !== "") throw new Error(`handshake failed: ${result.stderr || result.status}`);
const actual = JSON.parse(result.stdout);
const expected = {
  unit: { kind: "sidecar", id: ID, version: VERSION },
  interface: INTERFACE,
  transport: "stdio-json-lines",
  ops: ["run", "ping", "reconcile", "research", "next", "submit", "issuerize", "export"],
};
if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error(`handshake mismatch: ${JSON.stringify(actual)}`);
process.stdout.write(`${JSON.stringify({ ok: true, interface: INTERFACE })}\n`);
