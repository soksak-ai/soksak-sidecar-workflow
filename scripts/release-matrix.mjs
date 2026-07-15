#!/usr/bin/env node
import { readTargetMatrix } from "./release-contract.mjs";
process.stdout.write(`matrix=${JSON.stringify({ include: readTargetMatrix() })}\n`);
