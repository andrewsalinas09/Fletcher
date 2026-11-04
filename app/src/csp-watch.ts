// Anything the content security policy blocks is shown, never silent (TB-60).
// Imported first in main.tsx, so it listens before anything else runs. The bar
// is plain DOM, not React: it still appears if the block broke React. The
// policy applies in `tauri dev` too (devCsp), so a new feature that needs it
// loosened shows this bar the first time it runs, long before a release.
import { invoke } from "@tauri-apps/api/core";

let bar: HTMLDivElement | null = null;
const seen = new Set<string>();

document.addEventListener("securitypolicyviolation", (e) => {
  const what = `${e.effectiveDirective} blocked ${e.blockedURI || "inline code"}${
    e.sourceFile ? ` (${e.sourceFile}:${e.lineNumber})` : ""
  }`;
  if (seen.has(what)) return;
  seen.add(what);
  console.error(`Fletcher: security policy ${what}`);
  invoke("csp_violation", { report: `${what} sample=${e.sample || "-"}` }).catch(() => {});
  if (!bar) {
    bar = document.createElement("div");
    bar.style.cssText =
      "position:fixed;left:0;right:0;bottom:0;z-index:2147483647;padding:6px 12px;" +
      "background:#b3261e;color:#fff;font:12px/1.4 monospace;white-space:pre-wrap;cursor:pointer";
    bar.title = "click to hide";
    bar.onclick = () => bar?.remove();
    bar.textContent =
      "Something in this window was blocked by Fletcher's security policy and may not work. " +
      "Please report it (Settings › SETUP › export… includes the details).";
    (document.body ?? document.documentElement).appendChild(bar);
  }
  bar.textContent += `\n• ${what}`;
});

// Proof that the watcher works: a build made with VITE_CSP_SELFTEST=1 makes
// one request the policy must block. Normal builds drop this line.
if (import.meta.env.VITE_CSP_SELFTEST) fetch("https://example.com/csp-selftest").catch(() => {});
