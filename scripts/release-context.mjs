#!/usr/bin/env node
import { appendFileSync } from "node:fs";
import { ID, TAG, assertCommit } from "./release-contract.mjs";

if (process.env.GITHUB_REF !== "refs/heads/main") throw new Error("release must run on the main branch");
assertCommit(process.env.GITHUB_SHA ?? "");
const output = process.env.GITHUB_OUTPUT;
if (output) appendFileSync(output, `id=${ID}\ntag=${TAG}\n`);
