# Migrer un module existant vers le ModuleDescriptor

Public : un module `this-rs` écrit avant le `ModuleDescriptor` (macros `impl_data_entity!`,
`links.yaml`, trait `Module`). Contrat de conception : `docs/rfc/module-descriptor.md`. Exemple complet et
exécuté : `examples/descriptor_api` (`cargo test --example descriptor_api --features graphql,grpc`).

## En bref

- **Le passage est additif.** `Module::descriptor()` vaut `None` par défaut : un module qui ne change rien
  compile et se comporte comme avant (chemins historiques conservés : `EntityValidationConfig`, schéma GraphQL
  inféré, `auth:` YAML).
- Il y a **quelques changements non additifs**, listés plus bas. Lisez-les avant de mettre à jour.
- Un module qui adopte le descripteur gagne : la même validation sur REST, GraphQL et gRPC, des filtres typés,
  les politiques évaluées sur les trois protocoles, des messages localisés (fr/en) et un code d'erreur stable
  (`this.validation.positive`...).

## Pas à pas

1. **Déclarer les règles sur l'entité** (au lieu de `EntityValidationConfig`) avec
   `impl_data_entity_validated!` (grammaire inchangée : `validate: { create: {...}, update: {...} }`,
   `filters: {...}` pour les transformations d'entrée). Les règles deviennent des `ValidatorSpec` du champ.
2. **Fournir le descripteur** : `Module::descriptor() -> Option<ModuleDescriptor>` (la CLI `this descriptor
   export` l'engendre ; à la main : `ModuleDescriptor::new(..)`, `EntityDesc`, `FieldDesc`, `LinkDesc`).
   Pour qu'un champ soit interrogeable, déclarez-le : `.filterable(&[FilterOp::Gt, ..])`, `.sortable()`.
3. **Politiques** : `entity.permissions.ops.insert(Operation::Create, PolicyExpr::HasRole(..))`. Elles priment
   sur `auth:` du YAML **par opération** (le descripteur gagne en silence : retirez la ligne YAML qui la
   double). Défauts : `authenticated` pour les opérations CRUD d'un module compilé, `deny` pour les opérations
   métier.
4. **Messages** : un `MessageBundle` par langue (`host.locales.add_bundles(..)`). Les messages d'erreur du
   framework (`this.*`) sont déjà fournis en fr et en.
5. **Brancher le hôte** : `ServerBuilder::register_module` ne fait que stocker ; `build_host` résout les
   dépendances, vérifie l'identité (`name`/`version`/`entity_types` ⇔ descripteur, sinon
   `this.module.identity_mismatch`) et le pluriel (`this.module.plural_mismatch`).
6. **Listes** : dans vos handlers de liste, prenez `ListQuery` (extracteur) et appelez
   `EntityFetcher::list_query` plutôt que de relire `?filter=` à la main : un filtre mal formé est alors une
   erreur 400 typée, jamais ignorée.

## Changements non additifs

| Changement | Effet | Que faire |
|---|---|---|
| **Opération inconnue dans la macro** : `impl_data_entity_validated!` avec `validate: { creat: {..} }` | `compile_error!` (`unknown operation ... expected create or update`). Avant : la faute de frappe était ignorée en silence et la règle n'était jamais appliquée. | Corrigez en `create` / `update`. |
| **`ExtractorError` est `#[non_exhaustive]`** et porte la variante `Contract(Box<ContractError>)` | Un `match` exhaustif sur `ExtractorError` dans votre code ne compile plus. | Ajoutez un bras `_ => ..`. La variante `Contract` porte le statut, le code et le corps de l'erreur du contrat (422 `this.validation.failed`...). |
| **`Module`** : nouvelle méthode `descriptor()` | Additif : valeur par défaut `None`. Seul un `impl` qui définirait déjà une méthode de ce nom est touché. | Rien, ou implémentez-la (étape 2). |
| **`AuthProvider`** : nouvelles méthodes `extract_context_from_metadata` (feature `grpc`) et `extract_context_from_handshake` | Valeurs par défaut qui délèguent à `extract_context` : un fournisseur existant sert gRPC (en-têtes HTTP/2) et la poignée de main WebSocket (cookie `this_token`, paramètre `access_token`) sans changer. | Surchargez seulement pour lire des métadonnées binaires (`-bin`). |
| **`EntityFetcher`** : nouvelles méthodes `list_query` et `list_query_max_rows` | Valeurs par défaut. `list_query` charge **au plus** `list_query_max_rows()` lignes (10 000 par défaut, `THIS_LIST_QUERY_MAX_ROWS` ou surcharge) et échoue au-delà avec `this.query.too_large` (400). | Pour les grosses tables, surchargez `list_query` et poussez le filtre au moteur (SQL `WHERE`/`ORDER BY`/`LIMIT`, filtre Mongo, Cypher). |
| **Corps d'erreur HTTP** | Additif : `error` et `errors` restent ; `code`, `kind`, `message_key`, `fields`, `message` et les clés RFC 9457 (`type`, `title`, `status`, `detail`) s'ajoutent. En-tête `x-this-error`. | Les clients peuvent s'appuyer sur `code`. |
| **Validation structurelle** pour toute entité décrite | La validation s'applique aussi à GraphQL, gRPC et aux routes de lien, et refuse les champs inconnus (`this.validation.unknown_field`) et les types incorrects (`type_mismatch`, plus un 500). | Si vous envoyiez des champs en trop, déclarez-les ou retirez-les. |
| **Doublons** (module, entité, pluriel, code d'erreur, fetcher) | Refus au démarrage ; avant, le dernier gagnait en silence (entre modules décrits). | Renommez ou retirez le doublon. |
| **Jeton de politique inconnu** | Erreur (fail-closed) au lieu d'un repli sur `authenticated`. | Corrigez la politique du descripteur. |

## Ce qui n'est pas encore couvert

- Chargement à chaud : servi en **REST seulement** ; GraphQL, gRPC et WebSocket lisent les champs de l'hôte
  figés au build et refusent (fail-closed). Voir RFC §10.2 et la décision « chargement à chaud fermé par
  défaut ».
- Les liens d'un module compilé existent encore dans `links.yaml` **et** dans `module.yaml` : aucune
  comparaison n'est faite, une dérive est silencieuse (RFC §10.4, R3).
- Les listes de liens lisent encore `QueryParams::filter_value` (JSON mal formé ignoré).
- Tags proto : un module compilé est servi sans tags (`descriptor.lock` appliqué par la CLI seulement).
