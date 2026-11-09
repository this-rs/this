// Contrôle d'un module wasm contre l'ABI v1 (docs/modules-wasm-abi.md), sans la forge.
// Usage : node verify.mjs <module.wasm> [entity]   (code de sortie 1 au moindre écart)
import { readFileSync } from "node:fs";

const path = process.argv[2];
if (!path) { console.error("usage: node verify.mjs <module.wasm>"); process.exit(2); }
const bytes = readFileSync(path);
const mod = await WebAssembly.compile(bytes);
const imports = WebAssembly.Module.imports(mod).map((i) => `${i.module}.${i.name}`);
const exports = WebAssembly.Module.exports(mod);
const kinds = Object.fromEntries(exports.map((e) => [e.name, e.kind]));
console.log("imports:", JSON.stringify(imports));
console.log("exports:", JSON.stringify(exports.map((e) => `${e.name}:${e.kind}`)));

let bad = 0;
const check = (ok, msg) => { if (!ok) { console.log("ECART:", msg); bad++; } };
check(imports.length === 0, "des imports sont déclarés (ABI v1 : aucun)");
check(kinds.memory === "memory", "export memory manquant");
for (const f of ["alloc", "manifest", "handle"]) check(kinds[f] === "function", `export ${f} manquant`);

const inst = await WebAssembly.instantiate(mod, {});
const x = inst.exports;
check(x.alloc.length === 1 && x.manifest.length === 0 && x.handle.length === 4, "arités inattendues");

const enc = new TextEncoder(), dec = new TextDecoder();
const unpack = (r) => { const v = BigInt.asUintN(64, r); return [Number(v >> 32n), Number(v & 0xffffffffn)]; };
const read = (r) => { const [p, l] = unpack(r); return dec.decode(new Uint8Array(x.memory.buffer, p, l)); };
const put = (s) => { const b = enc.encode(s); const p = x.alloc(b.length); new Uint8Array(x.memory.buffer, p, b.length).set(b); return [p, b.length]; };
const call = (op, req) => { const [a, al] = put(op), [b, bl] = put(JSON.stringify(req)); return read(x.handle(a, al, b, bl)); };

const manifest = read(x.manifest());
console.log("manifest():", manifest);
JSON.parse(manifest);

const ctx = { now_ms: 1762000000000, tenant: "t", account: "u", locale: "fr", config: {} };
const ligne = { id: "11111111-1111-1111-1111-111111111111", titre: "Écrire", fait: false, priorite: 2 };
const cases = [
  ["create (titre valide)", "create", { op: "create", entity: "tache", input: { titre: "  Écrire le SDK ", priorite: 2 }, rows: [], ctx }],
  ["create (titre vide)", "create", { op: "create", entity: "tache", input: { titre: "   " }, rows: [], ctx }],
  ["call terminer", "call", { op: "call", name: "terminer", entity: "tache", input: { id: ligne.id }, rows: [ligne], ctx }],
  ["list (masquer_faites, tri=titre)", "list", { op: "list", entity: "tache", input: {}, rows: [ligne, { ...ligne, id: "2", titre: "A", fait: true }], ctx: { ...ctx, config: { masquer_faites: true, tri: "titre" } } }],
  ["requête illisible (doit rendre une valeur)", "create", "pas du json"],
];
for (const [label, op, req] of cases) {
  const raw = typeof req === "string" ? (() => { const [a, al] = put(op), [b, bl] = put(req); return read(x.handle(a, al, b, bl)); })() : call(op, req);
  console.log(`handle ${label}:`, raw);
  const r = JSON.parse(raw);
  check(typeof r.ok === "boolean", `${label} : pas de champ ok`);
}
// Durée de vie : deux appels successifs, la première réponse lue avant le second reste intacte.
const a = call("create", cases[0][2]); const b = call("create", cases[1][2]);
check(a !== b && JSON.parse(a).ok === true && JSON.parse(b).ok === false, "réponses successives incohérentes");
console.log(bad === 0 ? "OK : conforme à l'ABI v1" : `${bad} écart(s)`);
process.exit(bad === 0 ? 0 : 1);
