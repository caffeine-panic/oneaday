import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(new URL("../src/resourceTree.ts", import.meta.url), "utf8");
const output = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ES2022, target: ts.ScriptTarget.ES2022 },
}).outputText;
const tree = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString("base64")}`
);

const root = { type: "root" };
const parent = {
  kind: "resource",
  node: { address: { type: "zookeeper", path: "/apps" }, name: "apps", readable: true, hasChildren: true },
  depth: 0,
  expanded: false,
};

test("expanding and collapsing a resource keeps descendants local to their parent", () => {
  const expanded = tree.expandResourceRow([parent], 0, {
    parent: parent.node.address,
    items: [
      { address: { type: "zookeeper", path: "/apps/api" }, name: "api", readable: true, hasChildren: false },
    ],
  });

  assert.equal(expanded.length, 2);
  assert.equal(expanded[0].expanded, true);
  assert.equal(expanded[1].depth, 1);
  assert.deepEqual(
    tree.collapseResourceRow(expanded, 0, parent.node.address),
    [{ ...parent, expanded: false }],
  );
});

test("a continuation row is replaced atomically by the next page", () => {
  const rows = tree.pageRows([], 0, root, "next");
  const next = tree.replaceContinuationRow(rows, 0, tree.pageRows([
    { address: { type: "etcd", keyBase64: "YQ==" }, name: "a", readable: true, hasChildren: false },
  ], 0, root), rows[0]);

  assert.equal(next.length, 1);
  assert.equal(next[0].kind, "resource");
  assert.equal(next[0].node.name, "a");
});

test("stale async pages cannot mutate a different row now occupying the index", () => {
  const replacementParent = {
    ...parent,
    node: { ...parent.node, address: { type: "zookeeper", path: "/other" }, name: "other" },
  };
  const stalePage = {
    parent: parent.node.address,
    items: [{ address: { type: "zookeeper", path: "/apps/api" }, name: "api", readable: true, hasChildren: false }],
  };

  assert.deepEqual(tree.expandResourceRow([replacementParent], 0, stalePage), [replacementParent]);

  const currentMore = tree.pageRows([], 0, root, "new-cursor")[0];
  const staleMore = { ...currentMore, cursor: "old-cursor" };
  assert.deepEqual(
    tree.replaceContinuationRow([currentMore], 0, tree.pageRows([], 0, root), staleMore),
    [currentMore],
  );
});
