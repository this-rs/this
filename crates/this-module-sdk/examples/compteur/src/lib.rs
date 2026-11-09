//! Module `compteur` : un module qui **calcule** son arbre de vues.
//!
//! La config statique (`views` du manifeste) dit « une liste, un bouton ». Ici l'arbre dépend de
//! ce que l'hôte passe à `render` : le nombre de tâches ouvertes choisit la bannière et la
//! forme du corps, le filtre choisit les lignes lues. Voir `README.md`.

use this_module_sdk::views::{
    Brick, Filter, FormField, Node, Surface, Tab, Text, Tone, View, ViewError,
};
use this_module_sdk::{
    Entity, Field, Manifest, Module, OperationDecl, Request, Response, Value, ident, json, text,
};

pub struct Compteur;

impl Module for Compteur {
    fn manifest() -> Manifest {
        Manifest::new("compteur", "0.1.0")
            .entity(
                Entity::new("tache", "taches")
                    .field(Field::string("titre"))
                    .field(Field::bool("fait").default(json!(false))),
            )
            .operation(OperationDecl::new("terminer", "Terminer la tâche"))
            .action(json!({"kind": "create", "name": "creer", "entity": "tache", "fields": ["titre"]}))
            .action(json!({"kind": "invoke", "name": "terminer", "operation": "terminer", "entity": "tache"}))
    }

    fn handle(req: Request) -> Response {
        Response::err(
            "operation_inconnue",
            format!("ce module ne fait que rendre ({:?})", req.op),
        )
    }
}

/// Au-delà, la bannière passe en erreur.
const SEUIL: u64 = 10;

/// L'arbre : une fonction de la requête. `{"ouvertes": 3, "filtre": "a_faire" | "faites"}`.
pub fn rendre(req: Value) -> Result<View, ViewError> {
    let ouvertes = req["ouvertes"].as_u64().unwrap_or(0);
    let filtre = req["filtre"].as_str().unwrap_or("a_faire");

    // 1. La bannière : trois tons selon la valeur.
    let bandeau = match ouvertes {
        0 => Node::banner(Tone::Success, text!("Tout est fait")),
        n if n < SEUIL => Node::banner(Tone::Info, Text::new(format!("{n} à faire"))?),
        n => Node::banner(
            Tone::Error,
            Text::new(format!(
                "{n} tâches ouvertes : trop, en terminer avant d'en ajouter"
            ))?,
        ),
    };

    // 2. Le corps : une brique différente selon l'état.
    let corps = if ouvertes == 0 {
        // Rien à lire : on propose d'en créer une.
        Node::leaf(
            Brick::form(
                ident!("tache"),
                FormField::new(ident!("titre")).label(text!("Nouvelle tâche")),
                [],
            )
            .action(ident!("creer")),
        )
    } else {
        let faites = filtre == "faites";
        let lignes = Brick::list(ident!("tache"), ident!("titre"))
            .filter(Filter::eq(ident!("fait"), faites))
            .empty(text!("Rien ici"));
        let mut enfants = vec![Node::leaf(lignes)];
        // Le bouton n'existe que si on regarde les tâches ouvertes.
        if !faites {
            enfants.push(Node::button(ident!("tache"), ident!("terminer")));
        }
        Node::tabs(
            Tab::new(text!("Liste"), Node::stack(enfants.remove(0), enfants), []),
            [Tab::new(
                text!("Compte"),
                Node::leaf(
                    Brick::counter(ident!("tache"), None).filter(Filter::eq(ident!("fait"), false)),
                ),
                [],
            )],
        )
    };

    Ok(View::new(
        ident!("accueil"),
        Surface::Main,
        Node::stack(bandeau, [corps]),
    )?
    .title(text!("Tâches")))
}

#[cfg(feature = "render")]
this_module_sdk::export!(Compteur, render = rendre);
#[cfg(not(feature = "render"))]
this_module_sdk::export!(Compteur);
