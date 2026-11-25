//! Les objets S3 exposés **comme outils** au harnais.
//!
//! Ce n'est pas un back-end pour une app : c'est une **surface d'outils** que le
//! harnais (l'agent) appelle et que les modules composent — explorer, représenter,
//! consulter, organiser, écrire, supprimer. Le tenant est fixé à la construction
//! (le compte servi) : aucune clé d'un autre tenant n'est atteignable.
//!
//! Branché dans un [`this_harness::Toolboxes`] à côté des autres outillages :
//!
//! ```ignore
//! let boite = ObjetsToolbox::new(objets.clone(), "compte-123");
//! let outils = Toolboxes::new().with(Rc::new(boite));
//! ```
//!
//! **Événementiel** : l'écriture observe son effet (`Outcome::observed`) quand
//! l'appelant le demande ; le flux d'événements (créé/modifié/supprimé) se
//! branche côté hôte sur le même `Objets` (voir `events`), pas ici.

use super::{EvenementObjet, GenreObjet, Objets};
use async_trait::async_trait;
use bytes::Bytes;
use serde_json::{Value, json};
use this_harness::{Cancel, Declaration, Outcome, ToolCall, ToolSpec, Toolbox};
use tokio::sync::broadcast;

/// Surface d'outils « objets » pour un compte donné.
#[derive(Clone)]
pub struct ObjetsToolbox {
    objets: Objets,
    tenant: String,
}

const OUTILS: [&str; 6] = [
    "objets_explorer",
    "objets_consulter",
    "objets_ecrire",
    "objets_organiser",
    "objets_supprimer",
    "objets_representer",
];

/// Rend un objet texte lisible : UTF-8, tronqué proprement, ou « binaire ».
fn rendre(cle: &str, octets: &[u8], max: usize) -> String {
    match std::str::from_utf8(octets) {
        Ok(t) => {
            let coupe: String = t.chars().take(max).collect();
            if coupe.chars().count() < t.chars().count() {
                format!("{cle} (tronqué à {max} car.)\n\n{coupe}…")
            } else {
                format!("{cle}\n\n{coupe}")
            }
        }
        Err(_) => format!("{cle} : binaire, {} octets", octets.len()),
    }
}

fn arg_str(call: &ToolCall, cle: &str) -> Option<String> {
    call.arguments
        .get(cle)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn arg_u64(call: &ToolCall, cle: &str, defaut: u64, max: u64) -> usize {
    call.arguments
        .get(cle)
        .and_then(Value::as_u64)
        .unwrap_or(defaut)
        .min(max) as usize
}

fn arg_bool(call: &ToolCall, cle: &str) -> bool {
    call.arguments
        .get(cle)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

impl ObjetsToolbox {
    pub fn new(objets: Objets, tenant: impl Into<String>) -> Self {
        Self {
            objets,
            tenant: tenant.into(),
        }
    }

    /// S'abonner au flux des changements (écrit / supprimé) de l'espace.
    ///
    /// À brancher dans la boucle de l'hôte : chaque événement reçu peut devenir
    /// un [`super::EvenementObjet`] rendu par [`Self::describe_event`], injecté dans le
    /// contexte du harnais ou consommé par un module.
    pub fn abonner(&self) -> broadcast::Receiver<EvenementObjet> {
        self.objets.abonner()
    }

    /// Un événement rendu en une ligne lisible pour le contexte du harnais.
    pub fn describe_event(e: &EvenementObjet) -> String {
        match e.genre {
            GenreObjet::Ecrit => {
                format!(
                    "objet écrit : {} ({} o) [tenant {}]",
                    e.cle, e.taille, e.tenant
                )
            }
            GenreObjet::Supprime => {
                format!("objet supprimé : {} [tenant {}]", e.cle, e.tenant)
            }
        }
    }

    /// Exécute un appel ; rend l'`Outcome` que le modèle lira.
    async fn executer(&self, call: &ToolCall) -> Outcome {
        match call.name.as_str() {
            "objets_explorer" => self.explorer(call).await,
            "objets_consulter" => self.consulter(call).await,
            "objets_ecrire" => self.write(call).await,
            "objets_organiser" => self.organiser(call).await,
            "objets_supprimer" => self.supprimer(call).await,
            "objets_representer" => self.representer(call).await,
            autre => Outcome::refused(
                format!("outil inconnu : {autre}"),
                "appel à un outil qui n'est pas dans la surface « objets »".to_string(),
            ),
        }
    }

    async fn explorer(&self, call: &ToolCall) -> Outcome {
        let prefixe = arg_str(call, "prefixe").unwrap_or_default();
        let limite = arg_u64(call, "limite", 200, 500);
        match self.objets.lister(&self.tenant, &prefixe).await {
            Ok(entrees) => {
                if entrees.is_empty() {
                    return Outcome::ok("(aucun objet sous ce préfixe)".to_string());
                }
                let mut lignes: Vec<String> = entrees
                    .iter()
                    .take(limite)
                    .map(|e| format!("- {} ({} o)", e.cle, e.taille))
                    .collect();
                if entrees.len() > limite {
                    lignes.push(format!("… {} autre(s)", entrees.len() - limite));
                }
                Outcome::ok(lignes.join("\n"))
            }
            Err(e) => Outcome::failed(
                "l'exploration a échoué".to_string(),
                format!("lister « {prefixe} » : {e}"),
            ),
        }
    }

    async fn consulter(&self, call: &ToolCall) -> Outcome {
        let Some(cle) = arg_str(call, "cle") else {
            return Outcome::refused("clé absente".to_string(), "`cle` est requis".to_string());
        };
        let max = arg_u64(call, "max_octets", 4096, 1_000_000);
        match self.objets.read(&self.tenant, &cle).await {
            Ok(octets) => Outcome::ok(rendre(&cle, &octets, max)),
            Err(e) => Outcome::failed(
                format!("objet « {cle} » illisible"),
                format!("lire « {cle} » : {e}"),
            ),
        }
    }

    async fn write(&self, call: &ToolCall) -> Outcome {
        let (Some(cle), Some(contenu)) = (arg_str(call, "cle"), arg_str(call, "contenu")) else {
            return Outcome::refused(
                "écriture incomplète".to_string(),
                "`cle` et `contenu` sont requis".to_string(),
            );
        };
        let octets = Bytes::from(contenu.into_bytes());
        let taille = octets.len();
        match self.objets.write(&self.tenant, &cle, octets).await {
            Ok(()) => Outcome::ok(format!("écrit : {cle} ({taille} o)")),
            Err(e) => Outcome::failed(
                format!("écriture de « {cle} » refusée"),
                format!("écrire « {cle} » : {e}"),
            ),
        }
    }

    async fn organiser(&self, call: &ToolCall) -> Outcome {
        let (Some(de), Some(vers)) = (arg_str(call, "de"), arg_str(call, "vers")) else {
            return Outcome::refused(
                "réorganisation incomplète".to_string(),
                "`de` et `vers` sont requis".to_string(),
            );
        };
        let deplacer = arg_bool(call, "deplacer");
        let octets = match self.objets.read(&self.tenant, &de).await {
            Ok(o) => o,
            Err(e) => {
                return Outcome::failed(
                    format!("source « {de} » illisible"),
                    format!("lire « {de} » : {e}"),
                );
            }
        };
        if let Err(e) = self.objets.write(&self.tenant, &vers, octets).await {
            return Outcome::failed(
                format!("destination « {vers} » refusée"),
                format!("écrire « {vers} » : {e}"),
            );
        }
        if deplacer {
            if let Err(e) = self.objets.supprimer(&self.tenant, &de).await {
                return Outcome::failed(
                    format!("« {vers} » écrit mais « {de} » non supprimé"),
                    format!("supprimer « {de} » : {e}"),
                );
            }
            Outcome::ok(format!("déplacé : {de} → {vers}"))
        } else {
            Outcome::ok(format!("copié : {de} → {vers}"))
        }
    }

    async fn supprimer(&self, call: &ToolCall) -> Outcome {
        let Some(cle) = arg_str(call, "cle") else {
            return Outcome::refused("clé absente".to_string(), "`cle` est requis".to_string());
        };
        match self.objets.supprimer(&self.tenant, &cle).await {
            Ok(()) => Outcome::ok(format!("supprimé : {cle}")),
            Err(e) => Outcome::failed(
                format!("suppression de « {cle} » refusée"),
                format!("supprimer « {cle} » : {e}"),
            ),
        }
    }

    async fn representer(&self, call: &ToolCall) -> Outcome {
        let prefixe = arg_str(call, "prefixe").unwrap_or_default();
        match self.objets.lister(&self.tenant, &prefixe).await {
            Ok(entrees) => {
                let total: u64 = entrees.iter().map(|e| e.taille).sum();
                let mut par_dossier: std::collections::BTreeMap<&str, (usize, u64)> =
                    std::collections::BTreeMap::new();
                for e in &entrees {
                    let dossier = e.cle.split('/').next().unwrap_or("");
                    let case = par_dossier.entry(dossier).or_insert((0, 0));
                    case.0 += 1;
                    case.1 += e.taille;
                }
                let mut lignes = vec![format!("Espace : {} objet(s), {} o", entrees.len(), total)];
                for (dossier, (n, taille)) in par_dossier {
                    lignes.push(format!("  {dossier}/ : {n} objet(s), {taille} o"));
                }
                Outcome::ok(lignes.join("\n"))
            }
            Err(e) => Outcome::failed(
                "représentation impossible".to_string(),
                format!("lister « {prefixe} » : {e}"),
            ),
        }
    }
}

#[async_trait(?Send)]
impl Toolbox for ObjetsToolbox {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "objets_explorer".to_string(),
                description: concat!(
                    "Liste les objets rangés dans l'espace de l'utilisateur (clés et taille). ",
                    "`prefixe` filtre par début de clé, `limite` borne le nombre (défaut 200)."
                )
                .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "prefixe": { "type": "string", "description": "Début de clé à filtrer (optionnel)." },
                        "limite": { "type": "integer", "description": "Nombre max d'entrées (défaut 200, max 500)." }
                    }
                }),
            },
            ToolSpec {
                name: "objets_consulter".to_string(),
                description: concat!(
                    "Lit le contenu d'un objet texte (UTF-8) et le rend tronqué. ",
                    "`cle` désigne l'objet, `max_octets` la longueur rendue (défaut 4096)."
                )
                .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "cle": { "type": "string", "description": "Clé de l'objet." },
                        "max_octets": { "type": "integer", "description": "Longueur max rendue (défaut 4096)." }
                    },
                    "required": ["cle"]
                }),
            },
            ToolSpec {
                name: "objets_ecrire".to_string(),
                description: concat!(
                    "Écrit (ou remplace) un objet texte. `cle` la destination, `contenu` le texte. ",
                    "Pour un graphe de connaissances, écris sa sérialisation JSON sous une clé dédiée."
                )
                .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "cle": { "type": "string", "description": "Clé de destination." },
                        "contenu": { "type": "string", "description": "Contenu texte (UTF-8)." }
                    },
                    "required": ["cle", "contenu"]
                }),
            },
            ToolSpec {
                name: "objets_organiser".to_string(),
                description: concat!(
                    "Copie ou déplace un objet de `de` vers `vers`. ",
                    "`deplacer` (bool, défaut faux) retire la source après copie."
                )
                .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "de": { "type": "string", "description": "Clé source." },
                        "vers": { "type": "string", "description": "Clé destination." },
                        "deplacer": { "type": "boolean", "description": "Supprimer la source après copie (défaut faux)." }
                    },
                    "required": ["de", "vers"]
                }),
            },
            ToolSpec {
                name: "objets_supprimer".to_string(),
                description: "Supprime un objet de l'espace. `cle` désigne l'objet.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "cle": { "type": "string", "description": "Clé à supprimer." }
                    },
                    "required": ["cle"]
                }),
            },
            ToolSpec {
                name: "objets_representer".to_string(),
                description: concat!(
                    "Représente l'espace : nombre d'objets, taille totale, répartition par dossier. ",
                    "`prefixe` restreint la vue."
                )
                .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "prefixe": { "type": "string", "description": "Début de clé à restreindre (optionnel)." }
                    }
                }),
            },
        ]
    }

    fn declarations(&self) -> Vec<Declaration> {
        vec![
            Declaration::reads("objets_explorer").words(["exploré", "listé", "parcouru"]),
            Declaration::reads("objets_consulter").words(["consulté", "lu"]),
            Declaration::writes("objets_ecrire").words(["écrit", "enregistré", "stocké"]),
            Declaration::writes("objets_organiser").words(["organisé", "déplacé", "copié"]),
            Declaration::writes("objets_supprimer").words(["supprimé", "effacé"]),
            Declaration::reads("objets_representer").words(["représenté", "résumé"]),
        ]
    }

    async fn call(&self, call: &ToolCall) -> Outcome {
        self.call_with_cancel(call, &Cancel::new()).await
    }

    async fn call_with_cancel(&self, call: &ToolCall, cancel: &Cancel) -> Outcome {
        if cancel.is_cancelled() {
            return Outcome::failed(
                "appel annulé".to_string(),
                "annulé avant l'exécution".to_string(),
            );
        }
        if !OUTILS.contains(&call.name.as_str()) {
            return Outcome::refused(
                format!("outil inconnu : {}", call.name),
                "pas dans la surface « objets »".to_string(),
            );
        }
        self.executer(call).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use std::sync::Arc;

    fn boite() -> ObjetsToolbox {
        let objets = Objets::new(Arc::new(InMemory::new()) as Arc<dyn object_store::ObjectStore>);
        ObjetsToolbox::new(objets, "alice")
    }

    fn appel(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.into(),
            arguments,
        }
    }

    #[tokio::test]
    async fn ecrire_puis_lire_puis_explorer_puis_supprimer() {
        let b = boite();
        let o = b
            .executer(&appel(
                "objets_ecrire",
                json!({"cle":"graphes/socle.json","contenu":"{\"noeuds\":1}"}),
            ))
            .await;
        assert!(o.failed.is_none(), "{o:?}");

        let l = b
            .executer(&appel(
                "objets_consulter",
                json!({"cle":"graphes/socle.json"}),
            ))
            .await;
        assert!(l.text.contains("noeuds"), "{}", l.text);

        let x = b.executer(&appel("objets_explorer", json!({}))).await;
        assert!(x.text.contains("graphes/socle.json"), "{}", x.text);

        let r = b.executer(&appel("objets_representer", json!({}))).await;
        assert!(r.text.contains("Espace"), "{}", r.text);

        let d = b
            .executer(&appel(
                "objets_supprimer",
                json!({"cle":"graphes/socle.json"}),
            ))
            .await;
        assert!(d.failed.is_none(), "{d:?}");

        let x2 = b.executer(&appel("objets_explorer", json!({}))).await;
        assert!(!x2.text.contains("socle.json"), "{}", x2.text);
    }

    #[tokio::test]
    async fn organiser_deplace_vers_nouvelle_cle() {
        let b = boite();
        b.executer(&appel(
            "objets_ecrire",
            json!({"cle":"a.txt","contenu":"x"}),
        ))
        .await;
        let m = b
            .executer(&appel(
                "objets_organiser",
                json!({"de":"a.txt","vers":"d/b.txt","deplacer":true}),
            ))
            .await;
        assert!(m.failed.is_none(), "{m:?}");
        assert!(!b.objets.existe(&b.tenant, "a.txt").await.unwrap());
        assert!(b.objets.existe(&b.tenant, "d/b.txt").await.unwrap());
    }

    #[tokio::test]
    async fn un_tenant_ne_voit_pas_les_objets_d_un_autre() {
        let objets = Objets::new(Arc::new(InMemory::new()) as Arc<dyn object_store::ObjectStore>);
        let alice = ObjetsToolbox::new(objets.clone(), "alice");
        let bob = ObjetsToolbox::new(objets.clone(), "bob");
        alice
            .executer(&appel(
                "objets_ecrire",
                json!({"cle":"secret","contenu":"à moi"}),
            ))
            .await;
        let vue_bob = bob.executer(&appel("objets_explorer", json!({}))).await;
        assert!(!vue_bob.text.contains("secret"), "{}", vue_bob.text);
    }

    #[test]
    fn chaque_lecture_et_ecriture_est_declaree() {
        let b = boite();
        let noms: Vec<String> = b.declarations().into_iter().map(|d| d.tool).collect();
        for outil in OUTILS {
            assert!(
                noms.contains(&outil.to_string()),
                "déclaration manquante : {outil}"
            );
        }
    }

    #[tokio::test]
    async fn un_evenement_est_emis_a_l_ecriture_et_a_la_suppression() {
        let b = boite();
        let mut rx = b.abonner();
        b.executer(&appel(
            "objets_ecrire",
            json!({"cle":"graphes/g.json","contenu":"{\"n\":2}"}),
        ))
        .await;
        let e = rx.recv().await.expect("un événement d'écriture");
        assert_eq!(e.genre, GenreObjet::Ecrit);
        assert_eq!(e.cle, "graphes/g.json");
        assert_eq!(e.tenant, "alice");
        assert!(ObjetsToolbox::describe_event(&e).contains("écrit"));

        b.executer(&appel("objets_supprimer", json!({"cle":"graphes/g.json"})))
            .await;
        let e2 = rx.recv().await.expect("un événement de suppression");
        assert_eq!(e2.genre, GenreObjet::Supprime);
        assert_eq!(e2.cle, "graphes/g.json");
        assert!(ObjetsToolbox::describe_event(&e2).contains("supprimé"));
    }
}
