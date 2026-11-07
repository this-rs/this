//! # L'approbation d'une composition de harnais
//!
//! `graph.rs` le dit : « `_fingerprint` détecte qu'une composition a changé, il ne
//! prouve pas qu'elle est autorisée. L'approbation (signature, revue, ancre de
//! confiance) reste au-dessus, dans l'hôte. Un agent qui s'accorde sa propre
//! politique n'a pas de limite. » Or un agent qui écrit ses propres modules écrit
//! aussi ceux qui tiennent `Policy` et `Guard` : sans ancre, ce n'est pas de
//! l'auto-construction, c'est de l'escalade de privilèges.
//!
//! Ce module donne à l'hôte de quoi **refuser** — sans I/O, sans signature.
//!
//! ## Le modèle
//!
//! - [`HarnessDoc::fingerprint`] : SHA-256 d'une forme **canonique** de la composition
//!   (clés triées, aucun espace), `approval` exclue — sinon approuver changerait
//!   l'empreinte approuvée.
//! - [`Approval`] : *qui*, *quand*, et *l'empreinte de ce qui a été approuvé*.
//!   C'est l'empreinte qui rend l'approbation non transférable : modifier la
//!   composition après coup l'invalide mécaniquement. Une approbation peut en plus
//!   épingler le **contenu** de chaque module (`digests`) : elle est alors plus
//!   forte — un module republié sous la même version est refusé, et nommé.
//! - [`HarnessDoc::is_approved`] : la vérification, pure. Le champ `approval` du
//!   document est une **affirmation**, écrite par n'importe qui (l'agent compris).
//!   Seule la liste `trusted`, tenue par l'hôte, fait foi : l'approbation du
//!   document doit y trouver le même approbateur **et** la même empreinte.
//! - [`HarnessDoc::is_approved_strict`] : la même vérification, plus l'exigence
//!   qu'**aucun emplacement ne reste sans digest** — pour un hôte qui refuse de
//!   faire confiance à une référence seule.
//!
//! ## Ce que l'empreinte couvre
//!
//! Tout ce que sérialise `HarnessDoc` sauf `approval` : id, nom, version, brief,
//! emplacements (référence **et** digest de contenu), bornes — et tout champ ajouté
//! plus tard, sans rien changer ici (on hache le JSON canonique, pas une liste de
//! champs écrite à la main : un champ oublié serait un trou d'autorisation silencieux).
//!
//! `slots` est une **liste ordonnée** : l'ordre de composition décide qui tient un
//! emplacement (le premier du genre, voir [`HarnessDoc::slot`]). Permuter deux
//! emplacements change donc l'empreinte, et c'est voulu.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::graph::HarnessDoc;
use crate::substrate::Verdict;

/// Une approbation : qui a approuvé, quand, et **quelle composition**.
///
/// `at_ms` est informatif (audit) : il n'entre dans aucune décision, une
/// approbation n'expire pas toute seule.
///
/// `digests` épingle le **contenu** des modules au moment d'approuver
/// (`module` → digest hexadécimal). POURQUOI : la référence d'un module
/// (`module` + `version`) peut être republiée sous un autre code, pas un contenu
/// haché. Une approbation qui porte des digests est donc **plus forte** qu'une
/// approbation de référence seule : [`HarnessDoc::is_approved`] refuse un module
/// dont le contenu a bougé, en nommant l'emplacement. Absent (ou vide) :
/// approbation de référence, compatible avec les documents d'hier. Ce champ ne
/// change pas l'empreinte : il est porté par `approval`, exclue de l'empreinte.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub by: String,
    pub at_ms: u64,
    /// L'empreinte de la composition au moment où elle a été approuvée.
    pub fingerprint: String,
    /// Les digests de contenu épinglés : `module` → digest hexadécimal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub digests: BTreeMap<String, String>,
}

/// La forme canonique d'une valeur JSON : clés triées (indépendamment de la
/// feature `preserve_order` de serde_json, que n'importe quel crate du graphe de
/// dépendances peut activer), tableaux dans leur ordre, aucun espace.
fn canonique(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let tries: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (i, (k, val)) in tries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonique(val, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, val) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonique(val, out);
            }
            out.push(']');
        }
        autre => out.push_str(&autre.to_string()),
    }
}

impl HarnessDoc {
    /// Empreinte SHA-256 (64 caractères hexadécimaux) de la composition canonique,
    /// `approval` exclue. Voir le module pour ce qu'elle couvre.
    ///
    /// Elle **détecte** un changement ; elle ne prouve aucune autorisation.
    pub fn fingerprint(&self) -> String {
        let mut valeur = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut valeur {
            map.remove("approval");
        }
        let mut canon = String::new();
        canonique(&valeur, &mut canon);
        let mut hex = String::with_capacity(64);
        for octet in Sha256::digest(canon.as_bytes()) {
            hex.push_str(&format!("{octet:02x}"));
        }
        hex
    }

    /// Cette composition est-elle approuvée par quelqu'un que l'hôte connaît ?
    ///
    /// `trusted` est la liste de l'hôte : les approbations qu'il a lui-même
    /// enregistrées. Le refus dit *lequel des deux a bougé* :
    ///
    /// 1. pas d'`approval` dans le document → refusé ;
    /// 2. un emplacement dont le digest de contenu ne correspond plus à celui
    ///    approuvé → **le contenu** d'un module a changé, l'emplacement est nommé ;
    /// 3. l'empreinte approuvée n'est plus celle du document → **le document** a
    ///    changé depuis l'approbation ;
    /// 4. aucun approbateur de ce nom chez l'hôte → approbateur inconnu ;
    /// 5. l'hôte connaît l'approbateur mais pas pour cette empreinte → **l'hôte**
    ///    n'a pas approuvé cette composition (l'affirmation du document est seule).
    pub fn is_approved(&self, trusted: &[Approval]) -> Verdict {
        let Some(approval) = &self.approval else {
            return Verdict::Refuse("aucune approbation : composition non approuvée".into());
        };
        // Le contenu d'abord : si l'approbation a épinglé des digests, un module
        // republié sous la même version est nommé ici, précisément. L'empreinte
        // couvre déjà le digest (elle hache le JSON canonique) — mais elle ne dit
        // pas *lequel* des emplacements a bougé ; ce contrôle le dit.
        for slot in &self.slots {
            match (slot.digest.as_deref(), approval.digests.get(&slot.module)) {
                (Some(actuel), Some(attendu)) if actuel != attendu.as_str() => {
                    return Verdict::Refuse(format!(
                        "le contenu de l'emplacement {}:{}@{} a changé depuis \
                         l'approbation (digest {actuel}, approuvé {attendu})",
                        slot.kind.as_str(),
                        slot.module,
                        slot.version
                    ));
                }
                (None, Some(attendu)) => {
                    return Verdict::Refuse(format!(
                        "l'emplacement {}:{}@{} a perdu son digest de contenu depuis \
                         l'approbation (approuvé {attendu})",
                        slot.kind.as_str(),
                        slot.module,
                        slot.version
                    ));
                }
                _ => {}
            }
        }
        let actuelle = self.fingerprint();
        if approval.fingerprint != actuelle {
            return Verdict::Refuse(format!(
                "le document a changé depuis l'approbation de « {} » \
                 (approuvé {}, actuel {})",
                approval.by, approval.fingerprint, actuelle
            ));
        }
        let mut connu = false;
        for t in trusted.iter().filter(|t| t.by == approval.by) {
            connu = true;
            if t.fingerprint == actuelle {
                return Verdict::Allow;
            }
        }
        if connu {
            Verdict::Refuse(format!(
                "l'hôte connaît « {} » mais n'a pas approuvé cette empreinte ({actuelle}) : \
                 l'approbation du document est une affirmation sans contrepartie",
                approval.by
            ))
        } else {
            Verdict::Refuse(format!(
                "approbateur inconnu de l'hôte : « {} »",
                approval.by
            ))
        }
    }

    /// [`HarnessDoc::is_approved`], plus l'exigence que **chaque emplacement porte
    /// un digest de contenu**.
    ///
    /// Une approbation sans digest ne lie que la référence (`module`, `version`) :
    /// un registre qui accepte de republier `module 0.3.0` sous un autre code la
    /// laisse intacte. L'hôte qui exige que l'approbation épingle le **contenu**
    /// passe par ici — le refus nomme l'emplacement fautif. C'est l'hôte qui
    /// choisit : [`HarnessDoc::is_approved`] reste tolérant pour les documents
    /// d'hier qui n'ont pas de digest.
    pub fn is_approved_strict(&self, trusted: &[Approval]) -> Verdict {
        // Attaque : un digest `Some("")` (champ présent mais vide) passait le strict,
        // l'hôte croyant le contenu épinglé alors que rien ne l'était. Un digest vide
        // vaut un digest absent : il n'épingle aucun octet.
        for slot in &self.slots {
            if slot.digest.as_deref().is_none_or(str::is_empty) {
                return Verdict::Refuse(format!(
                    "l'emplacement {}:{}@{} n'a pas de digest de contenu : \
                     l'approbation ne peut pas garantir ce qui y tourne",
                    slot.kind.as_str(),
                    slot.module,
                    slot.version
                ));
            }
        }
        self.is_approved(trusted)
    }
}
