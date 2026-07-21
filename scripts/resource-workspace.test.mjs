import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(
  new URL("../src/resourceWorkspaceState.ts", import.meta.url),
  "utf8",
);
const output = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const workspace = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString("base64")}`
);

test("showing a document keeps its editable draft synchronized", () => {
  const document = {
    address: { type: "zookeeper", path: "/apps" },
    name: "apps",
    value: { content: "enabled=true", encoding: "utf8", sizeBytes: 12 },
    metadata: {},
  };
  const state = workspace.reduceResourceWorkspace(
    workspace.initialResourceWorkspaceState,
    { type: "document", document },
  );
  assert.equal(state.document, document);
  assert.equal(state.draftValue, "enabled=true");
});

test("clearing a connection view removes remote state but preserves user filters", () => {
  const state = {
    ...workspace.initialResourceWorkspaceState,
    rows: [
      { kind: "more", parent: { type: "root" }, cursor: "next", depth: 0 },
    ],
    draftValue: "dirty",
    selectedAddress: { type: "zookeeper", path: "/apps" },
    activeSearch: {
      scope: { type: "root" },
      query: "api",
      scanned: 10,
      exhaustive: true,
    },
    filter: "local",
    resourceQuery: "remote",
  };
  const cleared = workspace.reduceResourceWorkspace(state, {
    type: "clearView",
  });
  assert.deepEqual(cleared.rows, []);
  assert.equal(cleared.draftValue, "");
  assert.equal(cleared.selectedAddress, undefined);
  assert.equal(cleared.activeSearch, undefined);
  assert.equal(cleared.filter, "local");
  assert.equal(cleared.resourceQuery, "remote");
});
