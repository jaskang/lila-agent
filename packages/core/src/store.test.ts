import { expect, test } from "bun:test";
import { mkdtempSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Store } from "./store.ts";

test("backs up a corrupted state.json and falls back to an empty store", () => {
  const dir = mkdtempSync(join(tmpdir(), "lila-store-"));
  writeFileSync(join(dir, "state.json"), "{broken json");
  const store = new Store(dir);
  expect(store.projects).toEqual([]);
  expect(readdirSync(dir).some((name) => name.startsWith("state.json.corrupt."))).toBe(true);
});
