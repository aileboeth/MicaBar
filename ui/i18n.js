// Interface language. The Chinese text in the code is the key; ui/i18n/<lang>.js holds the
// translations (the backend reads the same files). The language is resolved by the backend
// (setting general.lang: auto = the Windows display language) and cached in localStorage so it
// is known before the first line of app.js runs; when it changes, the windows reload.
(function () {
  const LANGS = ["zh-CN", "zh-TW", "en", "ja", "ko", "ru", "es", "pt-BR", "fr", "de"];
  function pick(tag) {
    const t = String(tag || "").toLowerCase();
    if (/^zh-(tw|hk|mo)|^zh-hant/.test(t)) return "zh-TW";
    if (t.startsWith("zh")) return "zh-CN";
    if (t.startsWith("pt")) return "pt-BR";
    const base = t.split("-")[0];
    return LANGS.find((l) => l === base) || null;
  }
  let lang = null;
  try { lang = localStorage.getItem("mb-lang"); } catch { /* ignore */ }
  if (!LANGS.includes(lang)) lang = (navigator.languages || [navigator.language]).map(pick).find(Boolean) || "en";
  const dict = {};
  window.I18N = {
    LANGS,
    lang,
    pick,
    add(code, d) { if (code === lang) Object.assign(dict, d); },
    has(key) { return key in dict; },
    // the backend resolved another language: remember it and reload this window
    sync(code) {
      if (!LANGS.includes(code) || code === lang) return false;
      try { localStorage.setItem("mb-lang", code); } catch { /* ignore */ }
      location.reload();
      return true;
    },
  };
  // tr("{0} 个 IP", n): translation (or the key itself), {N} filled from the arguments.
  // Leading ideographic spaces (sub-option indent in the settings) are kept as they are.
  window.tr = function (key, ...args) {
    const m = /^　+/.exec(key);
    const pre = m ? m[0] : "";
    const k = pre ? key.slice(pre.length) : key;
    let s = dict[k] ?? k;
    if (args.length) s = s.replace(/\{(\d+)\}/g, (_, i) => String(args[+i]));
    return pre + s;
  };
  // the same Chinese word in another place (a pill unit vs a tab name): "ctx|key" first
  window.trc = function (ctx, key, ...args) {
    const k = ctx + "|" + key;
    return k in dict ? window.tr(k, ...args) : window.tr(key, ...args);
  };
  document.documentElement.lang = lang;
  if (lang !== "zh-CN") document.write(`<script src="i18n/${lang}.js"><\/script>`);
})();
