import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(
  new URL("../src/profileSelection.ts", import.meta.url),
  "utf8",
);
const output = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const profileSelection = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString("base64")}`
);

test("reselecting the current open profile preserves its loaded resources", () => {
  assert.equal(
    profileSelection.planProfileSelection("etcd", "etcd", true),
    "preserve",
  );
});

test("switching to another open profile reloads its root resources", () => {
  assert.equal(
    profileSelection.planProfileSelection("etcd", "zookeeper", true),
    "reload",
  );
});

test("selecting a closed profile clears the previous connection view", () => {
  assert.equal(
    profileSelection.planProfileSelection("etcd", "nacos", false),
    "clear",
  );
});
