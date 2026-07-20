import { mkdirSync, readdirSync, unlinkSync } from "node:fs";

const directory = new URL("../src/generated/", import.meta.url);
mkdirSync(directory, { recursive: true });
for (const entry of readdirSync(directory, { withFileTypes: true })) {
  if (entry.isFile() && entry.name.endsWith(".ts")) {
    unlinkSync(new URL(entry.name, directory));
  }
}
