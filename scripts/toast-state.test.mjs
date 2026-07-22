import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const toastSource = readFileSync(
  new URL("../src/toastState.ts", import.meta.url),
  "utf8",
);
const toastOutput = ts.transpileModule(toastSource, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const toastState = await import(
  `data:text/javascript;base64,${Buffer.from(toastOutput).toString("base64")}`
);

test("successful and informational notices dismiss themselves", () => {
  assert.equal(toastState.toastAutoDismisses("success"), true);
  assert.equal(toastState.toastAutoDismisses("info"), true);
});

test("warnings and errors remain actionable alerts", () => {
  assert.equal(toastState.toastAutoDismisses("warning"), false);
  assert.equal(toastState.toastAutoDismisses("error"), false);
  assert.equal(toastState.toastRole("warning"), "alert");
  assert.equal(toastState.toastRole("error"), "alert");
});

test("a replacement notice receives a fresh lifecycle", () => {
  const first = toastState.nextToast(undefined, "已连接", "success");
  const replacement = toastState.nextToast(first, "保存成功", "success");

  assert.deepEqual(first, { id: 1, text: "已连接", tone: "success" });
  assert.deepEqual(replacement, {
    id: 2,
    text: "保存成功",
    tone: "success",
  });
});
