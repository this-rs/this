//! # L'écran d'un module, dérivé du descripteur — sans navigateur
//!
//! Ce module est le chaînon qui manquait entre le descripteur d'un module et le rendu
//! d'un tableau (`this-ui::browser::EntityTable`). Il ne dessine **rien** : il traduit
//! une vue déclarée (`Brick::List`, `Brick::Form`) en un **arbre de données**
//! ([`Colonne`]/[`Ligne`]/[`Cellule`]) que le kit d'interface consomme tel quel — les
//! colonnes et les cellules sont exactement ce que `EntityTable` attend en entrée.
//!
//! Il est volontairement **pur** et compilé pour l'hôte (pas de Leptos, pas de wasm) :
//! c'est ce qui rend la preuve vérifiable sans navigateur, par un simple `cargo test`.
//!
//! # La règle qui compte
//!
//! Les colonnes d'une vue liste sont **dérivées du drapeau `sortable`** des champs de
//! l'entité — la même source unique que `handle(list)` (le tri) et la vue liste (`sort`).
//! Rendre un champ triable le fait donc apparaître comme colonne **sans toucher à ce
//! code**, exactement comme il entre déjà dans le tri. C'est la propriété au niveau de
//! l'affichage.

use serde_json::Value;
use this_descriptor::{Brick, EntityDesc, FieldType, ModuleDescriptor, Node, ViewDesc};

/// La nature d'un champ, réduite à ce qu'une cellule de tableau doit savoir.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nature {
    Texte,
    Entier,
    Decimal,
    Date,
}

impl Nature {
    pub fn of(ty: &FieldType) -> Self {
        match ty {
            FieldType::I32 | FieldType::I64 | FieldType::U32 | FieldType::U64 => Nature::Entier,
            FieldType::F32 | FieldType::F64 | FieldType::Decimal { .. } => Nature::Decimal,
            FieldType::Date => Nature::Date,
            // String, Text, Bool, Uuid, Json, Enum, Ref, List, DateTime… : lu comme du texte.
            _ => Nature::Texte,
        }
    }

    /// Une colonne numérique s'aligne à droite, en chiffres tabulaires.
    pub fn est_numerique(self) -> bool {
        matches!(self, Nature::Entier | Nature::Decimal)
    }

    /// Le `<input type=…>` inféré pour un formulaire (même inférence que le kit).
    pub fn saisie(self) -> &'static str {
        match self {
            Nature::Entier | Nature::Decimal => "number",
            Nature::Date => "date",
            Nature::Texte => "text",
        }
    }
}

/// Une colonne du tableau, dérivée du descripteur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Colonne {
    /// Le nom du champ (`due_date`).
    pub champ: String,
    /// Le libellé lu par une personne (« Due date »).
    pub libelle: String,
    pub nature: Nature,
}

/// Le contenu d'une cellule.
#[derive(Clone, Debug, PartialEq)]
pub enum Cellule {
    Texte(String),
    /// Un montant, aligné à droite.
    Montant(String),
    Date(String),
    Vide,
}

/// Une ligne du tableau, prête pour `EntityTable` (l'`id` porte l'action de ligne).
#[derive(Clone, Debug, PartialEq)]
pub struct Ligne {
    pub id: String,
    pub cellules: Vec<Cellule>,
}

/// L'écran liste : l'identité (le `label` de la brique) plus une colonne par champ
/// **triable** de l'entité, dans l'ordre de déclaration.
#[derive(Clone, Debug, PartialEq)]
pub struct ListeEcran {
    pub entite: String,
    /// Le champ d'identité de la ligne (le `label` de la brique `list`).
    pub identite: String,
    /// Les colonnes de données — une par champ triable.
    pub colonnes: Vec<Colonne>,
}

impl ListeEcran {
    /// L'en-tête réel du tableau : l'identité en premier, puis les champs triables.
    pub fn en_tete(&self) -> Vec<Colonne> {
        let mut tete = vec![Colonne {
            champ: self.identite.clone(),
            libelle: libelle(&self.identite),
            nature: Nature::Texte,
        }];
        tete.extend(self.colonnes.clone());
        tete
    }

    /// Les lignes du tableau, alignées sur [`ListeEcran::en_tete`].
    pub fn lignes(&self, lignes: &[Value]) -> Vec<Ligne> {
        lignes.iter().map(|ligne| self.ligne(ligne)).collect()
    }

    fn ligne(&self, ligne: &Value) -> Ligne {
        let id = ligne
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut cellules = vec![cellule(ligne.get(&self.identite), Nature::Texte)];
        cellules.extend(
            self.colonnes
                .iter()
                .map(|colonne| cellule(ligne.get(&colonne.champ), colonne.nature)),
        );
        Ligne { id, cellules }
    }
}

/// Un champ de formulaire, dérivé d'une vue `Brick::Form`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChampFormulaire {
    pub champ: String,
    pub libelle: String,
    pub saisie: &'static str,
    pub requis: bool,
    pub lecture_seule: bool,
}

/// L'écran formulaire : ses champs, dans l'ordre déclaré par la vue.
#[derive(Clone, Debug, PartialEq)]
pub struct FormulaireEcran {
    pub entite: String,
    /// Le nom de l'action de soumission (`submit` de la brique), s'il y en a une.
    pub soumission: Option<String>,
    pub champs: Vec<ChampFormulaire>,
}

/// L'écran liste d'une vue, dérivé du descripteur. `vue` est le nom d'une vue déclarée
/// dans le manifest (`"taches"`).
pub fn liste(desc: &ModuleDescriptor, vue: &str) -> Result<ListeEcran, String> {
    let v = vue_de(desc, vue)?;
    let Brick::List { entity, label, .. } = brique_de(v)? else {
        return Err(format!("la vue `{vue}` n'est pas une liste"));
    };
    let entite = entite_de(desc, entity)?;
    Ok(ListeEcran {
        entite: entity.clone(),
        identite: label.clone(),
        colonnes: colonnes_triables(entite),
    })
}

/// L'écran formulaire d'une vue, dérivé du descripteur. `vue` est le nom d'une vue
/// déclarée dans le manifest (`"nouvelle_tache"`).
pub fn formulaire(desc: &ModuleDescriptor, vue: &str) -> Result<FormulaireEcran, String> {
    let v = vue_de(desc, vue)?;
    let Brick::Form {
        entity,
        fields,
        submit,
        ..
    } = brique_de(v)?
    else {
        return Err(format!("la vue `{vue}` n'est pas un formulaire"));
    };
    let entite = entite_de(desc, entity)?;
    let mut champs = Vec::with_capacity(fields.len());
    for ff in fields {
        let champ = entite
            .field(&ff.field)
            .ok_or_else(|| format!("champ inconnu `{}`", ff.field))?;
        let nature = Nature::of(&champ.ty);
        champs.push(ChampFormulaire {
            champ: ff.field.clone(),
            libelle: ff.label.clone().unwrap_or_else(|| libelle(&ff.field)),
            saisie: nature.saisie(),
            requis: !champ.optional && champ.default.is_none(),
            lecture_seule: ff.read_only || champ.read_only,
        });
    }
    Ok(FormulaireEcran {
        entite: entity.clone(),
        soumission: submit.clone(),
        champs,
    })
}

/// Une colonne par champ **triable**, dans l'ordre de déclaration.
fn colonnes_triables(entite: &EntityDesc) -> Vec<Colonne> {
    entite
        .fields
        .iter()
        .filter(|f| f.sortable)
        .map(|f| Colonne {
            champ: f.name.clone(),
            libelle: libelle(&f.name),
            nature: Nature::of(&f.ty),
        })
        .collect()
}

fn vue_de<'a>(desc: &'a ModuleDescriptor, nom: &str) -> Result<&'a ViewDesc, String> {
    desc.views
        .iter()
        .find(|v| v.name == nom)
        .ok_or_else(|| format!("vue inconnue `{nom}`"))
}

fn brique_de(v: &ViewDesc) -> Result<&Brick, String> {
    match &v.root {
        Node::Leaf { brick } => Ok(brick),
        _ => Err(format!("la vue `{}` n'est pas une feuille", v.name)),
    }
}

fn entite_de<'a>(desc: &'a ModuleDescriptor, nom: &str) -> Result<&'a EntityDesc, String> {
    desc.entities
        .iter()
        .find(|e| e.name == nom)
        .ok_or_else(|| format!("entité inconnue `{nom}`"))
}

fn cellule(v: Option<&Value>, nature: Nature) -> Cellule {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Cellule::Vide;
    };
    match nature {
        Nature::Entier => Cellule::Montant(
            v.as_f64()
                .map(|n| format!("{n:.0}"))
                .unwrap_or_else(|| v.to_string()),
        ),
        Nature::Decimal => Cellule::Montant(
            v.as_f64()
                .map(|n| n.to_string())
                .unwrap_or_else(|| v.to_string()),
        ),
        Nature::Date => Cellule::Date(v.as_str().unwrap_or_default().to_owned()),
        Nature::Texte => Cellule::Texte(
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string()),
        ),
    }
}

/// `due_date` → « Due date ». Les identifiants de champs sont ASCII, donc un simple
/// `_` → espace et une capitale initiale suffisent.
fn libelle(champ: &str) -> String {
    let espaces = champ.replace('_', " ");
    let mut chars = espaces.chars();
    match chars.next() {
        None => espaces,
        Some(premier) => premier.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlanModule;
    use this_descriptor::{CheckProfile, CustomRegistry, EntityDesc, FieldDesc, ModuleDescriptor};
    use this_module_sdk::Module;

    /// Le descripteur réel du module, relu par le vrai `ModuleDescriptor`.
    fn descripteur() -> ModuleDescriptor {
        let json = serde_json::to_string(&PlanModule::manifest()).unwrap();
        let d = ModuleDescriptor::from_json(&json).expect("relu par le vrai descripteur");
        d.check(
            &CustomRegistry::default(),
            CheckProfile::Stored {
                allow_public: false,
            },
        )
        .expect("check(Stored)");
        d
    }

    /// Un descripteur minimal à une entité et une vue liste, pour isoler la règle.
    fn descripteur_minimal(triable: bool) -> ModuleDescriptor {
        let mut d = ModuleDescriptor::new("essai", "0.1.0");
        d.entities = vec![EntityDesc {
            fields: vec![FieldDesc {
                sortable: triable,
                ..FieldDesc::new("echeance", FieldType::Date)
            }],
            ..EntityDesc::new("tache")
        }];
        d.views = vec![serde_json::from_value(serde_json::json!({
            "name": "taches",
            "surface": "main",
            "root": { "type": "leaf", "brick": { "type": "list", "entity": "tache", "label": "echeance" } }
        }))
        .unwrap()];
        d
    }

    #[test]
    fn la_colonne_suit_le_drapeau_sortable_et_rien_dautre() {
        // Non triable → aucune colonne de donnée, même si le champ existe.
        let non_triable = descripteur_minimal(false);
        let ecran = liste(&non_triable, "taches").unwrap();
        assert!(
            ecran.colonnes.is_empty(),
            "rien à afficher sans champ triable"
        );

        // Triable → la colonne apparaît, sans changer `liste`.
        let triable = descripteur_minimal(true);
        let ecran = liste(&triable, "taches").unwrap();
        assert_eq!(ecran.colonnes.len(), 1);
        assert_eq!(ecran.colonnes[0].champ, "echeance");
        assert_eq!(ecran.colonnes[0].nature, Nature::Date);
    }

    #[test]
    fn la_vue_taches_expose_les_champs_triables_comme_colonnes() {
        let d = descripteur();
        let ecran = liste(&d, "taches").expect("vue `taches`");
        assert_eq!(ecran.entite, "task");
        assert_eq!(ecran.identite, "title");
        let champs: Vec<&str> = ecran.colonnes.iter().map(|c| c.champ.as_str()).collect();
        assert_eq!(champs, ["priority", "due_date"]);
        // priorité entière, échéance date.
        assert_eq!(ecran.colonnes[0].nature, Nature::Entier);
        assert_eq!(ecran.colonnes[1].nature, Nature::Date);
    }

    #[test]
    fn le_formulaire_nouvelle_tache_expose_ses_champs_et_leurs_contraintes() {
        let d = descripteur();
        let f = formulaire(&d, "nouvelle_tache").expect("vue `nouvelle_tache`");
        assert_eq!(f.entite, "task");
        assert_eq!(f.soumission.as_deref(), Some("creer_tache"));
        let champs: Vec<&str> = f.champs.iter().map(|c| c.champ.as_str()).collect();
        assert_eq!(champs, ["title", "description"]);
        // `title` est optionnel, `description` est requis (sans défaut).
        assert!(!f.champs[0].requis);
        assert_eq!(f.champs[0].saisie, "text");
        assert!(f.champs[1].requis);
        // le libellé déclaré dans la vue gagne sur l'humanisation.
        assert_eq!(f.champs[0].libelle, "Titre");
        assert_eq!(f.champs[1].libelle, "Description");
    }
}
