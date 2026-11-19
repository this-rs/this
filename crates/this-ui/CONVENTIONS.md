# this-ui — conventions (contrat)

Feuille de style + briques de vue partagées du framework. Toute app `this-rs` en
dépend : les règles ci-dessous sont un **contrat**, pas une préférence.

## Classes CSS
- **Composant** : `.vw-<brique>` — `vw-btn`, `vw-modal`, `vw-toggle`, `vw-list-row`, `vw-field`, `vw-settings-section`, `vw-cells`.
- **Modificatrice** : `.vw-<brique>--<variante>` — `vw-btn--primary|secondary|ghost|danger`.
- **Élément interne** : `.vw-<brique>-<partie>` — `vw-modal-head`, `vw-list-label`, `vw-list-sub`.
- **Jamais de classe nue** (`btn`, `card`, `sheet`) : toujours le préfixe `vw-`.

## Jetons
`--vw-*`, **toujours avec un repli** sur une valeur neutre (l'app les alias à ses propres jetons) :
`--vw-accent`, `--vw-surface`, `--vw-line`, `--vw-danger`, `--vw-hover`.
Aucune couleur/rayon/ombre en dur **hors** la valeur de repli.

## Mouvement
- Navigation : `transition::naviguer(f)` (fondu) ou `naviguer_sens(f, Sens::Avant|Arriere)` (glissement directionnel). Keyframes `vw-vtx-*`. Le sens se pose via `[data-sens]` sur `<html>`.
- Composants : entrée `vw-pop`/`vw-fade`, press `.vw-btn:active { transform: scale(.98) }`, focus `:focus-visible` = contour accent.
- **`@media (prefers-reduced-motion: reduce)` coupe TOUT mouvement** (transitions + animations).

## Accessibilité (jamais négociable)
- Cible tactile **≥ 44 px** (`.vw-btn` min 44 ; `.vw-field-toggle` 44).
- `:focus-visible` toujours visible.
- Zones sûres : toute surface plein écran a `padding: env(safe-area-inset-*)`, ou un voile `pointer-events:none` (cartes `auto`).

## Modules Rust
Un rôle = un module : `views` (ViewBody + la const CSS), `chrome`, `transition`, `theme`, `toast`.
Le CSS vit **uniquement** dans `views/style.rs` (const `CSS` injectée une fois).

## Build
Crate **hors workspace**
- `rustfmt --edition 2024` (le hook pre-push de `this` l'exige ; `cargo fmt` racine ne le voit pas).
- Toute API navigateur (`Closure`, `web_sys::*`) sous `#[cfg(target_arch = "wasm32")]` + **repli natif**.
- `cargo clippy -- -D warnings` doit rester vert (natif = les items wasm-only sont `allow(dead_code)`).
