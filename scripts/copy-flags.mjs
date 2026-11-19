// Copies the 4x3 country flag SVGs from the flag-icons package (MIT) into ui/flags.
import { cpSync, existsSync, mkdirSync, readdirSync, copyFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const src = join(root, "node_modules", "flag-icons", "flags", "4x3");
const dst = join(root, "ui", "flags");
if (!existsSync(src)) {
  console.error("flag-icons not installed; run pnpm install");
  process.exit(1);
}
mkdirSync(dst, { recursive: true });
let n = 0;
for (const f of readdirSync(src)) {
  if (f.endsWith(".svg")) {
    copyFileSync(join(src, f), join(dst, f));
    n++;
  }
}
cpSync(join(root, "node_modules", "flag-icons", "LICENSE"), join(dst, "LICENSE"));
console.log(`copied ${n} flags`);
