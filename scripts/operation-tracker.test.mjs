import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(
  new URL("../src/operationTracker.ts", import.meta.url),
  "utf8",
);
const output = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { OperationTracker } = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString("base64")}`
);

test("operations are tracked independently by workflow scope", () => {
  const ids = ["main-1", "history-1"];
  const tracker = new OperationTracker(() => ids.shift());

  const main = tracker.start("main");
  const history = tracker.start("serverHistory");

  assert.equal(main, "main-1");
  assert.equal(history, "history-1");
  assert.equal(tracker.current("main"), "main-1");
  assert.equal(tracker.current("serverHistory"), "history-1");
});

test("replacing an operation makes the older result stale", () => {
  const ids = ["old", "new"];
  const tracker = new OperationTracker(() => ids.shift());

  const old = tracker.start("search");
  const current = tracker.start("search");

  assert.equal(tracker.isCurrent("search", old), false);
  assert.equal(tracker.isCurrent("search", current), true);
  assert.equal(tracker.finish("search", old), false);
  assert.equal(tracker.current("search"), current);
  assert.equal(tracker.finish("search", current), true);
  assert.equal(tracker.current("search"), undefined);
});

test("cancelling a workflow invalidates its result before remote cancellation completes", () => {
  const tracker = new OperationTracker(() => "pending");
  const operationId = tracker.start("main");
  assert.equal(tracker.invalidate("main"), operationId);
  assert.equal(tracker.isCurrent("main", operationId), false);
  assert.equal(tracker.current("main"), undefined);
});
