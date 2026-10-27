// Print the CHANGELOG.md section for a version; it becomes the release body and the
// in-app "what's new" (Settings › UPDATES).
//   node scripts/release-notes.mjs 0.2.0
// Fails when the section is missing or empty, so a release never ships without notes.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const version = (process.argv[2] ?? "").replace(/^v/, "");
const text = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "CHANGELOG.md"), "utf8");
const section = text.split(/\r?\n(?=## )/).find((s) => s.startsWith(`## ${version}`));
const notes = section?.split(/\r?\n/).slice(1).join("\n").trim() ?? "";
if (!notes || /^-\s*$/.test(notes)) {
  console.error(`CHANGELOG.md has no notes under "## ${version}".`);
  process.exit(1);
}
process.stdout.write(notes + "\n");
