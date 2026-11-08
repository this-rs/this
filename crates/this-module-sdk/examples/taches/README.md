# Écrire un module `this` en Rust (wasm, ABI v1)

Un module est une **fonction pure** : il reçoit une requête (opération, entrée, lignes, contexte) et rend
une réponse (sortie + effets). Il n'a ni disque, ni horloge, ni réseau : tout ce qu'il lit est dans la
requête, tout ce qu'il décide sort en effets que l'hôte applique. Contrat : `docs/modules-wasm-abi.md`.

## 1. Le crate

```toml
[lib]
crate-type = ["cdylib"]
[dependencies]
this-module-sdk = "0.0.1"   # seules dépendances admises : serde, serde_json (via le SDK)
```
Interdits : proc-macro, `build.rs`, tokio, `std::fs`/`std::net`/`std::time`.

## 2. Le module (`src/lib.rs`)

```rust
use this_module_sdk::{json, Effect, Entity, Field, Manifest, Module, Op, Request, Response, Transform};

pub struct Taches;
impl Module for Taches {
    fn manifest() -> Manifest {
        Manifest::new("taches", "0.1.0")                       // nom ^[a-z][a-z0-9_]*$, semver
            .entity(Entity::new("tache", "taches")             // singulier, pluriel EXPLICITE
                .field(Field::string("titre").transform(Transform::trim()))
                .field(Field::int("priorite").default(json!(0))))
    }
    fn handle(req: Request) -> Response {
        match req.op {
            Op::Create => {
                let t = req.input["titre"].as_str().unwrap_or("").trim();
                if t.is_empty() {
                    return Response::field_err("titre_vide", "le titre est vide", "titre");
                }
                let body = json!({"titre": t});
                Response::ok(body.clone()).effect(Effect::put("tache", body))
            }
            _ => Response::err("non_gere", "opération non gérée"),
        }
    }
}
this_module_sdk::export!(Taches);   // génère alloc / manifest / handle ; `memory` vient de la cible
```

## 3. Règles

* **`handle` est total** : toute entrée invalide devient `Response::err(code, message)`. Un `panic!`
  est un trap, donc un appel perdu. Pas de `unwrap()` sur ce qui vient de la requête.
* **Aucune persistance** : un `Effect::put` crée (sans `id`) ou modifie (avec `id`) ; `delete`,
  `emit`, `log` complètent l'ensemble, **fermé**. L'hôte applique dans l'ordre et remplit `id`,
  `created_at`, `updated_at`.
* **`rows`** = les lignes du tenant, tronquées à 500 (`ctx.truncated`). Ce que le module ne voit pas,
  il ne peut pas le valider.
* **Le temps** est `ctx.now_ms`, la config `ctx.config` ; jamais `SystemTime::now()`.
* **Opération nommée** : déclarez-la (`.operation(OperationDecl::new("terminer", "Terminer"))`) puis
  traitez `Op::Call` avec `req.name`.
* Bornes par appel : fuel 50 M, mémoire 4 MiB, requête 1 MiB, réponse 4 MiB.

## 4. Vérifier

```sh
cargo test -p this-module-sdk -p this-module-taches        # JSON exact + check(Stored)
crates/this-module-sdk/verifier/run.sh [crate]             # build wasm + contrôle ABI (node)
```
Le contrôle affiche `imports: []` (obligatoire), les exports, `manifest()`, puis des appels `handle`.
