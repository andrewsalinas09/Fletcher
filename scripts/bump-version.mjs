// Set Fletcher's version everywhere it lives, and open a CHANGELOG section for it.
//   node scripts/bump-version.mjs 0.2.0
// The release workflow refuses a tag that doesn't match tauri.conf.json (ADR-0015).
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const version = process.argv[2];
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/.test(version ?? "")) {
  console.error("Usage: node scripts/bump-version.mjs <major.minor.patch>");
  process.exit(1);
}
const edit = (file, change) => {
  const path = join(root, file);
  const before = readFileSync(path, "utf8");
  const after = change(before);
  if (after === before) console.log(`unchanged  ${file}`);
  else {
    writeFileSync(path, after);
    console.log(`updated    ${file}`);
  }
};
const once = (text, pattern, replacement, file) => {
  if (!pattern.test(text)) throw new Error(`No version field found in ${file}`);
  return text.replace(pattern, replacement);
};
edit("app/package.json", (t) => once(t, /("version":\s*")[^"]+(")/, `$1${version}$2`, "package.json"));
// The root package appears twice at the top of the lockfile.
edit("app/package-lock.json", (t) => t.replace(/("name":\s*"app",\s*"version":\s*")[^"]+(")/g, `$1${version}$2`));
edit("app/src-tauri/tauri.conf.json", (t) => once(t, /("version":\s*")[^"]+(")/, `$1${version}$2`, "tauri.conf.json"));
edit("app/src-tauri/Cargo.toml", (t) =>
  once(t, /(\[package\][^[]*?\nversion\s*=\s*")[^"]+(")/, `$1${version}$2`, "app Cargo.toml"),
);
edit("Cargo.toml", (t) =>
  once(t, /(\[workspace\.package\][^[]*?\nversion\s*=\s*")[^"]+(")/, `$1${version}$2`, "workspace Cargo.toml"),
);
edit("Cargo.lock", (t) => t.replace(/(name = "fletcher-(?:app|core)"\r?\nversion = ")[^"]+(")/g, `$1${version}$2`));

const changelog = join(root, "CHANGELOG.md");
const heading = `## ${version}`;
const existing = existsSync(changelog) ? readFileSync(changelog, "utf8") : "# Changelog\n";
if (!existing.includes(heading)) {
  const [head, ...rest] = existing.split(/\r?\n(?=## )/);
  writeFileSync(changelog, `${head.trimEnd()}\n\n${heading}\n\n- \n\n${rest.join("\n")}`.trimEnd() + "\n");
  console.log(`added      CHANGELOG.md section ${heading} — fill it in before tagging`);
}
console.log(`\nNext: edit CHANGELOG.md, commit, then: git tag v${version} && git push origin main v${version}`);
