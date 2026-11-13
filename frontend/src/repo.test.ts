// @vitest-environment node
import { readFileSync } from "node:fs";
import { expect, test } from "vitest";

test("the local compute feed copy is never committed or shipped from a prebuilt deploy", () => {
  const lines = readFileSync(new URL("../.gitignore", import.meta.url), "utf8").split("\n").map(l => l.trim());
  expect(lines).toContain("public/compute-feed/");
});
