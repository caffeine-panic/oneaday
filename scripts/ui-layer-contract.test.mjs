import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
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
