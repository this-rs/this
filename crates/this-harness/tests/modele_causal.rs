//! **Modèle causal exécutable du cycle logique d'attribution.**
//!
//! Ce fichier ne réutilise RIEN de `crates/this-harness/src/attribution.rs` : il
//! pose ses propres variables, ses propres transitions et ses propres invariants,
//! puis *compare* le résultat à l'implémentation réelle. Le but n'est pas de
//! décrire le code : c'est de faire tourner un modèle qui **retrouve tout seul**
//! les familles de défauts corrigées en `e1e6e611` (abandon direct) et
//! `dd6382be` (génération non vérifiée dans `resoudre`), sous forme de
//! contre-exemples **minimaux**.
//!
//! # Variables
//!
//! | lettre | variable | ici |
//! |---|---|---|
//! | D | décision (contenu) | `Def`, `Action::Record` |
//! | O | observation immuable | `Obs` |
//! | K | publication logique | `Cycle::s` (`S`) |
//! | B | backend | `Action::Resoudre` (verdict) |
//! | G | génération | `Cycle::g` |
//! | S | état opérationnel | `Cycle::s` |
//! | R | connaissance attestable | `Cycle::r` (`R`) |
//! | C | annulation | `Cycle::c` |
//! | P | provenance | `Prov` |
//! | E | **effet externe** (environnement) | `Monde::effets` |
//!
//! E appartient à l'environnement, pas au cycle : le modèle le porte comme une
//! variable fantôme, précisément pour pouvoir nommer le moment où S en dit plus
//! que ce que E permet. C'est toute la sûreté du cycle.
//!
//! # Invariants
//!
//! Sûreté — S1 observation immuable · S2 confirmation ⟹ effet attesté ·
//! S3 abandon ⟹ effet non inconnu · S4 réponse positive ⟹ provenance vivante ·
//! S5 provenance non vivante ⟹ aucune modification · S6 une seule tentative
//! vivante · S7 Indetermine ⟹ effet inconnu · S8 pas de ré-publication aveugle ·
//! S9 terminal absorbant · S10 annulation orthogonale.
//!
//! Progression — L1 pas de blocage · L2 absence attestée ⟹ reprise ·
//! L3 terminal atteignable · L4 rejeu idempotent · L5 tentative ⟹ observation.
//!
//! # Méthode
//!
//! 1. La **référence** (`Semantique::REFERENCE`) explore les séquences de
//!    transitions jusqu'à `PROFONDEUR`, en dédupliquant les états, et ne doit
//!    violer aucun invariant (`modele_reference_ne_viole_aucun_invariant`).
//! 2. Trois **variantes défectueuses** — un drapeau par famille corrigée —
//!    doivent, elles, produire un contre-exemple, et le plus court possible.
//!    C'est ce qui prouve que l'explorateur discrimine au lieu de dire « ok »
//!    partout.
//! 3. Le modèle de référence est **comparé** à `this_harness::attribution` :
//!    chaque transition de l'exploration est rejouée sur l'implémentation, et
//!    la réponse comme l'état d'arrivée doivent coïncider.
//!
//! # Registre
//!
//! **MESURÉ** — l'exploration de référence (états, transitions, invariants
//! satisfaits), la profondeur des trois contre-exemples minimaux, la conformité
//! de l'implémentation livrée sur chaque transition rejouée, et le refus des
//! transitions par lesquelles les variantes s'écartent de la référence. Les
//! chiffres sont ceux que `cargo test -p this-harness --test modele_causal --
//! --nocapture rapport` imprime ; ils ne sont pas recopiés ici pour ne pas
//! dériver.
//!
//! **RELU** — la correspondance entre chaque variante et le code supprimé, lue
//! dans les diffs de `e1e6e611` (suppression de `abandonner(id, Tentative)` et
//! du `true` inconditionnel de `resoudre(Indetermine)`) et de `dd6382be`
//! (contrôle de génération ajouté dans les bras `Indetermine` et `Incompatible`).
//! Elle est réécrite ici en sémantique, pas importée du code : le modèle reste
//! indépendant de l'implémentation.
//!
//! **SUPPOSÉ** — rien.
//!
//! # Asymétrie assumée entre le modèle et l'implémentation
//!
//! Dans le modèle, `Prov` est une donnée : on peut présenter n'importe quel
//! triplet (autorité, décision, génération). Dans l'implémentation, `Tentative`
//! est **opaque** et seule `reserver` en émet une : une provenance jamais émise
//! n'est pas présentable. Les présentations que le rejeu ne peut pas exprimer
//! sont donc comptées (`non exprimables`) et non comparées. Les deux dimensions
//! ainsi perdues sont couvertes par des sondes dédiées, exhaustives sur les cinq
//! méthodes qui portent une provenance : autorité étrangère et décision croisée
//! (`sonde_provenances_non_presentables`). La génération jamais réservée, elle,
//! reste inexprimable par l'API publique — mais un appelant blanc (test unitaire
//! du module, ou appelant fautif) peut la présenter, et c'est précisément la
//! surface où `dd6382be` se manifeste.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::OnceLock;

use this_harness::attribution as reel;
use this_harness::attribution::{
    Definition, Etat as EtatReel, Observation, Resultat, RésolutionBackend,
};

// ===========================================================================
// 1. Variables du modèle
// ===========================================================================

/// D — identifiant de décision.
type Dec = u8;

/// P — identité d'autorité : qui émet une tentative.
type Aut = u8;

/// L'autorité du monde que l'on explore. Les provenances qui portent une AUTRE
/// autorité sont étrangères et doivent être refusées.
const AUTORITE: Aut = 0;

const DECISIONS: [Dec; 2] = [0, 1];
const AUTORITES: [Aut; 2] = [0, 1];
/// Les générations qu'une provenance peut *présenter*. Le cycle, lui, peut
/// monter plus haut : c'est ce qui rend une provenance « périmée ».
const GEN_PRESENTEES: [u64; 2] = [1, 2];

/// D — le contenu d'une décision : ce qui doit rester identique pour un rejeu.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Def {
    identite: u8,
    prevue: (u8, u8),
    servie: (u8, u8),
}

/// O — l'observation. Une fois reçue, elle est immuable.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Obs {
    fournisseur: u8,
    modele: u8,
    resultat: u8,
}

/// Les **deux contenus** : même action prévue, cible servie différente. Le
/// contenu 1 rend toute observation `Incompatible`.
const CONTENUS: [Def; 2] = [
    Def {
        identite: 1,
        prevue: (1, 1),
        servie: (1, 1),
    },
    Def {
        identite: 2,
        prevue: (1, 1),
        servie: (2, 2),
    },
];

/// Les **deux contenus d'observation** : même cible que `CONTENUS[0].servie`,
/// résultat divergent → une seconde réception est `Divergente`.
const OBSERVATIONS: [Obs; 2] = [
    Obs {
        fournisseur: 1,
        modele: 1,
        resultat: 0,
    },
    Obs {
        fournisseur: 1,
        modele: 1,
        resultat: 1,
    },
];

/// S — l'état opérationnel (K, la publication logique, est portée par lui).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum S {
    Ouvert,
    Recu,
    EnCours,
    Indetermine,
    Confirme,
    Abandonne,
}

/// E — l'effet externe, côté environnement.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum E {
    /// Aucun effet, et c'est certain.
    Aucun,
    /// L'effet existe et il est attesté.
    Present,
    /// On ne sait pas : une publication est engagée, aucun verdict n'est arrivé.
    Inconnu,
}

/// P — provenance d'une tentative.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Prov {
    aut: Aut,
    dec: Dec,
    generation: u64,
}

/// R — la connaissance attestable : ce que le backend a prouvé, et pour quelle
/// génération.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
struct R {
    effet: Option<u64>,
    absence: Option<u64>,
}

/// L'état d'un cycle.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Cycle {
    def: Def,
    o: Option<Obs>,
    s: S,
    g: u64,
    c: bool,
    vivante: Option<Prov>,
    r: R,
}

impl Cycle {
    fn neuf(def: Def) -> Self {
        Self {
            def,
            o: None,
            s: S::Ouvert,
            g: 0,
            c: false,
            vivante: None,
            r: R::default(),
        }
    }

    fn terminal(&self) -> bool {
        matches!(self.s, S::Confirme | S::Abandonne)
    }
}

/// L'état du système : les cycles + l'effet de l'environnement.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
struct Monde {
    cycles: BTreeMap<Dec, Cycle>,
    effets: BTreeMap<Dec, E>,
}

impl Monde {
    fn effet(&self, d: Dec) -> E {
        self.effets.get(&d).copied().unwrap_or(E::Aucun)
    }
}

// ===========================================================================
// 2. Alphabet de transitions
// ===========================================================================

/// Verdict du backend pour une résolution (B).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Verdict {
    EffetConfirme,
    AbsenceDefinitive,
    Indetermine,
    Incompatible,
}

const VERDICTS: [Verdict; 4] = [
    Verdict::EffetConfirme,
    Verdict::AbsenceDefinitive,
    Verdict::Indetermine,
    Verdict::Incompatible,
];

/// Une transition. Deux décisions, deux autorités, deux générations et deux
/// contenus se croisent dans cet alphabet.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Action {
    Record {
        d: Dec,
        contenu: usize,
    },
    Recevoir {
        d: Dec,
        obs: usize,
    },
    Reserver {
        d: Dec,
    },
    Confirmer {
        d: Dec,
        p: Prov,
    },
    EchecCertain {
        d: Dec,
        p: Prov,
    },
    Indeterminer {
        d: Dec,
        p: Prov,
    },
    Resoudre {
        d: Dec,
        p: Prov,
        verdict: Verdict,
    },
    /// La transition **supprimée** en `e1e6e611` : abandon direct d'une
    /// tentative engagée. Dans la référence elle n'existe pas (toujours refusée).
    Abandonner {
        d: Dec,
        p: Prov,
    },
    AbandonSansTentative {
        d: Dec,
    },
    DemanderAnnulation {
        d: Dec,
    },
}

/// Réponse du cycle à une transition. Les variantes nommées sont celles de
/// l'implémentation réelle ; `Oui`/`Non` sont les booléens.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Issue {
    Enregistre,
    Rejeu,
    Conflit,
    Recu,
    Divergente,
    Incompatible,
    Termine,
    Absent,
    Oui,
    Non,
}

impl Issue {
    /// « Réponse positive » : le cycle prétend avoir fait ce qu'on lui demande.
    fn positive(self) -> bool {
        matches!(self, Issue::Enregistre | Issue::Recu | Issue::Oui)
    }
}

// ===========================================================================
// 3. Sémantique : la référence, et les variantes défectueuses
// ===========================================================================

/// La sémantique **de référence** est celle qui respecte les invariants. Les
/// trois drapeaux reproduisent, chacun, une famille de défauts historiquement
/// corrigée : ils servent à prouver que l'explorateur les retrouve.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Semantique {
    /// ⚑ `e1e6e611` (a) — `abandonner(id, Tentative)` : `EnCours|Indetermine →
    /// Abandonne` sans attestation.
    abandon_direct: bool,
    /// ⚑ `e1e6e611` (b) — `resoudre(Indetermine) => true` sans validation.
    indetermine_sans_validation: bool,
    /// ⚑ `dd6382be` — `resoudre(Indetermine|Incompatible)` sans contrôle de
    /// génération.
    inconnue_sans_generation: bool,
}

impl Semantique {
    const REFERENCE: Self = Self {
        abandon_direct: false,
        indetermine_sans_validation: false,
        inconnue_sans_generation: false,
    };

    const ABANDON_DIRECT: Self = Self {
        abandon_direct: true,
        indetermine_sans_validation: false,
        inconnue_sans_generation: false,
    };

    const INDETERMINE_SANS_VALIDATION: Self = Self {
        abandon_direct: false,
        indetermine_sans_validation: true,
        inconnue_sans_generation: false,
    };

    const INCONNUE_SANS_GENERATION: Self = Self {
        abandon_direct: false,
        indetermine_sans_validation: false,
        inconnue_sans_generation: true,
    };
}

// ===========================================================================
// 4. Transitions
// ===========================================================================

impl Monde {
    /// D : enregistrer une décision. Un contenu différent sur la même décision
    /// est un CONFLIT, jamais un écrasement.
    fn record(&mut self, d: Dec, def: Def) -> Issue {
        match self.cycles.get(&d) {
            Some(c) if c.def == def => Issue::Rejeu,
            Some(_) => Issue::Conflit,
            None => {
                self.cycles.insert(d, Cycle::neuf(def));
                self.effets.insert(d, E::Aucun);
                Issue::Enregistre
            }
        }
    }

    /// O : recevoir une observation. Immuable : une seconde observation
    /// divergente est refusée et la première reste.
    fn recevoir(&mut self, d: Dec, o: Obs) -> Issue {
        let Some(c) = self.cycles.get_mut(&d) else {
            return Issue::Absent;
        };
        if c.terminal() {
            return Issue::Termine;
        }
        if (o.fournisseur, o.modele) != c.def.servie {
            return Issue::Incompatible;
        }
        match c.o {
            Some(x) if x == o => Issue::Rejeu,
            Some(_) => Issue::Divergente,
            None => {
                c.o = Some(o);
                c.s = S::Recu;
                Issue::Recu
            }
        }
    }

    /// G + K : réserver une tentative. Un seul écrivain à la fois, et seulement
    /// après une observation. La publication étant ENGAGÉE, l'effet devient
    /// inconnu : c'est le fait causal que le cycle doit ensuite respecter.
    fn reserver(&mut self, d: Dec) -> Option<Prov> {
        let c = self.cycles.get_mut(&d)?;
        if c.s != S::Recu {
            return None;
        }
        c.g += 1;
        c.s = S::EnCours;
        let p = Prov {
            aut: AUTORITE,
            dec: d,
            generation: c.g,
        };
        c.vivante = Some(p);
        self.effets.insert(d, E::Inconnu);
        Some(p)
    }

    fn provenance_valide(&self, d: Dec, p: Prov) -> bool {
        p.aut == AUTORITE && p.dec == d
    }

    /// La provenance présentée EST la tentative vivante de cette décision :
    /// même autorité, même décision, même génération.
    fn vivante_meme(&self, d: Dec, p: Prov) -> bool {
        self.cycles.get(&d).and_then(|c| c.vivante) == Some(p)
    }

    /// L'effet est attesté. Terminal.
    fn confirmer(&mut self, d: Dec, p: Prov) -> bool {
        if !self.provenance_valide(d, p) || !self.vivante_meme(d, p) {
            return false;
        }
        let Some(c) = self.cycles.get_mut(&d) else {
            return false;
        };
        c.s = S::Confirme;
        c.r.effet = Some(p.generation);
        c.vivante = None;
        self.effets.insert(d, E::Present);
        true
    }

    /// L'absence d'effet est certaine et définitive : la réservation est
    /// relâchée, une nouvelle tentative devient possible.
    fn echec_certain(&mut self, d: Dec, p: Prov) -> bool {
        if !self.provenance_valide(d, p) || !self.vivante_meme(d, p) {
            return false;
        }
        let Some(c) = self.cycles.get_mut(&d) else {
            return false;
        };
        c.s = S::Recu;
        c.r.absence = Some(p.generation);
        c.vivante = None;
        self.effets.insert(d, E::Aucun);
        true
    }

    /// La publication est d'issue inconnue. On ne sait RIEN de plus qu'avant.
    fn indeterminer(&mut self, d: Dec, p: Prov) -> bool {
        if !self.provenance_valide(d, p) || !self.vivante_meme(d, p) {
            return false;
        }
        let Some(c) = self.cycles.get_mut(&d) else {
            return false;
        };
        if c.s != S::EnCours {
            return false; // on ne « re-déclare » pas indéterminé ce qui l'est déjà
        }
        c.s = S::Indetermine;
        true
    }

    /// Abandon AVANT toute tentative : aucune écriture n'est engagée, donc
    /// l'effet est certainement absent. Terminal.
    fn abandonner_sans_tentative(&mut self, d: Dec) -> bool {
        match self.cycles.get_mut(&d) {
            Some(c) if matches!(c.s, S::Ouvert | S::Recu) => {
                c.s = S::Abandonne;
                true
            }
            _ => false,
        }
    }

    /// ⚑ Transition supprimée en `e1e6e611` : abandon d'une tentative ENGAGÉE.
    /// Refusée dans la référence — une transition RAM ne peut pas masquer un
    /// effet déjà engagé.
    fn abandonner(&mut self, d: Dec, p: Prov, sem: &Semantique) -> bool {
        if !sem.abandon_direct {
            return false;
        }
        if !self.provenance_valide(d, p) || !self.vivante_meme(d, p) {
            return false;
        }
        let Some(c) = self.cycles.get_mut(&d) else {
            return false;
        };
        c.s = S::Abandonne;
        c.vivante = None;
        true
    }

    /// Résolution d'une publication indéterminée, selon le verdict du backend.
    fn resoudre(&mut self, d: Dec, p: Prov, v: Verdict, sem: &Semantique) -> bool {
        match v {
            Verdict::EffetConfirme => self.confirmer(d, p),
            Verdict::AbsenceDefinitive => self.echec_certain(d, p),
            Verdict::Indetermine => {
                // ⚑ e1e6e611 (b)
                if sem.indetermine_sans_validation {
                    return true;
                }
                // ⚑ dd6382be, bras Indetermine
                if sem.inconnue_sans_generation {
                    if !self.provenance_valide(d, p) {
                        return false;
                    }
                    return matches!(
                        self.cycles.get(&d).map(|c| c.s),
                        Some(S::EnCours) | Some(S::Indetermine)
                    );
                }
                if !self.provenance_valide(d, p) || !self.vivante_meme(d, p) {
                    return false;
                }
                matches!(
                    self.cycles.get(&d).map(|c| c.s),
                    Some(S::EnCours) | Some(S::Indetermine)
                )
            }
            Verdict::Incompatible => {
                // ⚑ dd6382be, bras Incompatible
                if sem.inconnue_sans_generation {
                    if !self.provenance_valide(d, p) {
                        return false;
                    }
                    if let Some(c) = self.cycles.get_mut(&d)
                        && matches!(c.s, S::EnCours | S::Indetermine)
                    {
                        c.s = S::Indetermine;
                    }
                    return false;
                }
                if !self.provenance_valide(d, p) || !self.vivante_meme(d, p) {
                    return false;
                }
                let Some(c) = self.cycles.get_mut(&d) else {
                    return false;
                };
                c.s = S::Indetermine;
                false
            }
        }
    }

    /// C — demande d'annulation, ORTHOGONALE à la publication : elle ne dit
    /// rien de l'effet externe.
    fn demander_annulation(&mut self, d: Dec) -> bool {
        match self.cycles.get_mut(&d) {
            Some(c) if !c.terminal() => {
                c.c = true;
                true
            }
            _ => false,
        }
    }

    fn appliquer(&mut self, a: Action, sem: &Semantique) -> Issue {
        match a {
            Action::Record { d, contenu } => self.record(d, CONTENUS[contenu]),
            Action::Recevoir { d, obs } => self.recevoir(d, OBSERVATIONS[obs]),
            Action::Reserver { d } => self.reserver(d).map_or(Issue::Non, |_| Issue::Oui),
            Action::Confirmer { d, p } => b(self.confirmer(d, p)),
            Action::EchecCertain { d, p } => b(self.echec_certain(d, p)),
            Action::Indeterminer { d, p } => b(self.indeterminer(d, p)),
            Action::Resoudre { d, p, verdict } => b(self.resoudre(d, p, verdict, sem)),
            Action::Abandonner { d, p } => b(self.abandonner(d, p, sem)),
            Action::AbandonSansTentative { d } => b(self.abandonner_sans_tentative(d)),
            Action::DemanderAnnulation { d } => b(self.demander_annulation(d)),
        }
    }
}

fn b(x: bool) -> Issue {
    if x { Issue::Oui } else { Issue::Non }
}

// ===========================================================================
// 5. Invariants
// ===========================================================================

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Invariant {
    /// S1 — O, une fois posée, ne change plus jamais.
    S1ObservationImmuable,
    /// S2 — S = Confirme ⟹ l'effet est attesté (E = Présent) pour la génération courante.
    S2ConfirmationSansAttestation,
    /// S3 — S = Abandonne ⟹ l'effet n'est pas inconnu (on ne déclare pas
    /// abandonné ce qui peut exister).
    S3AbandonMasqueEffet,
    /// S4 — une réponse POSITIVE porte la provenance de la tentative vivante.
    S4ReponsePositiveSansProvenanceVivante,
    /// S5 — une provenance non vivante (étrangère, croisée, périmée) ne modifie
    /// RIEN. Inclut la localité : une action sur d ne touche que d.
    S5ProvenanceNonVivanteModifie,
    /// S6 — S ∈ {EnCours, Indetermine} ⟺ exactement une tentative vivante.
    S6TentativeVivante,
    /// S7 — S = Indetermine ⟹ E = Inconnu (indétermination = ignorance de l'effet).
    S7IndetermineSansIgnorance,
    /// S8 — pas de ré-publication aveugle : on ne réserve que depuis Recu.
    S8RepublicationAveugle,
    /// S9 — Confirme et Abandonne sont absorbants, et n'obtiennent pas de réponse positive.
    S9TerminalNonAbsorbant,
    /// S10 — l'annulation est orthogonale : elle ne touche ni S, ni O, ni G, ni R, ni E.
    S10AnnulationNonOrthogonale,
    /// L1 — pas de blocage : tout état non terminal a une transition effective.
    L1Blocage,
    /// L2 — absence attestée ⟹ nouvelle tentative possible.
    L2PasDeReprise,
    /// L3 — un état terminal est atteignable depuis tout état exploré.
    L3TerminalInatteignable,
    /// L4 — le rejeu (même décision, même observation) est idempotent.
    L4RejeuNonIdempotent,
    /// L5 — pas de tentative sans observation.
    L5TentativeSansObservation,
}

impl Invariant {
    fn nom(self) -> &'static str {
        match self {
            Invariant::S1ObservationImmuable => "S1  observation immuable",
            Invariant::S2ConfirmationSansAttestation => "S2  confirmation sans effet attesté",
            Invariant::S3AbandonMasqueEffet => "S3  abandon masquant un effet possible",
            Invariant::S4ReponsePositiveSansProvenanceVivante => {
                "S4  réponse positive sans provenance vivante"
            }
            Invariant::S5ProvenanceNonVivanteModifie => {
                "S5  provenance non vivante qui modifie l'état"
            }
            Invariant::S6TentativeVivante => "S6  tentative vivante incohérente",
            Invariant::S7IndetermineSansIgnorance => "S7  Indetermine sans ignorance de l'effet",
            Invariant::S8RepublicationAveugle => "S8  ré-publication aveugle",
            Invariant::S9TerminalNonAbsorbant => "S9  terminal non absorbant",
            Invariant::S10AnnulationNonOrthogonale => "S10 annulation non orthogonale",
            Invariant::L1Blocage => "L1  blocage",
            Invariant::L2PasDeReprise => "L2  pas de reprise après absence attestée",
            Invariant::L3TerminalInatteignable => "L3  terminal inatteignable",
            Invariant::L4RejeuNonIdempotent => "L4  rejeu non idempotent",
            Invariant::L5TentativeSansObservation => "L5  tentative sans observation",
        }
    }
}

const TOUS_LES_INVARIANTS: [Invariant; 15] = [
    Invariant::S1ObservationImmuable,
    Invariant::S2ConfirmationSansAttestation,
    Invariant::S3AbandonMasqueEffet,
    Invariant::S4ReponsePositiveSansProvenanceVivante,
    Invariant::S5ProvenanceNonVivanteModifie,
    Invariant::S6TentativeVivante,
    Invariant::S7IndetermineSansIgnorance,
    Invariant::S8RepublicationAveugle,
    Invariant::S9TerminalNonAbsorbant,
    Invariant::S10AnnulationNonOrthogonale,
    Invariant::L1Blocage,
    Invariant::L2PasDeReprise,
    Invariant::L3TerminalInatteignable,
    Invariant::L4RejeuNonIdempotent,
    Invariant::L5TentativeSansObservation,
];

/// Invariants **d'état** : ce qui doit être vrai de tout état atteignable.
fn violations_etat(m: &Monde) -> Vec<Invariant> {
    let mut v = Vec::new();
    for (d, c) in &m.cycles {
        let e = m.effet(*d);
        match c.s {
            S::Confirme => {
                if e != E::Present || c.r.effet != Some(c.g) {
                    v.push(Invariant::S2ConfirmationSansAttestation);
                }
            }
            S::Abandonne => {
                if e == E::Inconnu || c.vivante.is_some() {
                    v.push(Invariant::S3AbandonMasqueEffet);
                }
            }
            S::Indetermine if e != E::Inconnu => {
                v.push(Invariant::S7IndetermineSansIgnorance);
            }
            _ => {}
        }
        // S6 : une tentative vivante si et seulement si une tentative est engagée.
        let attendu = matches!(c.s, S::EnCours | S::Indetermine);
        if attendu != c.vivante.is_some() {
            v.push(Invariant::S6TentativeVivante);
        }
    }
    v
}

/// Invariants de **transition** : ce qu'une transition ne doit pas faire.
fn violations_transition(avant: &Monde, a: Action, issue: Issue, apres: &Monde) -> Vec<Invariant> {
    let mut v = Vec::new();

    // S1 — O immuable.
    for (d, ca) in &avant.cycles {
        if let Some(o) = ca.o
            && apres.cycles.get(d).and_then(|c| c.o) != Some(o)
        {
            v.push(Invariant::S1ObservationImmuable);
        }
    }

    // Localité : une action sur d ne touche que d.
    if let Some(d) = decision_visee(a) {
        for (autre, ca) in &avant.cycles {
            if *autre != d && apres.cycles.get(autre) != Some(ca) {
                v.push(Invariant::S5ProvenanceNonVivanteModifie);
            }
        }
        for (autre, e) in &avant.effets {
            if *autre != d && apres.effets.get(autre) != Some(e) {
                v.push(Invariant::S5ProvenanceNonVivanteModifie);
            }
        }
    }

    // S4 et S5 — pour les actions qui PORTENT une provenance.
    if let Some(p) = provenance_portee(a)
        && let Some(d) = decision_visee(a)
    {
        let vivante = avant.cycles.get(&d).and_then(|c| c.vivante);
        if issue.positive() && vivante != Some(p) {
            v.push(Invariant::S4ReponsePositiveSansProvenanceVivante);
        }
        if vivante != Some(p) && avant != apres {
            v.push(Invariant::S5ProvenanceNonVivanteModifie);
        }
    }

    // S8 — on ne réserve que depuis Recu.
    if let Action::Reserver { d } = a
        && issue.positive()
        && avant.cycles.get(&d).map(|c| c.s) != Some(S::Recu)
    {
        v.push(Invariant::S8RepublicationAveugle);
    }

    // S9 — terminal absorbant, et jamais de réponse positive depuis un terminal.
    if let Some(d) = decision_visee(a)
        && let Some(ca) = avant.cycles.get(&d)
        && ca.terminal()
    {
        if apres.cycles.get(&d).map(|c| c.s) != Some(ca.s) {
            v.push(Invariant::S9TerminalNonAbsorbant);
        }
        if issue.positive() {
            v.push(Invariant::S9TerminalNonAbsorbant);
        }
    }

    // S10 — l'annulation ne touche que C.
    if let Action::DemanderAnnulation { .. } = a {
        let sans_c = |m: &Monde| -> Vec<(Dec, Cycle)> {
            m.cycles
                .iter()
                .map(|(d, c)| {
                    (
                        *d,
                        Cycle {
                            c: false,
                            ..c.clone()
                        },
                    )
                })
                .collect()
        };
        if sans_c(avant) != sans_c(apres) || avant.effets != apres.effets {
            v.push(Invariant::S10AnnulationNonOrthogonale);
        }
    }

    v
}

fn decision_visee(a: Action) -> Option<Dec> {
    match a {
        Action::Record { d, .. }
        | Action::Recevoir { d, .. }
        | Action::Reserver { d }
        | Action::Confirmer { d, .. }
        | Action::EchecCertain { d, .. }
        | Action::Indeterminer { d, .. }
        | Action::Resoudre { d, .. }
        | Action::Abandonner { d, .. }
        | Action::AbandonSansTentative { d }
        | Action::DemanderAnnulation { d } => Some(d),
    }
}

fn provenance_portee(a: Action) -> Option<Prov> {
    match a {
        Action::Confirmer { p, .. }
        | Action::EchecCertain { p, .. }
        | Action::Indeterminer { p, .. }
        | Action::Resoudre { p, .. }
        | Action::Abandonner { p, .. } => Some(p),
        _ => None,
    }
}

// ===========================================================================
// 6. Explorateur : séquences de transitions de profondeur bornée
// ===========================================================================

fn toutes_provenances() -> Vec<Prov> {
    let mut v = Vec::new();
    for aut in AUTORITES {
        for dec in DECISIONS {
            for generation in GEN_PRESENTEES {
                v.push(Prov {
                    aut,
                    dec,
                    generation,
                });
            }
        }
    }
    v
}

fn provenances_propres(d: Dec) -> Vec<Prov> {
    GEN_PRESENTEES
        .iter()
        .map(|&generation| Prov {
            aut: AUTORITE,
            dec: d,
            generation,
        })
        .collect()
}

/// Les transitions qui PORTENT une provenance donnée. Elle peut être vivante,
/// périmée, étrangère ou croisée : c'est l'action, pas le modèle, qui décide.
fn actions_de_provenance(d: Dec, p: Prov) -> Vec<Action> {
    let mut v = vec![
        Action::Confirmer { d, p },
        Action::EchecCertain { d, p },
        Action::Indeterminer { d, p },
        Action::Abandonner { d, p },
    ];
    for verdict in VERDICTS {
        v.push(Action::Resoudre { d, p, verdict });
    }
    v
}

fn alphabet(decisions: &[Dec], provenances: &[Prov]) -> Vec<Action> {
    let mut a = Vec::new();
    for &d in decisions {
        for contenu in 0..CONTENUS.len() {
            a.push(Action::Record { d, contenu });
        }
        for obs in 0..OBSERVATIONS.len() {
            a.push(Action::Recevoir { d, obs });
        }
        a.push(Action::Reserver { d });
        a.push(Action::AbandonSansTentative { d });
        a.push(Action::DemanderAnnulation { d });
        for &p in provenances {
            a.extend(actions_de_provenance(d, p));
        }
    }
    a
}

/// Un contre-exemple minimal : l'invariant violé, sa profondeur, et la séquence
/// de transitions qui y mène.
#[derive(Clone, Debug)]
struct ContreExemple {
    trace: Vec<Action>,
}

impl ContreExemple {
    fn profondeur(&self) -> usize {
        self.trace.len()
    }

    fn lisible(&self) -> String {
        let etapes: Vec<String> = self.trace.iter().map(action_lisible).collect();
        etapes.join(" ; ")
    }
}

struct Noeud {
    monde: Monde,
    parent: Option<usize>,
    action: Option<Action>,
    profondeur: usize,
}

struct Exploration {
    noeuds: Vec<Noeud>,
    /// (source, action, cible, réponse)
    aretes: Vec<(usize, Action, usize, Issue)>,
    contre_exemples: BTreeMap<Invariant, ContreExemple>,
    acceptees: usize,
    profondeur_max: usize,
}

impl Exploration {
    fn trace_de(&self, mut i: usize) -> Vec<Action> {
        let mut t = Vec::new();
        while let Some(n) = self.noeuds.get(i) {
            if let Some(a) = n.action {
                t.push(a);
            }
            match n.parent {
                Some(p) => i = p,
                None => break,
            }
        }
        t.reverse();
        t
    }

    fn contre_exemple(&self, inv: Invariant) -> Option<&ContreExemple> {
        self.contre_exemples.get(&inv)
    }

    fn violate(&self, inv: Invariant) -> bool {
        self.contre_exemples.contains_key(&inv)
    }

    /// Une action a-t-elle été acceptée au moins une fois ?
    fn action_acceptee(&self, pred: impl Fn(Action) -> bool) -> bool {
        self.aretes
            .iter()
            .any(|(_, a, _, i)| pred(*a) && i.positive())
    }

    fn etat_atteint(&self, pred: impl Fn(&Monde) -> bool) -> bool {
        self.noeuds.iter().any(|n| pred(&n.monde))
    }

    /// L3 — atteignabilité d'un état terminal, par remontée depuis les terminaux.
    fn terminaux_atteignables(&self) -> BTreeSet<usize> {
        let mut inverses: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, _, j, _) in &self.aretes {
            inverses.entry(*j).or_default().push(*i);
        }
        let mut atteints: BTreeSet<usize> = BTreeSet::new();
        let mut file = VecDeque::new();
        for (i, n) in self.noeuds.iter().enumerate() {
            if tous_terminaux(&n.monde) {
                atteints.insert(i);
                file.push_back(i);
            }
        }
        while let Some(j) = file.pop_front() {
            for &i in inverses.get(&j).map(|v| v.as_slice()).unwrap_or(&[]) {
                if atteints.insert(i) {
                    file.push_back(i);
                }
            }
        }
        atteints
    }
}

fn tous_terminaux(m: &Monde) -> bool {
    !m.cycles.is_empty() && m.cycles.values().all(|c| c.terminal())
}

fn explorer(
    sem: &Semantique,
    decisions: &[Dec],
    provenances: &[Prov],
    profondeur: usize,
) -> Exploration {
    let base = alphabet(decisions, provenances);
    let depart = Monde::default();
    let mut noeuds = vec![Noeud {
        monde: depart.clone(),
        parent: None,
        action: None,
        profondeur: 0,
    }];
    let mut index: BTreeMap<Monde, usize> = BTreeMap::new();
    index.insert(depart, 0);
    let mut aretes: Vec<(usize, Action, usize, Issue)> = Vec::new();
    let mut contre_exemples: BTreeMap<Invariant, ContreExemple> = BTreeMap::new();
    let mut acceptees = 0usize;

    for inv in violations_etat(&noeuds[0].monde) {
        contre_exemples
            .entry(inv)
            .or_insert_with(|| ContreExemple { trace: Vec::new() });
    }

    let mut file: VecDeque<usize> = VecDeque::new();
    file.push_back(0);

    while let Some(i) = file.pop_front() {
        if noeuds[i].profondeur >= profondeur {
            continue;
        }
        let avant = noeuds[i].monde.clone();
        let mut actions: BTreeSet<Action> = base.iter().copied().collect();
        for (d, c) in &avant.cycles {
            if let Some(p) = c.vivante {
                actions.extend(actions_de_provenance(*d, p));
            }
        }
        for a in &actions {
            let mut apres = avant.clone();
            let issue = apres.appliquer(*a, sem);
            if issue.positive() {
                acceptees += 1;
            }
            for inv in violations_transition(&avant, *a, issue, &apres) {
                contre_exemples.entry(inv).or_insert_with(|| {
                    let mut trace = trace_de(&noeuds, i);
                    trace.push(*a);
                    ContreExemple { trace }
                });
            }
            if apres == avant {
                aretes.push((i, *a, i, issue));
                continue;
            }
            let j = match index.get(&apres) {
                Some(&j) => j,
                None => {
                    let j = noeuds.len();
                    noeuds.push(Noeud {
                        monde: apres.clone(),
                        parent: Some(i),
                        action: Some(*a),
                        profondeur: noeuds[i].profondeur + 1,
                    });
                    index.insert(apres, j);
                    file.push_back(j);
                    for inv in violations_etat(&noeuds[j].monde) {
                        contre_exemples.entry(inv).or_insert_with(|| {
                            let trace = trace_de(&noeuds, j);
                            ContreExemple { trace }
                        });
                    }
                    j
                }
            };
            aretes.push((i, *a, j, issue));
        }
    }

    Exploration {
        noeuds,
        aretes,
        contre_exemples,
        acceptees,
        profondeur_max: profondeur,
    }
}

fn trace_de(noeuds: &[Noeud], mut i: usize) -> Vec<Action> {
    let mut t = Vec::new();
    loop {
        let n = &noeuds[i];
        if let Some(a) = n.action {
            t.push(a);
        }
        match n.parent {
            Some(p) => i = p,
            None => break,
        }
    }
    t.reverse();
    t
}

fn action_lisible(a: &Action) -> String {
    match a {
        Action::Record { d, contenu } => format!("record(d{d},k{contenu})"),
        Action::Recevoir { d, obs } => format!("recevoir(d{d},o{obs})"),
        Action::Reserver { d } => format!("reserver(d{d})"),
        Action::Confirmer { d, p } => format!("confirmer(d{d},{})", prov_lisible(p)),
        Action::EchecCertain { d, p } => format!("echec_certain(d{d},{})", prov_lisible(p)),
        Action::Indeterminer { d, p } => format!("indeterminer(d{d},{})", prov_lisible(p)),
        Action::Resoudre { d, p, verdict } => {
            let v = match verdict {
                Verdict::EffetConfirme => "effet_confirme",
                Verdict::AbsenceDefinitive => "absence_definitive",
                Verdict::Indetermine => "indetermine",
                Verdict::Incompatible => "incompatible",
            };
            format!("resoudre(d{d},{},{v})", prov_lisible(p))
        }
        Action::Abandonner { d, p } => format!("abandonner(d{d},{})", prov_lisible(p)),
        Action::AbandonSansTentative { d } => format!("abandon_sans_tentative(d{d})"),
        Action::DemanderAnnulation { d } => format!("demander_annulation(d{d})"),
    }
}

fn prov_lisible(p: &Prov) -> String {
    format!("a{}/d{}/g{}", p.aut, p.dec, p.generation)
}

// ===========================================================================
// 7. Comparaison avec l'implémentation réelle
// ===========================================================================

fn def_reelle(d: Def) -> Definition {
    Definition {
        identite: Some(format!("k{}", d.identite)),
        prevue: Some((format!("p{}", d.prevue.0), format!("q{}", d.prevue.1))),
        servie: Some((format!("f{}", d.servie.0), format!("m{}", d.servie.1))),
    }
}

fn obs_reelle(o: Obs) -> Observation {
    Observation {
        fournisseur: format!("f{}", o.fournisseur),
        modele: format!("m{}", o.modele),
        resultat: if o.resultat == 0 {
            Resultat::Succes
        } else {
            Resultat::Echec
        },
    }
}

fn cle_obs_reelle(o: &Observation) -> (String, String, String) {
    (
        o.fournisseur.clone(),
        o.modele.clone(),
        format!("{:?}", o.resultat),
    )
}

fn cle_obs_modele(o: &Obs) -> (String, String, String) {
    cle_obs_reelle(&obs_reelle(*o))
}

fn etat_modele(e: &EtatReel) -> S {
    match *e {
        EtatReel::Ouvert => S::Ouvert,
        EtatReel::Recu => S::Recu,
        EtatReel::EnCours => S::EnCours,
        EtatReel::Indetermine => S::Indetermine,
        EtatReel::Confirme => S::Confirme,
        EtatReel::Abandonne => S::Abandonne,
    }
}

/// Projection comparable d'un état : (décision, S, O, C).
type Projection = Vec<(Dec, S, Option<(String, String, String)>, bool)>;

fn projection(m: &Monde) -> Projection {
    m.cycles
        .iter()
        .map(|(d, c)| (*d, c.s, c.o.map(|o| cle_obs_modele(&o)), c.c))
        .collect()
}

/// L'implémentation réelle, vue par le modèle. Les `Tentative` sont opaques :
/// on ne les fabrique pas, on conserve seulement celles que `reserver` a
/// réellement émises, indexées par la provenance que le modèle leur donne.
struct Reel {
    a: reel::Attribution,
    t: BTreeMap<Prov, reel::Tentative>,
    generation: BTreeMap<Dec, u64>,
}

impl Reel {
    fn neuf() -> Self {
        Self {
            a: reel::Attribution::new(),
            t: BTreeMap::new(),
            generation: BTreeMap::new(),
        }
    }

    fn projection(&self) -> Projection {
        let mut v = Vec::new();
        for d in DECISIONS {
            if let Some(c) = self.a.etat(d as u128) {
                v.push((
                    d,
                    etat_modele(&c.etat),
                    c.observation.as_ref().map(cle_obs_reelle),
                    c.annulation_demandee,
                ));
            }
        }
        v
    }

    /// `None` : l'action n'est pas exprimable contre l'implémentation réelle —
    /// la provenance présentée n'a jamais été émise par cette instance.
    fn appliquer(&mut self, a: Action) -> Option<Issue> {
        Some(match a {
            Action::Record { d, contenu } => {
                match self.a.record(d as u128, def_reelle(CONTENUS[contenu])) {
                    reel::Record::Enregistre => Issue::Enregistre,
                    reel::Record::Rejeu => Issue::Rejeu,
                    reel::Record::Conflit => Issue::Conflit,
                }
            }
            Action::Recevoir { d, obs } => {
                match self.a.recevoir(d as u128, obs_reelle(OBSERVATIONS[obs])) {
                    reel::Reception::Recu => Issue::Recu,
                    reel::Reception::Rejeu => Issue::Rejeu,
                    reel::Reception::Divergente => Issue::Divergente,
                    reel::Reception::Incompatible => Issue::Incompatible,
                    reel::Reception::Termine => Issue::Termine,
                    reel::Reception::Absent => Issue::Absent,
                }
            }
            Action::Reserver { d } => match self.a.reserver(d as u128) {
                Some(t) => {
                    let g = self.generation.entry(d).or_insert(0);
                    *g += 1;
                    self.t.insert(
                        Prov {
                            aut: AUTORITE,
                            dec: d,
                            generation: *g,
                        },
                        t,
                    );
                    Issue::Oui
                }
                None => Issue::Non,
            },
            Action::Confirmer { d, p } => {
                let t = *self.t.get(&p)?;
                b(self.a.confirmer(d as u128, t))
            }
            Action::EchecCertain { d, p } => {
                let t = *self.t.get(&p)?;
                b(self.a.echec_certain(d as u128, t))
            }
            Action::Indeterminer { d, p } => {
                let t = *self.t.get(&p)?;
                b(self.a.indetermine(d as u128, t))
            }
            Action::Resoudre { d, p, verdict } => {
                let t = *self.t.get(&p)?;
                let v = match verdict {
                    Verdict::EffetConfirme => RésolutionBackend::EffetConfirme,
                    Verdict::AbsenceDefinitive => RésolutionBackend::AbsenceDefinitive,
                    Verdict::Indetermine => RésolutionBackend::Indetermine,
                    Verdict::Incompatible => RésolutionBackend::Incompatible,
                };
                b(self.a.resoudre(d as u128, t, v))
            }
            // La transition n'existe plus dans l'implémentation : refusée par construction.
            Action::Abandonner { .. } => Issue::Non,
            Action::AbandonSansTentative { d } => b(self.a.abandonner_sans_tentative(d as u128)),
            Action::DemanderAnnulation { d } => b(self.a.demander_annulation(d as u128)),
        })
    }
}

fn rejouer(trace: &[Action]) -> Option<Reel> {
    let mut r = Reel::neuf();
    for a in trace {
        r.appliquer(*a)?;
    }
    Some(r)
}

fn trace_lisible(trace: &[Action]) -> String {
    let etapes: Vec<String> = trace.iter().map(action_lisible).collect();
    if etapes.is_empty() {
        "∅".to_string()
    } else {
        etapes.join(" → ")
    }
}

// ===========================================================================
// 8. Épreuves
// ===========================================================================

/// Profondeur d'exploration. 7 permet d'observer le contre-exemple de
/// `dd6382be` (6 transitions) et de vérifier qu'il est bien MINIMAL.
const PROFONDEUR: usize = 10;

fn reference() -> &'static Exploration {
    static REF: OnceLock<Exploration> = OnceLock::new();
    REF.get_or_init(|| {
        explorer(
            &Semantique::REFERENCE,
            &[0],
            &toutes_provenances(),
            PROFONDEUR,
        )
    })
}

fn variante(sem: &Semantique) -> Exploration {
    explorer(sem, &[0], &toutes_provenances(), PROFONDEUR)
}

#[test]
fn modele_reference_ne_viole_aucun_invariant() {
    let e = reference();
    assert!(
        e.noeuds.len() > 50,
        "exploration suspicieusement vide : {} états",
        e.noeuds.len()
    );
    let mut manques = Vec::new();
    for inv in TOUS_LES_INVARIANTS {
        if let Some(cx) = e.contre_exemple(inv) {
            manques.push(format!("{} → {}", inv.nom(), cx.lisible()));
        }
    }
    assert!(
        manques.is_empty(),
        "la référence doit satisfaire S1–S10 et L1–L5 :\n{}",
        manques.join("\n")
    );
}

#[test]
fn exploration_couvre_les_dimensions_demandees() {
    let e = reference();

    // 2 contenus
    assert!(
        e.action_acceptee(|a| matches!(a, Action::Record { contenu: 1, .. })),
        "le second contenu doit être enregistrable"
    );
    assert!(
        e.aretes.iter().any(
            |(_, a, _, i)| matches!(a, Action::Record { contenu: 1, .. }) && *i == Issue::Conflit
        ),
        "un contenu différent sur la même décision doit produire un CONFLIT"
    );

    // 2 observations : reçue, puis divergente
    assert!(e.action_acceptee(|a| matches!(a, Action::Recevoir { obs: 0, .. })));
    assert!(
        e.aretes
            .iter()
            .any(|(_, a, _, i)| matches!(a, Action::Recevoir { obs: 1, .. })
                && *i == Issue::Divergente),
        "une seconde observation divergente doit être refusée"
    );

    // 2 autorités : une provenance étrangère n'est JAMAIS acceptée
    assert!(
        !e.action_acceptee(|a| provenance_portee(a).is_some_and(|p| p.aut != AUTORITE)),
        "aucune action d'une autorité étrangère ne doit être acceptée"
    );
    assert!(
        e.aretes
            .iter()
            .any(|(_, a, _, _)| provenance_portee(*a).is_some_and(|p| p.aut != AUTORITE)),
        "l'alphabet doit réellement présenter des provenances étrangères"
    );

    // 2 générations
    assert!(
        e.etat_atteint(|m| m.cycles.values().any(|c| c.g >= 2)),
        "la génération 2 doit être atteinte"
    );
    assert!(
        e.action_acceptee(|a| provenance_portee(a).is_some_and(|p| p.generation == 2)),
        "une tentative de génération 2 doit pouvoir être confirmée"
    );

    // 2 décisions : une provenance croisée est refusée
    assert!(
        !e.action_acceptee(
            |a| provenance_portee(a).is_some_and(|p| p.dec != decision_visee(a).unwrap())
        ),
        "une provenance croisée (autre décision) ne doit jamais être acceptée"
    );

    // publication indéterminée
    assert!(
        e.etat_atteint(|m| m.cycles.values().any(|c| c.s == S::Indetermine)),
        "l'état Indetermine doit être atteint"
    );

    // résolution ±
    assert!(
        e.action_acceptee(|a| matches!(
            a,
            Action::Resoudre {
                verdict: Verdict::EffetConfirme,
                ..
            }
        )),
        "une résolution positive doit être possible"
    );
    assert!(
        e.action_acceptee(|a| matches!(
            a,
            Action::Resoudre {
                verdict: Verdict::AbsenceDefinitive,
                ..
            }
        )),
        "une résolution négative doit être possible"
    );

    // annulation
    assert!(
        e.etat_atteint(|m| m.cycles.values().any(|c| c.c)),
        "l'annulation doit être demandable"
    );

    // réponse périmée : une provenance de génération 1 refusée alors que la
    // tentative vivante est de génération 2.
    let perimee = e.aretes.iter().any(|(i, a, _, issue)| {
        provenance_portee(*a).is_some_and(|p| p.generation == 1 && p.aut == AUTORITE && p.dec == 0)
            && !issue.positive()
            && e.noeuds[*i]
                .monde
                .cycles
                .get(&0)
                .is_some_and(|c| c.g == 2 && c.s == S::EnCours)
    });
    assert!(
        perimee,
        "une réponse périmée doit être refusée sur une génération vivante plus récente"
    );
}

#[test]
fn famille_abandon_direct_retrouvee_en_s3() {
    // ⚑ e1e6e611 (a) : `abandonner(id, Tentative)` — EnCours|Indetermine → Abandonne.
    let e = variante(&Semantique::ABANDON_DIRECT);
    let cx = e
        .contre_exemple(Invariant::S3AbandonMasqueEffet)
        .expect("la référence défectueuse doit violer S3");
    assert_eq!(
        cx.profondeur(),
        4,
        "contre-exemple minimal attendu en 4 transitions : {}",
        cx.lisible()
    );
    assert!(
        matches!(cx.trace[3], Action::Abandonner { .. }),
        "la transition fautive est l'abandon direct : {}",
        cx.lisible()
    );
    assert!(
        !e.violate(Invariant::S5ProvenanceNonVivanteModifie),
        "cette famille ne doit pas se confondre avec celle de dd6382be"
    );
}

#[test]
fn famille_indetermine_sans_validation_retrouvee_en_s4() {
    // ⚑ e1e6e611 (b) : `resoudre(Indetermine) => true`.
    let e = variante(&Semantique::INDETERMINE_SANS_VALIDATION);
    let cx = e
        .contre_exemple(Invariant::S4ReponsePositiveSansProvenanceVivante)
        .expect("la référence défectueuse doit violer S4");
    // Le défaut est l'absence TOTALE de validation : il se manifeste dès la
    // première transition, sur une décision qui n'existe même pas.
    assert_eq!(
        cx.profondeur(),
        1,
        "contre-exemple minimal attendu en 1 transition : {}",
        cx.lisible()
    );
    assert!(
        matches!(
            cx.trace[0],
            Action::Resoudre {
                verdict: Verdict::Indetermine,
                ..
            }
        ),
        "la transition fautive est la résolution Indetermine : {}",
        cx.lisible()
    );
}

#[test]
fn famille_generation_non_verifiee_retrouvee_en_s5() {
    // ⚑ dd6382be : la génération n'est pas contrôlée dans resoudre.
    let e = variante(&Semantique::INCONNUE_SANS_GENERATION);
    let cx = e
        .contre_exemple(Invariant::S5ProvenanceNonVivanteModifie)
        .expect("la référence défectueuse doit violer S5");
    assert_eq!(
        cx.profondeur(),
        4,
        "contre-exemple minimal attendu en 4 transitions : {}",
        cx.lisible()
    );
    assert!(
        matches!(
            cx.trace[3],
            Action::Resoudre {
                verdict: Verdict::Incompatible | Verdict::Indetermine,
                ..
            }
        ),
        "la transition fautive est une résolution à génération non vérifiée : {}",
        cx.lisible()
    );
    // La provenance présentée porte une génération que la tentative vivante n'a
    // jamais eue : c'est le contrôle manquant.
    let presentee = cx
        .trace
        .iter()
        .filter_map(|a| provenance_portee(*a))
        .next_back()
        .unwrap();
    assert_ne!(
        presentee.generation,
        1,
        "la génération présentée doit différer de la génération vivante : {}",
        cx.lisible()
    );

    // Le scénario exact du commit — une ANCIENNE tentative (génération 1)
    // présentée après une nouvelle réservation (génération 2) — est aussi
    // détecté, plus tard.
    let perimee = e.aretes.iter().any(|(i, a, j, _)| {
        matches!(
            a,
            Action::Resoudre {
                verdict: Verdict::Incompatible,
                ..
            }
        ) && provenance_portee(*a).is_some_and(|p| p.generation == 1 && p.aut == AUTORITE)
            && e.noeuds[*i]
                .monde
                .cycles
                .get(&0)
                .is_some_and(|c| c.g == 2 && c.s == S::EnCours)
            && e.noeuds[*j]
                .monde
                .cycles
                .get(&0)
                .is_some_and(|c| c.s == S::Indetermine)
    });
    assert!(
        perimee,
        "une ancienne tentative ne doit pas pouvoir modifier le cycle de la génération vivante"
    );
    assert!(
        !e.violate(Invariant::S3AbandonMasqueEffet),
        "cette famille ne doit pas se confondre avec l'abandon direct"
    );
}

#[test]
fn les_trois_familles_sont_distinctes() {
    let a = variante(&Semantique::ABANDON_DIRECT);
    let b = variante(&Semantique::INDETERMINE_SANS_VALIDATION);
    let c = variante(&Semantique::INCONNUE_SANS_GENERATION);

    assert!(a.violate(Invariant::S3AbandonMasqueEffet));
    assert!(!a.violate(Invariant::S5ProvenanceNonVivanteModifie));

    assert!(b.violate(Invariant::S4ReponsePositiveSansProvenanceVivante));
    assert!(!b.violate(Invariant::S3AbandonMasqueEffet));

    assert!(c.violate(Invariant::S5ProvenanceNonVivanteModifie));
    assert!(!c.violate(Invariant::S3AbandonMasqueEffet));

    // Discrimination entre (b) et (c) : (b) ne valide RIEN — il répond
    // positivement dès la première transition, sur une décision inexistante.
    // (c) valide encore l'autorité et l'existence du cycle ; seul le contrôle
    // de génération manque, donc il se trahit plus tard.
    let s4_b = b
        .contre_exemple(Invariant::S4ReponsePositiveSansProvenanceVivante)
        .unwrap();
    assert_eq!(
        s4_b.profondeur(),
        1,
        "(b) se trahit sans aucune validation : {}",
        s4_b.lisible()
    );

    let s4_c = c
        .contre_exemple(Invariant::S4ReponsePositiveSansProvenanceVivante)
        .expect("(c) se trahit aussi sur Indetermine, mais autrement");
    assert!(
        s4_c.profondeur() > s4_b.profondeur(),
        "(c) valide encore quelque chose et se trahit donc plus tard : {} vs {}",
        s4_c.profondeur(),
        s4_b.profondeur()
    );
    // …et il faut qu'un cycle existe réellement à ce moment-là, et que la
    // génération présentée ne soit pas celle de la tentative vivante.
    let p_c = s4_c
        .trace
        .iter()
        .filter_map(|x| provenance_portee(*x))
        .next_back()
        .unwrap();
    assert_eq!(
        p_c.aut,
        AUTORITE,
        "(c) valide encore l'autorité : {}",
        s4_c.lisible()
    );
    assert_eq!(
        s4_c.profondeur(),
        4,
        "il faut un cycle réellement ouvert avant que (c) se trahisse : {}",
        s4_c.lisible()
    );
    assert_ne!(
        p_c.generation,
        1,
        "(c) ne se trahit que sur une génération qui n'est pas celle de la tentative vivante : {}",
        s4_c.lisible()
    );
}

#[test]
fn progression_l1_a_l5_sur_la_reference() {
    let e = reference();

    // L1 — pas de blocage.
    for (i, n) in e.noeuds.iter().enumerate() {
        if n.profondeur >= PROFONDEUR || tous_terminaux(&n.monde) {
            continue;
        }
        let vivant = e
            .aretes
            .iter()
            .any(|(src, _, dst, _)| *src == i && *dst != i);
        assert!(
            vivant,
            "blocage (L1) depuis {}",
            trace_lisible(&e.trace_de(i))
        );
    }

    // L2 — absence attestée ⟹ nouvelle tentative possible.
    for i in 0..e.noeuds.len() {
        for (_, a, j, issue) in e.aretes.iter().filter(|(src, _, _, _)| *src == i) {
            if let Action::Resoudre {
                d,
                p,
                verdict: Verdict::AbsenceDefinitive,
            } = a
                && issue.positive()
                && e.noeuds[*j].profondeur < PROFONDEUR
                && e.noeuds[*j]
                    .monde
                    .cycles
                    .get(d)
                    .is_some_and(|c| c.s == S::Recu)
            {
                let possible = e.aretes.iter().any(|(src, b, dst, bi)| {
                    *src == *j && matches!(b, Action::Reserver { .. }) && bi.positive() && dst != j
                });
                assert!(
                    possible,
                    "pas de reprise (L2) après absence attestée en d{d} génération {}",
                    p.generation
                );
            }
        }
    }

    // L3 — un terminal est atteignable depuis tout état non feuille.
    let atteignables = e.terminaux_atteignables();
    for (i, n) in e.noeuds.iter().enumerate() {
        if n.profondeur >= PROFONDEUR {
            continue;
        }
        assert!(
            atteignables.contains(&i),
            "aucun terminal atteignable (L3) depuis {}",
            trace_lisible(&e.trace_de(i))
        );
    }

    // L4 — rejeu idempotent.
    for (i, a, j, issue) in &e.aretes {
        let avant = &e.noeuds[*i].monde;
        match a {
            Action::Record { d, contenu }
                if avant.cycles.get(d).map(|c| c.def) == Some(CONTENUS[*contenu]) =>
            {
                assert_eq!(
                    *issue,
                    Issue::Rejeu,
                    "rejeu de record (L4) : {}",
                    action_lisible(a)
                );
                assert_eq!(i, j, "rejeu de record (L4) modifie l'état");
            }
            Action::Recevoir { d, obs }
                if avant
                    .cycles
                    .get(d)
                    .is_some_and(|c| c.o == Some(OBSERVATIONS[*obs]) && !c.terminal()) =>
            {
                assert_eq!(
                    *issue,
                    Issue::Rejeu,
                    "rejeu de recevoir (L4) : {}",
                    action_lisible(a)
                );
                assert_eq!(i, j, "rejeu de recevoir (L4) modifie l'état");
            }
            _ => {}
        }
    }

    // L5 — pas de tentative sans observation.
    for (i, a, _, issue) in &e.aretes {
        if let Action::Reserver { d } = a
            && issue.positive()
        {
            assert!(
                e.noeuds[*i]
                    .monde
                    .cycles
                    .get(d)
                    .is_some_and(|c| c.o.is_some()),
                "réservation sans observation (L5) en d{d}"
            );
        }
    }
}

#[test]
fn localite_et_entrelacement_deux_decisions() {
    let mut provenances = Vec::new();
    for d in DECISIONS {
        provenances.extend(provenances_propres(d));
    }
    let e = explorer(&Semantique::REFERENCE, &DECISIONS, &provenances, 7);

    let mut manques = Vec::new();
    for inv in TOUS_LES_INVARIANTS {
        if let Some(cx) = e.contre_exemple(inv) {
            manques.push(format!("{} → {}", inv.nom(), cx.lisible()));
        }
    }
    assert!(
        manques.is_empty(),
        "violations avec deux décisions :\n{}",
        manques.join("\n")
    );
    assert!(
        e.etat_atteint(|m| m.cycles.len() == 2),
        "les deux décisions doivent coexister dans un état atteint"
    );
    assert!(
        e.etat_atteint(|m| {
            m.cycles.len() == 2
                && m.cycles.values().any(|c| c.s == S::Confirme)
                && m.cycles.values().any(|c| c.s == S::Recu)
        }),
        "les deux décisions doivent pouvoir être dans des états différents"
    );
}

#[test]
fn le_reel_est_conforme_au_modele_de_reference() {
    let e = reference();
    let mut comparees = 0usize;
    let mut non_exprimables = 0usize;
    let mut divergences: Vec<String> = Vec::new();

    for (i, a, j, issue_modele) in &e.aretes {
        let trace = e.trace_de(*i);
        let Some(mut r) = rejouer(&trace) else {
            non_exprimables += 1;
            continue;
        };
        let attendu_avant = projection(&e.noeuds[*i].monde);
        if r.projection() != attendu_avant {
            divergences.push(format!(
                "état après le chemin {} : modèle {:?} / réel {:?}",
                trace_lisible(&trace),
                attendu_avant,
                r.projection()
            ));
            continue;
        }
        let Some(issue_reelle) = r.appliquer(*a) else {
            non_exprimables += 1;
            continue;
        };
        comparees += 1;
        if issue_reelle != *issue_modele {
            divergences.push(format!(
                "réponse sur {} : modèle {:?} / réel {:?}",
                action_lisible(a),
                issue_modele,
                issue_reelle
            ));
        }
        let attendu_apres = projection(&e.noeuds[*j].monde);
        if r.projection() != attendu_apres {
            divergences.push(format!(
                "après {} : modèle {:?} / réel {:?}",
                action_lisible(a),
                attendu_apres,
                r.projection()
            ));
        }
    }

    assert!(
        divergences.is_empty(),
        "{} divergences modèle/implémentation (sur {comparees} transitions, {non_exprimables} non exprimables) :\n{}",
        divergences.len(),
        divergences
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        comparees > 500,
        "comparaison trop pauvre : {comparees} transitions"
    );
    println!(
        "comparaison : {comparees} transitions conformes, {non_exprimables} non exprimables côté réel"
    );
}

#[test]
fn le_reel_se_comporte_comme_la_reference_partout_ou_le_defaut_differe() {
    let reference = reference();
    let mut ref_aretes: BTreeMap<(Monde, Action), (Issue, Monde)> = BTreeMap::new();
    for (i, a, j, issue) in &reference.aretes {
        ref_aretes.insert(
            (reference.noeuds[*i].monde.clone(), *a),
            (*issue, reference.noeuds[*j].monde.clone()),
        );
    }

    let familles: [(Semantique, &str); 3] = [
        (Semantique::ABANDON_DIRECT, "e1e6e611 (a) abandon direct"),
        (
            Semantique::INDETERMINE_SANS_VALIDATION,
            "e1e6e611 (b) Indetermine non validé",
        ),
        (
            Semantique::INCONNUE_SANS_GENERATION,
            "dd6382be génération non vérifiée",
        ),
    ];

    for (sem, etiquette) in familles {
        let e = variante(&sem);
        let mut ecarts = 0usize;
        let mut exprimes = 0usize;
        let mut refus = 0usize;

        for (i, a, j, issue_var) in &e.aretes {
            let avant = e.noeuds[*i].monde.clone();
            // La transition n'est un DÉFAUT que si la référence, depuis le même
            // état et sur la même action, fait autre chose.
            let Some((issue_ref, monde_ref)) = ref_aretes.get(&(avant.clone(), *a)) else {
                continue;
            };
            if e.noeuds[*j].monde == *monde_ref && *issue_var == *issue_ref {
                continue;
            }
            ecarts += 1;
            let trace = e.trace_de(*i);
            let Some(mut r) = rejouer(&trace) else {
                continue;
            };
            // Le préfixe peut lui-même contenir une transition défectueuse : dans
            // ce cas l'implémentation réelle n'est pas dans l'état `avant` et la
            // comparaison n'aurait pas de sens.
            if r.projection() != projection(&avant) {
                continue;
            }
            let Some(issue_reelle) = r.appliquer(*a) else {
                continue;
            };
            exprimes += 1;
            if issue_reelle == *issue_ref && r.projection() == projection(monde_ref) {
                refus += 1;
            }
        }

        assert!(
            ecarts > 0,
            "famille {etiquette} : aucune transition ne s'écarte de la référence"
        );
        assert!(
            exprimes > 0,
            "famille {etiquette} : aucun écart exprimable contre l'implémentation"
        );
        assert_eq!(
            refus,
            exprimes,
            "famille {etiquette} : {}/{} écarts reproduits par l'implémentation réelle — le défaut serait présent",
            exprimes - refus,
            exprimes
        );
        println!(
            "famille {etiquette} : {exprimes} contre-exemples, tous refusés par l'implémentation réelle"
        );
    }
}

#[test]
fn le_reel_n_atteint_jamais_abandonne_depuis_une_tentative() {
    // Le contre-exemple de `e1e6e611` (a), rejoué contre l'implémentation réelle :
    // record, recevoir, reserver → le cycle est EnCours et l'effet est inconnu.
    // Aucun chemin de l'API ne doit le déclarer Abandonne.
    let mut a = reel::Attribution::new();
    a.record(0, def_reelle(CONTENUS[0]));
    a.recevoir(0, obs_reelle(OBSERVATIONS[0]));
    let t = a.reserver(0).expect("réservation");
    assert_eq!(etat_modele(&a.etat(0).unwrap().etat), S::EnCours);

    assert!(
        !a.abandonner_sans_tentative(0),
        "abandon direct depuis EnCours"
    );
    assert_eq!(etat_modele(&a.etat(0).unwrap().etat), S::EnCours);

    assert!(a.indetermine(0, t));
    assert!(
        !a.abandonner_sans_tentative(0),
        "abandon direct depuis Indetermine"
    );
    assert_eq!(etat_modele(&a.etat(0).unwrap().etat), S::Indetermine);

    // Seule une résolution d'absence ouvre l'abandon.
    assert!(a.resoudre(0, t, RésolutionBackend::AbsenceDefinitive));
    assert!(a.abandonner_sans_tentative(0));
    assert_eq!(etat_modele(&a.etat(0).unwrap().etat), S::Abandonne);
}

#[test]
fn sonde_provenances_non_presentables() {
    // Ce que le modèle affirme (aucune action d'une autorité étrangère n'est
    // acceptée) doit être vrai de l'implémentation : deux instances, les mêmes
    // décisions, les tentatives de l'une présentées à l'autre.
    let mut a = reel::Attribution::new();
    let mut b = reel::Attribution::new();
    let def = def_reelle(CONTENUS[0]);
    let obs = obs_reelle(OBSERVATIONS[0]);
    a.record(0, def.clone());
    b.record(0, def);
    a.recevoir(0, obs.clone());
    b.recevoir(0, obs);

    let ta = a.reserver(0).expect("réservation A");
    let tb = b.reserver(0).expect("réservation B");

    assert!(!b.confirmer(0, ta), "confirmer avec la tentative de A");
    assert!(
        !b.echec_certain(0, ta),
        "echec_certain avec la tentative de A"
    );
    assert!(!b.indetermine(0, ta), "indeterminer avec la tentative de A");
    assert!(
        !b.resoudre(0, ta, RésolutionBackend::Indetermine),
        "resoudre(Indetermine) étranger"
    );
    assert!(
        !b.resoudre(0, ta, RésolutionBackend::Incompatible),
        "resoudre(Incompatible) étranger"
    );
    assert!(!a.confirmer(0, tb), "confirmer avec la tentative de B");
    assert_eq!(etat_modele(&b.etat(0).unwrap().etat), S::EnCours);
    assert_eq!(etat_modele(&a.etat(0).unwrap().etat), S::EnCours);

    // Décision croisée : la tentative de la décision 1 présentée à la décision 0
    // de la MÊME instance.
    let mut c = reel::Attribution::new();
    c.record(1, def_reelle(CONTENUS[0]));
    c.recevoir(1, obs_reelle(OBSERVATIONS[0]));
    let t1 = c.reserver(1).expect("réservation sur la décision 1");
    c.record(0, def_reelle(CONTENUS[0]));
    c.recevoir(0, obs_reelle(OBSERVATIONS[0]));
    assert!(!c.confirmer(0, t1), "confirmer d1 sur d0");
    assert!(!c.echec_certain(0, t1), "echec_certain d1 sur d0");
    assert!(!c.indetermine(0, t1), "indeterminer d1 sur d0");
    assert!(
        !c.resoudre(0, t1, RésolutionBackend::Indetermine),
        "resoudre d1 sur d0"
    );
    assert_eq!(etat_modele(&c.etat(0).unwrap().etat), S::Recu);
    assert_eq!(etat_modele(&c.etat(1).unwrap().etat), S::EnCours);
}

#[test]
fn rapport() {
    let e = reference();
    println!("\n=== Modèle causal — exploration de référence ===");
    let g_max = e
        .noeuds
        .iter()
        .flat_map(|n| n.monde.cycles.values().map(|c| c.g))
        .max()
        .unwrap_or(0);
    let prof_max = e.noeuds.iter().map(|n| n.profondeur).max().unwrap_or(0);
    println!(
        "profondeur ≤ {}, {} états atteints (profondeur réelle {}), {} transitions examinées, {} acceptées, génération max {}",
        e.profondeur_max,
        e.noeuds.len(),
        prof_max,
        e.aretes.len(),
        e.acceptees,
        g_max
    );
    for inv in TOUS_LES_INVARIANTS {
        match e.contre_exemple(inv) {
            None => println!("  ok   {}", inv.nom()),
            Some(cx) => println!("  KO   {} ← {}", inv.nom(), cx.lisible()),
        }
    }

    for (nom, sem) in [
        ("e1e6e611 (a) abandon direct", Semantique::ABANDON_DIRECT),
        (
            "e1e6e611 (b) Indetermine non validé",
            Semantique::INDETERMINE_SANS_VALIDATION,
        ),
        (
            "dd6382be génération non vérifiée",
            Semantique::INCONNUE_SANS_GENERATION,
        ),
    ] {
        let v = variante(&sem);
        println!("\n--- variante : {nom} ---");
        let mut rien = true;
        for inv in TOUS_LES_INVARIANTS {
            if let Some(cx) = v.contre_exemple(inv) {
                rien = false;
                println!("  KO   {} (profondeur {})", inv.nom(), cx.profondeur());
                println!("       {}", cx.lisible());
            }
        }
        if rien {
            println!("  (aucun invariant violé — la variante serait indétectable)");
        }
    }
    println!();
}
