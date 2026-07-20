import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

test("desktop updater is signed and served by the repository release channel", () => {
  const config = JSON.parse(read("src-tauri/tauri.conf.json"));
  const updater = config.plugins?.updater;

  assert.equal(config.bundle?.createUpdaterArtifacts, true);
  assert.deepEqual(updater?.endpoints, [
    "https://github.com/caffeine-panic/oneaday/releases/latest/download/latest.json",
  ]);
  assert.ok(
    typeof updater?.pubkey === "string"
      && updater.pubkey.length > 80
      && !updater.pubkey.includes("PLACEHOLDER"),
    "a real updater public key must be embedded in the application",
  );
});

test("release workflow requires the private updater key without committing it", () => {
  const workflow = read(".github/workflows/release.yml");
  const qualityWorkflow = read(".github/workflows/quality.yml");
  const ignore = read(".gitignore");

  assert.match(workflow, /TAURI_SIGNING_PRIVATE_KEY:\s*\$\{\{ secrets\.TAURI_SIGNING_PRIVATE_KEY \}\}/);
  assert.match(workflow, /TAURI_SIGNING_PRIVATE_KEY_PASSWORD:\s*\$\{\{ secrets\.TAURI_SIGNING_PRIVATE_KEY_PASSWORD \}\}/);
  assert.match(workflow, /test -n "\$TAURI_SIGNING_PRIVATE_KEY"/);
  assert.match(workflow, /test -n "\$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"/);
  assert.match(workflow, /name: Configure macOS signing/);
  assert.match(workflow, /if \[ -z "\$APPLE_CERTIFICATE" \]; then/);
  assert.match(workflow, /APPLE_SIGNING_IDENTITY=-/);
  assert.match(workflow, /includeUpdaterJson:\s*true/);
  assert.doesNotMatch(workflow, /uploadUpdaterJson|uploadUpdaterSignatures/);

  const jobEnvironment = workflow.match(/timeout-minutes:[\s\S]*?strategy:/)?.[0] ?? "";
  assert.doesNotMatch(jobEnvironment, /APPLE_/);

  const actionStep = workflow.slice(workflow.indexOf("- uses: tauri-apps/tauri-action"));
  assert.doesNotMatch(actionStep, /APPLE_CERTIFICATE|APPLE_ID:|APPLE_PASSWORD:|APPLE_TEAM_ID:/);
  assert.match(qualityWorkflow, /"createUpdaterArtifacts":false/g);
  assert.match(ignore, /^\.codex\/$/m);
  assert.match(ignore, /^\*\.key$/m);
});

test("update operations stay behind the audited Rust command surface", () => {
  const backend = read("src-tauri/src/lib.rs");
  const build = read("src-tauri/build.rs");
  const frontend = read("src/registry.ts");
  const capability = JSON.parse(read("src-tauri/capabilities/default.json"));

  assert.match(backend, /tauri_plugin_updater::Builder::new\(\)\.build\(\)/);
  assert.match(backend, /check_for_app_update/);
  assert.match(backend, /install_app_update/);
  assert.match(build, /"check_for_app_update"/);
  assert.match(build, /"install_app_update"/);
  assert.match(frontend, /invoke<AppUpdateInfo \| null>\("check_for_app_update"/);
  assert.match(frontend, /invoke<void>\("install_app_update"/);
  assert.ok(capability.permissions.includes("allow-check-for-app-update"));
  assert.ok(capability.permissions.includes("allow-install-app-update"));
  assert.ok(!capability.permissions.includes("updater:default"));
});

test("update checks and downloads share the user-selected proxy policy", () => {
  const manifest = read("src-tauri/Cargo.toml");
  const backend = read("src-tauri/src/updates.rs");
  const frontend = read("src/registry.ts");

  assert.match(manifest, /reqwest[^\n]*version\s*=\s*"0\.13"[^\n]*"system-proxy"/);
  assert.match(backend, /UpdateProxySettings/);
  assert.match(backend, /\.proxy\(proxy\)/);
  assert.match(backend, /\.no_proxy\(\)/);
  assert.match(frontend, /invoke<AppUpdateInfo \| null>\("check_for_app_update", \{ proxySettings \}\)/);
});
