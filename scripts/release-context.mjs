#!/usr/bin/env node
import { TAG, assertCommit } from "./release-contract.mjs";

if (process.env.GITHUB_EVENT_NAME !== "push") throw new Error("release requires a tag push event");
if (process.env.GITHUB_REF !== `refs/tags/${TAG}`) throw new Error(`release ref must equal refs/tags/${TAG}`);
assertCommit(process.env.GITHUB_SHA ?? "");
