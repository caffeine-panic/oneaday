import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(
  new URL("../src/panelLayout.ts", import.meta.url),
  "utf8",
);
const output = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const panelLayout = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString("base64")}`
);

test("connections and resources can be collapsed independently", () => {
  const connectionsCollapsed = panelLayout.togglePanel(
    panelLayout.DEFAULT_PANEL_LAYOUT,
    "connections",
  );
  assert.deepEqual(connectionsCollapsed, {
    connections: "collapsed",
    resources: "expanded",
  });

  assert.deepEqual(panelLayout.togglePanel(connectionsCollapsed, "resources"), {
    connections: "collapsed",
    resources: "collapsed",
  });
});

test("the last valid panel layout is restored on startup", () => {
  const storage = {
    getItem: () =>
      JSON.stringify({
        version: 1,
        layout: {
          connections: "collapsed",
          resources: "expanded",
        },
      }),
  };

  assert.deepEqual(panelLayout.loadPanelLayout(storage), {
    connections: "collapsed",
    resources: "expanded",
  });
});

test("panel layout changes are persisted without application data", () => {
  let savedKey;
  let savedValue;
  const storage = {
    setItem: (key, value) => {
      savedKey = key;
      savedValue = value;
    },
  };
  const layout = {
    connections: "expanded",
    resources: "collapsed",
  };

  assert.deepEqual(panelLayout.savePanelLayout(layout, storage), layout);
  assert.equal(savedKey, "atlas.panelLayout");
  assert.deepEqual(JSON.parse(savedValue), {
    version: 1,
    layout,
  });
});

test("invalid or unavailable storage falls back to both panels expanded", () => {
  assert.deepEqual(
    panelLayout.loadPanelLayout({
      getItem: () =>
        JSON.stringify({
          version: 1,
          layout: {
            connections: "hidden",
            resources: "collapsed",
          },
        }),
    }),
    panelLayout.DEFAULT_PANEL_LAYOUT,
  );
  assert.deepEqual(
    panelLayout.loadPanelLayout({
      getItem: () => {
        throw new Error("storage unavailable");
      },
    }),
    panelLayout.DEFAULT_PANEL_LAYOUT,
  );
});
