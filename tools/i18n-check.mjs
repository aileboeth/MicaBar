// node tools/i18n-check.mjs : lists translation keys used in the code that a language file lacks,
// and entries whose {N} placeholders differ from the key. No dependencies.
//   keys: tr("…") / trc("ctx", "…") in ui/*.js, t("…") / tr!("…") in src-tauri/src/*.rs, static titles in ui/*.html
import { readFileSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const CJK = /[　-〿一-鿿＀-￯]/;
const keys = new Set();
const add = (k) => { k = k.replace(/^　+/, ""); if (CJK.test(k)) keys.add(k); };
const unq = (s) => JSON.parse('"' + s.replace(/\\u\{([0-9a-fA-F]+)\}/g, (_, h) => "\\u" + h.padStart(4, "0")) + '"');
const STR = String.raw`"((?:[^"\\]|\\.)*)"`;

for (const f of ["app.js", "charts.js", "motion.js"]) {
  const src = readFileSync(join(root, "ui", f), "utf8");
  for (const m of src.matchAll(new RegExp(String.raw`\btr\(\s*` + STR, "g"))) add(unq(m[1]));
  for (const m of src.matchAll(new RegExp(String.raw`\btrc\(\s*"(\w+)",\s*` + STR, "g"))) { add(unq(m[2])); keys.add(m[1] + "|" + unq(m[2])); }
}
for (const f of ["index.html", "manager.html"]) {
  const src = readFileSync(join(root, "ui", f), "utf8");
  for (const m of src.matchAll(/(?:title|placeholder)="([^"]*)"/g)) add(m[1]);
  for (const m of src.matchAll(/data-do="layout-edit">([^<]*)</g)) add(m[1].trim());
}
const rs = join(root, "src-tauri", "src");
for (const f of readdirSync(rs).filter((n) => n.endsWith(".rs"))) {
  const src = readFileSync(join(rs, f), "utf8");
  for (const m of src.matchAll(new RegExp(String.raw`(?:crate::i18n::t|crate::tr!|\bt)\(\s*` + STR, "g"))) add(unq(m[1]));
  for (const m of src.matchAll(new RegExp(String.raw`step\("\w+", ` + STR + String.raw`\)`, "g"))) add(unq(m[1]));
  if (f === "startup.rs") for (const m of src.matchAll(new RegExp(String.raw`(?:label: |HKEY_\w+, )` + STR, "g"))) add(unq(m[1]));
  if (f === "iporg.rs") for (const m of src.matchAll(new RegExp(String.raw`Some\(` + STR + String.raw`\)`, "g"))) add(unq(m[1]));
}

const ph = (s) => [...s.matchAll(/\{(\d+|n)\}/g)].map((m) => m[1]).sort().join(",");
let bad = 0;
for (const file of readdirSync(join(root, "ui", "i18n")).filter((n) => n.endsWith(".js"))) {
  const src = readFileSync(join(root, "ui", "i18n", file), "utf8");
  const dict = JSON.parse(src.slice(src.indexOf("{"), src.lastIndexOf("}") + 1));
  const missing = [...keys].filter((k) => !(k in dict) && !(k.includes("|") && false));
  const ctxMissing = missing.filter((k) => /^[a-z]+\|/.test(k));
  const plain = missing.filter((k) => !/^[a-z]+\|/.test(k));
  const wrong = Object.entries(dict).filter(([k, v]) => ph(k) !== ph(v)).map(([k]) => k);
  const unused = Object.keys(dict).filter((k) => !keys.has(k));
  console.log(`${file.padEnd(10)} ${Object.keys(dict).length} entries · missing ${plain.length}${ctxMissing.length ? ` (+${ctxMissing.length} context)` : ""} · placeholder mismatch ${wrong.length} · unused ${unused.length}`);
  for (const k of plain.slice(0, 20)) console.log("   missing:", JSON.stringify(k));
  for (const k of wrong.slice(0, 10)) console.log("   placeholders:", JSON.stringify(k));
  bad += plain.length + wrong.length;
}
console.log(`${keys.size} keys in the code`);
process.exit(bad ? 1 : 0);
