import { spawnSync } from "node:child_process";

const result = spawnSync(
  "git",
  ["status", "--porcelain", "--untracked-files=all", "--", "src/generated"],
  { encoding: "utf8" },
);
if (result.error) throw result.error;
if (result.status !== 0) {
  process.stderr.write(result.stderr);
  process.exit(result.status ?? 1);
}
if (result.stdout.trim()) {
  process.stderr.write("Generated TypeScript contracts are stale:\n");
  process.stderr.write(result.stdout);
  process.exit(1);
}
