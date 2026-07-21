import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(
  new URL("../src/updateSettings.ts", import.meta.url),
  "utf8",
);
const output = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const updateSettings = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString("base64")}`
);

test("update traffic follows the operating system proxy by default", () => {
  const storage = { getItem: () => null };
  assert.deepEqual(updateSettings.loadUpdateProxySettings(storage), {
    mode: "system",
  });
});

test("a manual update proxy is trimmed and normalized before it is persisted", () => {
  assert.deepEqual(
    updateSettings.normalizeUpdateProxySettings({
      mode: "manual",
      url: "  http://127.0.0.1:7897  ",
    }),
    { mode: "manual", url: "http://127.0.0.1:7897/" },
  );
});

test("manual proxy credentials are not persisted in webview storage", () => {
  assert.throws(
    () =>
      updateSettings.normalizeUpdateProxySettings({
        mode: "manual",
        url: "http://user:secret@127.0.0.1:7897",
      }),
    /用户名或密码/,
  );
});

test("corrupt persisted settings safely fall back to the system proxy", () => {
  const storage = { getItem: () => "not-json" };
  assert.deepEqual(updateSettings.loadUpdateProxySettings(storage), {
    mode: "system",
  });
});
