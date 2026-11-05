/**
 * Affirmative claim hits for built UI, PDF, and pricing surfaces.
 * Negated does-nots ("not a quantum-safe badge") are not hits.
 * Sales-ready: NO. Available: NO.
 */
import fs from "node:fs";
import path from "node:path";

// Negation must govern this claim. A disclaimer in another sentence, HTML
// element, source string or contrast clause cannot exempt affirmative copy.
function negatesClaim(prefix) {
  const clause = prefix.split(/[.!?;\n"<>]|\b(?:but|however|yet)\b/i).at(-1);
  return /\b(?:not|never|without|isn't|isnt|aren't|cannot|can't)\s+(?:(?:a|an|the|be|being|claiming|inventing|invents|issued|certified|certification|or|and)\s*[, ]*\s*)*$/i.test(clause)
    || /\bnot a certification,? warranty,? (?:audit opinion,? )?(?:or legal opinion|or proof) that any (?:system|vendor) is\s*$/i.test(clause);
}

const PATTERNS = [
  ["quantum-proof", /\bquantum[\s-]+proof\b/gi],
  ["quantum-safe-absolute", /\bquantum[\s-]+safe\b/gi],
  ["nist-certified", /\bnist[\s-]+certified\b/gi],
  ["cisa-endorsed", /\bcisa[\s-]+endorsed\b/gi],
  ["unhackable", /\bunhackable\b/gi],
];

export function claimHits(text) {
  const hits = new Set();
  const source = String(text ?? "")
    .replace(/[\u2010-\u2015]/g, "-")
    .replace(/&(?:nbsp|#160|#xA0);/gi, " ")
    .replace(/<\/?(?:strong|em|b|i|span)\b[^>]*>/gi, "");
  for (const [id, re] of PATTERNS) {
    re.lastIndex = 0;
    for (const match of source.matchAll(re)) {
      if (negatesClaim(source.slice(0, match.index ?? 0))) continue;
      hits.add(id);
    }
  }
  return [...hits];
}

const SURFACE_ROOTS = [
  "apps/orbit-web/src",
  "crates/orbit-application/src/pqc_report_issue.rs",
  "crates/orbit-application/src/pqc_scorecard.rs",
  "crates/orbit-application/src/pqc_pdf.rs",
  "crates/orbit-http/src/pqc_report_json.rs",
  "crates/orbit-http/src/pqc_report_pdf.rs",
  "docs/contracts/pqc-report-preview-v1.schema.json",
  "docs/contracts/pqc-report-issued-v1.schema.json",
  "docs/contracts/pqc-scorecard-preview-v1.schema.json",
  "docs/contracts/pqc-scorecard-trust-report-v1.schema.json",
  "docs/contracts/fixtures/pqc-preview",
  "e2e/staging/fixtures",
];

function surfaceOf(rel) {
  const file = rel.replaceAll("\\", "/");
  if (
    file.includes("pqc_buyer_pages.rs") ||
    file.includes("/pricing") ||
    file.endsWith("pricing.html")
  ) {
    return "pricing";
  }
  if (
    file.includes("pqc_report") ||
    file.includes("pqc_pdf") ||
    file.includes("pqc_scorecard") ||
    file.includes("pqc-report") ||
    file.includes("pqc-scorecard") ||
    file.includes("fixtures/pqc-preview") ||
    file.endsWith(".pdf")
  ) {
    return "pdf";
  }
  return "ui";
}

function skipFile(rel) {
  const file = rel.replaceAll("\\", "/");
  if (file.includes("/tests/")) return true;
  if (/(^|\/)[^/]*tests?\.rs$/i.test(file)) return true;
  return false;
}

function walk(abs, acc) {
  if (!fs.existsSync(abs)) return;
  const stat = fs.statSync(abs);
  if (stat.isFile()) {
    acc.push(abs);
    return;
  }
  for (const name of fs.readdirSync(abs)) {
    if (name === "target" || name === "node_modules") continue;
    walk(path.join(abs, name), acc);
  }
}

function allowlist(root) {
  const file = path.join(root, "scripts/fixtures/forbidden-claims-allowlist.txt");
  if (!fs.existsSync(file)) return [];
  return fs
    .readFileSync(file, "utf8")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"));
}

export function scanBuiltSurfaces(root) {
  const allowed = allowlist(root);
  const files = [];
  for (const rel of SURFACE_ROOTS) walk(path.join(root, rel), files);
  const findings = [];
  for (const abs of files) {
    const rel = path.relative(root, abs).replaceAll("\\", "/");
    if (skipFile(rel)) continue;
    if (!/\.(rs|html|json|md|txt)$/i.test(rel)) continue;
    if (allowed.some((entry) => rel.includes(entry))) continue;
    const text = fs.readFileSync(abs, "utf8");
    const hits = claimHits(text);
    if (!hits.length) continue;
    findings.push({ file: rel, surface: surfaceOf(rel), hits });
  }
  return findings;
}
