// Fails when a color could break a theme (ADR-0020). Run by CI.
//  1. App.css: color literals only in the token blocks at the top.
//  2. Every token in the light block has a dark value, and the dark block
//     defines nothing the light one doesn't.
//  3. No color literal in the UI's .ts/.tsx strings: canvas code reads tokens
//     with tok() / tokA() from theme.ts.
// Usage: node scripts/check-colors.mjs
import fs from "node:fs";
import path from "node:path";

const SRC = path.join(path.dirname(new URL(import.meta.url).pathname.replace(/^\/(\w:)/, "$1")), "..", "app", "src");
const LITERAL = /#[0-9a-fA-F]{3,8}\b|\b(rgba?|hsla?|oklch|oklab|lab|lch)\(/;
// Files allowed literals, and why.
const EXEMPT = {
  // Must still show if a blocked resource broke the stylesheet itself.
  "csp-watch.ts": "the security-policy alarm draws without the stylesheet",
  "theme.ts": "tokA() turns a token into rgba() for canvas",
};
const problems = [];

const css = fs.readFileSync(path.join(SRC, "App.css"), "utf8");
const block = (sel) => {
  const i = css.indexOf(`${sel} {`);
  if (i < 0) return null;
  const end = css.indexOf("\n}", i);
  return { start: i, end, body: css.slice(i, end) };
};
const light = block(":root");
const dark = block(':root[data-theme="dark"]');
if (!light || !dark) problems.push("App.css: the light :root block and the :root[data-theme=\"dark\"] block must both exist at the top");
else {
  const names = (b) => new Set([...b.body.matchAll(/^\s*(--[a-z0-9-]+)\s*:/gm)].map((m) => m[1]));
  const L = names(light), D = names(dark);
  for (const n of L) if (!D.has(n)) problems.push(`App.css: ${n} has no dark value`);
  for (const n of D) if (!L.has(n)) problems.push(`App.css: ${n} is set for dark only (define it in :root first)`);
  // Comments are prose; strip them before looking for literals.
  const after = css.slice(dark.end + 2).replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " "));
  after.split("\n").forEach((line, i) => {
    if (LITERAL.test(line)) {
      const lineNo = css.slice(0, dark.end + 2).split("\n").length + i;
      problems.push(`App.css:${lineNo}: color literal outside the token blocks: ${line.trim()}`);
    }
  });
}

for (const f of fs.readdirSync(SRC)) {
  if (!/\.(ts|tsx)$/.test(f) || EXEMPT[f]) continue;
  fs.readFileSync(path.join(SRC, f), "utf8").split("\n").forEach((line, i) => {
    const strings = line.match(/"[^"]*"|'[^']*'|`[^`]*`/g) ?? [];
    if (strings.some((s) => LITERAL.test(s))) problems.push(`${f}:${i + 1}: color literal (use a token): ${line.trim()}`);
  });
}

if (problems.length) {
  console.error(`Colors: ${problems.length} problem(s). Every color is a token in App.css (ADR-0020).`);
  for (const p of problems) console.error("  " + p);
  process.exit(1);
}
console.log("Colors: every color is a token with a light and a dark value.");
