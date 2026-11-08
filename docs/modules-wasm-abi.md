# ABI des modules `this` compilés (wasm) — v1

Contrat unique, partagé par quatre chantiers : le loader du framework (F1), le SDK invité (F2),
le service de build (F3) et l'app (F6). Toute divergence avec ce document est un bug du chantier,
pas du document : on modifie le document d'abord.

## 1. L'artefact

Un fichier `program.wasm`, produit d'un crate `cdylib` compilé pour `wasm32-unknown-unknown`.
Il est rangé dans le `VfsModuleStore` comme un *asset* du module (`assets/program.wasm`), donc signé,
vérifié par etag, immuable par version — rien de nouveau à ce niveau.

## 2. Exports (obligatoires, signature exacte)

| export | signature | rôle |
|---|---|---|
| `memory` | memory | seul canal de données |
| `alloc` | `(param i32) (result i32)` | réserve `n` octets, rend le décalage |
| `manifest` | `(result i64)` | rend `(ptr << 32) \| len` d'un JSON `ModuleDescriptor` |
| `handle` | `(param i32 i32 i32 i32) (result i64)` | `(op_ptr, op_len, req_ptr, req_len)` → `(resp_ptr << 32) \| resp_len` |

`(ptr << 32) | len` : couple empaqueté sur un `i64`, exactement comme `render` dans
`crates/this-module-harness/src/render.rs`.

## 3. Imports : **aucun** en v1

Un module est une **fonction pure de ce qu'on lui passe** — même règle que le contrat de vue.
Tout import déclaré est refusé **avant instanciation** (`ImportsRefused`), donc avant la première
instruction exécutée. Le mécanisme d'une table d'imports fermée existe dans le code (refus par
liste), mais la liste est vide en v1 : ce qui manque (lignes, temps, journal) est **dans la requête**.
Accès direct au stockage = v2, décision séparée.

## 4. Requête (`req`) — JSON UTF-8

```json
{
  "op": "create | get | list | update | delete | call",
  "name": "add_task",
  "entity": "task",
  "input": { },
  "rows": [ ],
  "ctx": { "now_ms": 1762000000000, "tenant": "…", "account": "…", "locale": "fr", "config": { } }
}
```

- `name` : seulement pour `op = "call"` (opération nommée déclarée dans le descripteur `operations`).
- `input` : charge utile de `create`/`update`, ligne demandée pour `get`.
- `rows` : pour `list` et `call`, les lignes **de la portée du tenant**, bornées par l'hôte
  (plafond `max_rows`, défaut 500). Ce que le module ne voit pas, il ne peut pas le valider :
  l'hôte décide de la portée, jamais le module.
- `ctx.now_ms` : le temps est une **donnée**, pas un import.

## 5. Réponse — JSON UTF-8

Succès :
```json
{ "ok": true, "output": { }, "effects": [ ] }
```
Échec (une valeur, jamais un panic ni un trap) :
```json
{ "ok": false, "error": { "code": "invalid_title", "message": "le titre est vide", "field": "title" } }
```

## 6. Effets (ensemble fermé)

| kind | forme | appliqué par l'hôte |
|---|---|---|
| `put` | `{"kind":"put","entity":"task","body":{ }}` | écrit la ligne (create si pas d'`id`, update sinon) |
| `delete` | `{"kind":"delete","entity":"task","id":"…"}` | supprime la ligne |
| `emit` | `{"kind":"emit","topic":"…","payload":{ }}` | publie un `EntityEvent` sur l'`EventBus` |
| `log` | `{"kind":"log","level":"info\|warn\|error","message":"…"}` | journalise |

Un `kind` inconnu est refusé (réponse entière rejetée, rien n'est appliqué) : un effet partiel
donnerait un état que ni le module ni l'hôte ne sait décrire.

## 7. Découpage des responsabilités

- **Le module** : valide, transforme, décide. Il porte la logique métier propre à l'entité.
- **L'hôte** : stocke, applique les effets **dans l'ordre**, remplit `id`/`created_at`/`updated_at`,
  publie les événements, et refuse tout ce qui sort du contrat.
- **Le descripteur** (rendu par `manifest()`) passe le même `check(Stored)` qu'un module stocké.
  Un module à code n'obtient aucun privilège : il ne peut pas élargir sa propre portée.

## 8. Bornes (par appel, imposées par l'hôte)

| borne | défaut | dépassement |
|---|---|---|
| fuel | 50 000 000 | `OutOfFuel`, jamais un blocage |
| mémoire | 64 pages (4 MiB) | `Memory`, refusé avant instanciation si déclaré au-delà |
| requête | 1 MiB | `RequestTooLarge` |
| réponse | 4 MiB | `ResponseTooLarge` |
| lignes passées | 500 | tronquées, signalé dans `ctx.truncated` |

## 9. Opérations nommées et interface

Une opération se déclare dans le descripteur :
```json
{ "operations": [ { "name": "add_task", "label": "Ajouter une tâche", "input": { }, "output": { } } ] }
```
Un bouton de vue (`Action::Invoke { operation: "add_task" }`) déclenche `handle(op:"call", name:"add_task")`.
C'est ce qui fait que **les interfaces évoluent avec le comportement** : le module rend ses vues
(`render`, contrat existant) et répond à ses opérations, dans le même artefact.

## 10. API du service de build (forge)

Base : `https://<hôte>/v1`. Auth : `Authorization: Bearer <jwt>` (sauf `auth/*` et `forge/key`).

| méthode | chemin | corps / réponse |
|---|---|---|
| POST | `/auth/signup` | `{invitation, email, password}` → `{token, refresh, account}` |
| POST | `/auth/login` | `{email, password}` → `{token, refresh, account}` |
| POST | `/auth/refresh` | `{refresh}` → `{token, refresh}` |
| POST | `/auth/logout` | — → 204 |
| GET | `/me` | → `{account:{id,email}, quota:{builds_today, builds_limit, cpu_seconds_left}}` |
| GET | `/forge/key` | → `{algorithm:"ed25519", public_key:"<hex>", fingerprint:"sha256:<hex>"}` |
| POST | `/builds` | `{name, version, files:[{path, content}], toolchain?}` → `{id, status}` |
| GET | `/builds/{id}` | → `{id, status, started_at, finished_at, diagnostics:[{file,line,column,level,message}], artifact:{sha256,size,signature}}` |
| GET | `/builds/{id}/events` | SSE : `event: status`, `event: diagnostic`, `event: artifact`, `event: done` |
| GET | `/builds/{id}/artifact` | `application/wasm`, en-têtes `X-Forge-Signature`, `X-Forge-Manifest` (JSON base64url) |

`status` ∈ `queued | building | failed | signed`. Un build est **borné** : temps maximal 180 s,
CPU 6, mémoire 4 Go, un build à la fois par défaut.

Charge signée (canonique, JSON à clés triées) :
```json
{"account":"<uuid>","built_at":"<iso8601>","forge_version":"0.1.0",
 "module":{"name":"…","version":"…"},"source_sha256":"…","toolchain":"rustc 1.93.1",
 "wasm_sha256":"…"}
```
L'appareil vérifie la signature contre la clé épinglée à l'appairage (`TrustAnchor` accepte
plusieurs clés) et refuse tout artefact non signé par une clé de la liste (`require_anchor_for_code`).
