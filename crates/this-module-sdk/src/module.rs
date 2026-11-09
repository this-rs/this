//! Le trait qu'un module implémente.

use crate::{Manifest, Request, Response};

/// Un module `this`. Deux fonctions, aucun état : le module est rappelé à chaque requête
/// dans une instance neuve, rien ne survit d'un appel à l'autre.
pub trait Module {
    /// Le descripteur : il doit passer le même `check(Stored)` qu'un module stocké.
    fn manifest() -> Manifest;
    /// La logique métier. Total : toute entrée se traduit en `Response` (un panic est un trap).
    fn handle(req: Request) -> Response;
}
