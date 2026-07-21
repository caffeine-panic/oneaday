import type { ResourceAddress, ResourceNode, ResourcePage } from "./registry";

export type ResourceRow = {
  kind: "resource";
  node: ResourceNode;
  depth: number;
  expanded: boolean;
};

export type MoreRow = {
  kind: "more";
  parent: ResourceAddress;
  cursor: string;
  depth: number;
  search?: {
    scope: ResourceAddress;
    query: string;
  };
};

export type TreeRow = ResourceRow | MoreRow;

export function pageRows(
  items: ResourceNode[],
  depth: number,
  parent: ResourceAddress,
  nextCursor?: string,
): TreeRow[] {
  const rows: TreeRow[] = items.map((node) => ({
    kind: "resource",
    node,
    depth,
    expanded: false,
  }));
  if (nextCursor)
    rows.push({ kind: "more", parent, cursor: nextCursor, depth });
  return rows;
}

export function searchPageRows(
  items: ResourceNode[],
  scope: ResourceAddress,
  query: string,
  nextCursor?: string,
): TreeRow[] {
  const rows = pageRows(items, 0, scope, nextCursor);
  return rows.map((row) =>
    row.kind === "more" ? { ...row, search: { scope, query } } : row,
  );
}

function sameAddress(left: ResourceAddress, right: ResourceAddress): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

export function collapseResourceRow(
  rows: TreeRow[],
  index: number,
  expectedAddress: ResourceAddress,
): TreeRow[] {
  const row = rows[index];
  if (
    !row ||
    row.kind !== "resource" ||
    !sameAddress(row.node.address, expectedAddress)
  )
    return rows;
  const next = [...rows];
  next[index] = { ...row, expanded: false };
  let end = index + 1;
  while (end < next.length && next[end].depth > row.depth) end += 1;
  next.splice(index + 1, end - index - 1);
  return next;
}

export function expandResourceRow(
  rows: TreeRow[],
  index: number,
  page: ResourcePage,
): TreeRow[] {
  const row = rows[index];
  if (
    !row ||
    row.kind !== "resource" ||
    !sameAddress(row.node.address, page.parent)
  )
    return rows;
  const next = [...rows];
  next[index] = {
    ...row,
    expanded: page.items.length > 0,
    node: {
      ...row.node,
      hasChildren: page.items.length > 0 || Boolean(page.nextCursor),
    },
  };
  next.splice(
    index + 1,
    0,
    ...pageRows(page.items, row.depth + 1, page.parent, page.nextCursor),
  );
  return next;
}

export function replaceContinuationRow(
  rows: TreeRow[],
  index: number,
  replacement: TreeRow[],
  expected: MoreRow,
): TreeRow[] {
  const row = rows[index];
  if (
    !row ||
    row.kind !== "more" ||
    JSON.stringify(row) !== JSON.stringify(expected)
  )
    return rows;
  const next = [...rows];
  next.splice(index, 1, ...replacement);
  return next;
}
