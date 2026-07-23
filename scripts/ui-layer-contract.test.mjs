import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
const appSource = readFileSync(
  new URL("../src/App.tsx", import.meta.url),
  "utf8",
);
const mainSource = readFileSync(
  new URL("../src/main.tsx", import.meta.url),
  "utf8",
);
const toastSource = readFileSync(
  new URL("../src/Toast.tsx", import.meta.url),
  "utf8",
);
const tauriConfig = JSON.parse(
  readFileSync(
    new URL("../src-tauri/tauri.conf.json", import.meta.url),
    "utf8",
  ),
);
const defaultCapability = JSON.parse(
  readFileSync(
    new URL("../src-tauri/capabilities/default.json", import.meta.url),
    "utf8",
  ),
);
const connectionAuthSource = readFileSync(
  new URL("../src/connectionAuth.ts", import.meta.url),
  "utf8",
);
const connectionAuthOutput = ts.transpileModule(connectionAuthSource, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const connectionAuth = await import(
  `data:text/javascript;base64,${Buffer.from(connectionAuthOutput).toString("base64")}`
);

function declarationBlock(selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = css.match(new RegExp(`${escaped}\\s*\\{([\\s\\S]*?)\\}`));
  assert.ok(match, `missing CSS rule for ${selector}`);
  return match[1];
}

function numericDeclaration(selector, property) {
  const match = declarationBlock(selector).match(
    new RegExp(`${property}\\s*:\\s*(\\d+)`),
  );
  assert.ok(match, `missing numeric ${property} for ${selector}`);
  return Number(match[1]);
}

function selectorsDeclaringZIndex(className) {
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter(
      ([, selectors, declarations]) =>
        selectors
          .split(",")
          .some((selector) => selector.trim().includes(className)) &&
        /z-index\s*:/.test(declarations),
    )
    .map(([, selectors]) => selectors.trim());
}

test("global result toast stays above the blurred modal backdrop", () => {
  const backdrop = declarationBlock(".dialog-backdrop");
  assert.match(backdrop, /backdrop-filter\s*:\s*blur\(/);
  assert.deepEqual(
    selectorsDeclaringZIndex(".toast"),
    [".toast"],
    "toast stacking must have one authoritative rule; later or more-specific overrides can hide it behind a modal",
  );
  assert.ok(
    numericDeclaration(".toast", "z-index") >
      numericDeclaration(".dialog-backdrop", "z-index"),
    "the toast is treated as background content and blurred while a dialog is open",
  );
});

test("macOS window chrome blends into a draggable application top bar", () => {
  const mainWindow = tauriConfig.app?.windows?.[0];

  assert.equal(mainWindow?.titleBarStyle, "Overlay");
  assert.equal(mainWindow?.hiddenTitle, true);
  assert.equal(mainWindow?.theme, "Dark");
  assert.equal(mainWindow?.backgroundColor, "#0d141d");
  assert.deepEqual(mainWindow?.trafficLightPosition, { x: 16, y: 20 });
  assert.match(
    appSource,
    /<header\s+className="topbar"\s+data-tauri-drag-region="deep">/,
  );
  assert.match(
    appSource,
    /<div\s+className="top-spacer"\s+data-tauri-drag-region\s*\/>/,
  );
  assert.match(
    mainSource,
    /isTauri\(\).*navigator\.userAgent\.includes\("Mac"\)/s,
  );
  assert.match(
    mainSource,
    /document\.documentElement\.dataset\.platform\s*=\s*"macos"/,
  );
  assert.match(
    css,
    /html\[data-platform="macos"\]\s+\.topbar\s*\{[^}]*padding-left:\s*82px/s,
  );
});

test("overlay window chrome authorizes native window dragging", () => {
  const mainWindow = tauriConfig.app?.windows?.[0];

  assert.equal(mainWindow?.titleBarStyle, "Overlay");
  assert.ok(
    defaultCapability.permissions.includes("core:window:allow-start-dragging"),
    "the overlay title bar cannot move the native window without the start_dragging permission",
  );
});

test("successful notices fade after four seconds and pause while interactive", () => {
  assert.match(toastSource, /toastAutoDismisses\(toast\.tone\)/);
  assert.match(
    toastSource,
    /globalThis\.setTimeout\([\s\S]*?remainingMs\.current,\s*\)/,
  );
  assert.match(toastSource, /onMouseEnter=\{\(\) => setHovered\(true\)\}/);
  assert.match(toastSource, /onMouseLeave=\{\(\) => setHovered\(false\)\}/);
  assert.match(toastSource, /onFocus=\{\(\) => setFocused\(true\)\}/);
  assert.match(toastSource, /onBlur=/);
  assert.match(
    css,
    /\.toast--auto-dismiss\s*\{[^}]*animation:\s*toast-dismiss\s+4s/s,
  );
  assert.match(
    css,
    /\.toast--auto-dismiss:hover,[\s\S]*\.toast--auto-dismiss:focus-within\s*\{[^}]*animation-play-state:\s*paused/s,
  );
});

test("connection success uses the typed dismissing notice path", () => {
  assert.match(appSource, /showSuccess\("连接已断开"\)/);
  assert.match(appSource, /<Toast\s+key=\{toast\.id\}/);
  assert.doesNotMatch(appSource, /<button\s+className="toast"/);
});

test("every application notice declares its lifecycle explicitly", () => {
  assert.doesNotMatch(appSource, /\bsetMessage\(/);
  assert.match(appSource, /showSuccess\(`已连接 \$\{session\.endpoint\}`\)/);
  assert.match(appSource, /showError\(reason\)/);
  assert.match(
    appSource,
    /showWarning\(`变更已成功，但刷新失败：\$\{errorMessage\(reason\)\}`\)/,
  );
});

test("Nacos connections offer an explicit MSE AccessKey authentication mode", () => {
  assert.deepEqual(connectionAuth.authModes("nacos"), [
    "none",
    "usernamePassword",
    "mseAccessKey",
    "custom",
  ]);
  assert.equal(connectionAuth.authLabels.mseAccessKey, "阿里云 MSE AccessKey");
  assert.equal(
    connectionAuth.credentialIdentityLabel("mseAccessKey"),
    "AccessKey ID",
  );
  assert.equal(
    connectionAuth.credentialSecretLabel("mseAccessKey"),
    "AccessKey Secret",
  );
});

test("both navigation panels can be collapsed and restored accessibly", () => {
  assert.match(appSource, /data-connections=\{panelLayout\.connections\}/);
  assert.match(appSource, /data-resources=\{panelLayout\.resources\}/);
  assert.match(
    appSource,
    /aria-controls="connections-panel-content"[\s\S]*?aria-expanded=\{connectionsExpanded\}/,
  );
  assert.match(
    appSource,
    /aria-controls="resources-panel-content"[\s\S]*?aria-expanded=\{resourcesExpanded\}/,
  );
  assert.match(
    appSource,
    /id="connections-panel-content"[\s\S]*?hidden=\{!connectionsExpanded\}/,
  );
  assert.match(
    appSource,
    /id="resources-panel-content"[\s\S]*?hidden=\{!resourcesExpanded\}/,
  );
  assert.match(
    css,
    /\.shell\[data-connections="collapsed"\]\s*\{[^}]*--connections-width:\s*42px/s,
  );
  assert.match(
    css,
    /\.shell\[data-resources="collapsed"\]\s*\{[^}]*--resources-width:\s*42px/s,
  );
});
