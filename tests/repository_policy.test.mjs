import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relative) => fs.readFileSync(path.join(root, relative), "utf8");

const VERSION = "0.0.1";
const SPEC_SHA = "d7f54852754195527f125d1fc11362316157d19b";
const TARGETS = [
  "aarch64-apple-darwin",
  "aarch64-unknown-linux-gnu",
  "x86_64-apple-darwin",
  "x86_64-pc-windows-msvc",
  "x86_64-unknown-linux-gnu",
];

test("the private Rust unit starts at 0.0.1 and pins the public service harness by exact commit", () => {
  const manifest = read("Cargo.toml");
  assert.match(manifest, /\nversion = "0\.0\.1"\n/);
  assert.match(manifest, /\npublish = false\n/);
  assert.match(manifest, new RegExp(`rev = "${SPEC_SHA}"`));
  assert.match(manifest, /git = "https:\/\/github\.com\/soksak-ai\/soksak-spec\.git"/);
  const dependencies = manifest.match(/\[dependencies\]([\s\S]*?)(?:\n\[|$)/)?.[1] ?? "";
  assert.doesNotMatch(dependencies, /\b(?:path|branch)\s*=/);
});

test("the release matrix is the five supported desktop targets", () => {
  const matrix = JSON.parse(read("release/targets.json"));
  assert.deepEqual(matrix.map(({ target }) => target), TARGETS);
  assert.ok(matrix.every(({ runner }) => typeof runner === "string" && runner.length > 0));
});

test("the source tree contains neither symlink creation nor executable-relative asset guessing", () => {
  const source = fs.readdirSync(path.join(root, "src"), { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".rs"))
    .map((entry) => read(path.relative(root, path.join(entry.parentPath, entry.name))))
    .join("\n");
  assert.doesNotMatch(source, /(?<![A-Za-z0-9_])(?:std::os::[^\n]*::)?symlink\s*\(/);
  assert.doesNotMatch(source, /current_exe\s*\(/);
  assert.doesNotMatch(source, /while\s+let\s+Some\([^)]*\)\s*=\s*dir/);
});

test("first-party contract identifiers use the 0.0.1 baseline", () => {
  const files = [
    "src/doc_exec.rs",
    "src/wf_service.rs",
    "src/main.rs",
    "src/lib.rs",
    "workflows/draft.doc.json",
    "workflows/research.doc.json",
    "references/workflow/api-reference.md",
  ];
  const text = files.map(read).join("\n");
  assert.doesNotMatch(text, /\b(?:workflow-doc|soksak-spec-[a-z0-9-]+)@1\b/);
  assert.match(read("src/doc_exec.rs"), /pub const SPEC: &str = "workflow-doc@0\.0\.1"/);
  assert.match(read("src/wf_service.rs"), /soksak-spec-plugin-issue-board@0\.0\.1/);
  assert.match(read("src/wf_service.rs"), /soksak-spec-plugin-prompt-store@0\.0\.1/);
});

test("the directive loop requires a declared store and never polls or kills its lock owner", () => {
  const source = read("src/bin/directive-loop.rs");
  assert.doesNotMatch(source, /PathBuf::from\("ledger\.json"\)/);
  assert.doesNotMatch(source, /Command::new\("kill"\)/);
  assert.doesNotMatch(source, /for _ in 0\.\.100/);
  assert.match(source, /--store requires an absolute path/);
});

test("repository and workflow files are regular files only", () => {
  const walk = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      if ([".git", "target", "dist"].includes(entry.name)) continue;
      const child = path.join(directory, entry.name);
      assert.equal(fs.lstatSync(child).isSymbolicLink(), false, child);
      if (entry.isDirectory()) walk(child);
    }
  };
  walk(root);
});

test("GitHub Actions are SHA-pinned and never publish to a package registry", () => {
  const workflowDir = path.join(root, ".github", "workflows");
  const workflows = fs.readdirSync(workflowDir)
    .sort()
    .map((name) => fs.readFileSync(path.join(workflowDir, name), "utf8"))
    .join("\n");
  for (const line of workflows.split("\n").filter((line) => line.includes("uses:"))) {
    assert.match(line, /@[a-f0-9]{40}(?:\s|$)/);
  }
  assert.doesNotMatch(workflows, /cargo publish|npm publish|crates\.io/);
  const release = read(".github/workflows/release.yml");
  assert.match(release, /tags:\s*\["v\*"\]/);
  assert.match(release, /actions\/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1/);
  assert.match(release, /client-id:\s*\$\{\{ vars\.SOKSAK_RELEASE_APP_CLIENT_ID \}\}/);
  assert.match(release, /private-key:\s*\$\{\{ secrets\.SOKSAK_RELEASE_APP_PRIVATE_KEY \}\}/);
  assert.match(release, /permission-administration:\s*read/);
  assert.match(release, /permission-contents:\s*write/);
  assert.match(release, /SOKSAK_RELEASE_TOKEN:\s*\$\{\{ steps\.release-token\.outputs\.token \}\}/);
  assert.doesNotMatch(release, /GITHUB_TOKEN|secrets\.GITHUB_TOKEN/);
});
