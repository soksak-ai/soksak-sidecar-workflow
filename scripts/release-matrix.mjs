#!/usr/bin/env node
import { appendFileSync } from "node:fs";
import { readTargetMatrix } from "./release-contract.mjs";

const line = `matrix=${JSON.stringify({ include: readTargetMatrix() })}\n`;
const output = process.env.GITHUB_OUTPUT;
if (output) appendFileSync(output, line);
else process.stdout.write(line);
