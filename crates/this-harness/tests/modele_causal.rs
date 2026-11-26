//! **Modèle causal exécutable du cycle logique d'attribution — A-2.**
//!
//! Ce fichier ne réutilise RIEN de `crates/this-harness/src/attribution.rs` : il
//! pose ses propres variables, ses propres transitions et ses propres invariants,
//! puis *compare* le résultat à l'implémentation réelle. Le but n'est pas de
//! décrire le code : c'est de faire tourner un modèle qui **retrouve tout seul**
//! les familles de défauts corrigées en `e1e6e611` (abandon direct) et
//! `dd6382be` (génération non vérifiée dans `resoudre`), sous forme de
//! contre-exemples **minimaux**.
//!
//! # Ce que A-2 sépare (suite de `chore/modele-causal` @ `0a45843a`)
//!
//! A-1 portait une table de variables où deux identifications étaient fausses :
//!
//! | A-1 disait | la vérité |
//! |---|---|
//! | K « publication logique » = `Cycle::s` | S est l'**état**, pas l'identité ; une publication garde son identité pendant que l'état change trois fois (`EnCours → Indetermine → Confirme`) |
//! | B « backend » = `Action::Resoudre` (verdict) | le verdict est un **fait sur le monde**, le backend est une **autorité** ; le même verdict rendu par deux backends n'est pas la même preuve |
//!
//! Conséquence mesurable de la seconde : en A-1, deux résolutions de même verdict
//! mais d'autorités backend différentes étaient **la même action** — donc le
//! modèle ne pouvait pas les distinguer, donc il ne pouvait pas dire qu'un
//! verdict étranger ne prouve rien. A-2 les sépare et l'affirme (S11).
//!
//! # Variables
//!
//! | lettre | variable | ici |
//! |---|---|---|
//! | D | opération (contenu) | `Def`, `Action::Record` |
//! | O | observation immuable | `Obs` |
//! | **K** | **identité logique de publication** | `K { dec, rang }`, portée par `Cycle::vivante` et par `Prov` |
//! | **B** | **autorité backend** | `Cycle::b`, `R::par`, `Action::{Reserver,Confirmer,EchecCertain,Resoudre}` |
//! | **V** | verdict rendu par B | `Action::Resoudre { verdict }` — **distinct de B** |
//! | G | génération (compteur) | `Cycle::g` |
//! | S | état opérationnel | `Cycle::s` |
//! | R | connaissance attestable | `Cycle::r` (`R`) |
//! | C | annulation | `Cycle::c` |
//! | P | provenance (qui émet) | `Prov { aut, k }` — **distinct de B** (qui atteste) |
//! | E | effet externe, **par backend** | `Monde::effets: (Dec, Back) → E` |
//!
//! E appartient à l'environnement, pas au cycle : le modèle le porte comme une
//! variable fantôme, précisément pour pouvoir nommer le moment où S en dit plus
//! que ce que E permet. C'est toute la sûreté du cycle. En A-2, E est indexé par
//! **backend** : un effet engagé dans un magasin n'existe pas dans l'autre, et
//! c'est ce qui donne un contenu à B (sinon B ne serait qu'une étiquette).
//!
//! ## K n'est ni D, ni G, ni S
//!
//! * **ni D** : deux opérations de *contenu identique* sont deux publications
//!   distinctes (`k_est_une_identite_stable_distincte_de_s_et_de_g`).
//! * **ni G** : `rang = 1` sur l'opération 0 et sur l'opération 1 est le **même
//!   nombre** et deux publications **différentes** ; un contrôle qui ne compare
//!   que le nombre accepte la seconde à la place de la première (variante
//!   `identite_reduite_au_rang`).
//! * **ni S** : la même K est vivante en `EnCours`, en `Indetermine` et attestée
//!   en `Confirme`.
//!
//! ## B n'est ni V, ni P
//!
//! * **ni V** : S11 dit qu'une attestation ne vaut que pour le backend qui a
//!   *engagé* la publication ; le même `EffetConfirme` d'un autre backend est
//!   refusé (variante `attestation_sans_backend`).
//! * **ni P** : P dit *qui émet* une tentative (l'instance d'`Attribution`), B
//!   dit *qui atteste* l'effet. Deux axes d'étrangeté indépendants, exercés
//!   séparément.
//!
//! # Invariants
//!
//! Sûreté — S1 observation immuable · S2 confirmation ⟹ effet attesté ·
//! S3 abandon ⟹ effet non inconnu **nulle part** · S4 réponse positive ⟹
//! provenance vivante · S5 provenance non vivante ⟹ aucune modification ·
//! S6 une seule tentative vivante · S7 Indetermine ⟹ effet inconnu ·
//! S8 pas de ré-publication aveugle · S9 terminal absorbant · S10 annulation
//! orthogonale · **S11 attestation locale au backend de la publication** ·
//! **S12 contenu incompatible ⟹ ni attestation ni reprise** ·
//! **S13 aucun effet présent dans deux backends**.
//!
//! Progression — L1 pas de blocage · L2 absence attestée ⟹ reprise ·
//! L3 terminal atteignable · L4 rejeu idempotent · L5 tentative ⟹ observation.
//!
//! # Méthode
//!
//! 1. La **référence** (`Semantique::REFERENCE`) explore les séquences de
//!    transitions jusqu'à `PROFONDEUR`, en dédupliquant les états, et ne doit
//!    violer aucun invariant (`modele_reference_ne_viole_aucun_invariant`).
//! 2. Cinq **variantes défectueuses** — trois historiques, deux propres à A-2 —
//!    doivent, elles, produire un contre-exemple, et le plus court possible.
//!    C'est ce qui prouve que l'explorateur discrimine au lieu de dire « ok »
//!    partout.
//! 3. Le modèle de référence est **comparé** à `this_harness::attribution` :
//!    chaque transition de l'exploration est rejouée sur l'implémentation, et
//!    la réponse comme l'état d'arrivée doivent coïncider. Le périmètre de ce
//!    rejeu est mesuré et borné par `perimetre_du_rejeu`.
//!
//! # Registre
//!
//! **MESURÉ** — l'exploration de référence (états, transitions, invariants
//! satisfaits), la profondeur des contre-exemples minimaux, la conformité de
//! l'implémentation livrée sur chaque transition rejouée, le refus des
//! transitions par lesquelles les variantes s'écartent de la référence, et le
//! décompte du périmètre non rejouable. Les chiffres sont ceux que `cargo test
//! -p this-harness --test modele_causal -- --nocapture` imprime ; ils ne sont
//! pas recopiés ici pour ne pas dériver.
//!
//! **RELU** — la correspondance entre chaque variante historique et le code
//! supprimé, lue dans les diffs de `e1e6e611` (suppression de
//! `abandonner(id, Tentative)` et du `true` inconditionnel de
//! `resoudre(Indetermine)`) et de `dd6382be` (contrôle de génération ajouté dans
//! les bras `Indetermine` et `Incompatible`). Elle est réécrite ici en
//! sémantique, pas importée du code : le modèle reste indépendant de
//! l'implémentation.
//!
//! **SUPPOSÉ** — rien.
//!
//! # Asymétries assumées entre le modèle et l'implémentation
//!
//! Elles sont toutes **mesurées** par `perimetre_du_rejeu`, qui les compte au
//! lieu de les passer sous silence.
//!
//! 1. **`Tentative` est opaque** : dans le modèle, `Prov` est une donnée, on peut
//!    présenter n'importe quel couple (autorité, K). Dans l'implémentation, seule
//!    `reserver` en émet une. Les présentations jamais émises ne sont donc pas
//!    rejouables ; elles sont couvertes par des sondes dédiées
//!    (`sonde_provenances_non_presentables`).
//! 2. **L'implémentation ne porte aucun B.** `RésolutionBackend` est un verdict
//!    et il n'existe pas de type d'autorité backend : une résolution ne dit pas
//!    *qui* a répondu. Tout ce que le modèle exprime avec `Back != BACKEND` est
//!    donc **hors du périmètre du rejeu** — S11 est une propriété que le modèle
//!    énonce et que l'implémentation ne peut ni violer ni respecter, faute de
//!    porter la variable. C'est le contrat manquant, nommé et non contourné.
//! 3. **Aucun état durable du cycle** : `Attribution` est une `HashMap` en RAM.
//!    Le modèle explore des enchaînements qu'un redémarrage ne peut pas
//!    présenter ; il ne dit rien de la reprise après mort du processus.
//!
//! Ce que le rejeu prouve est donc borné : la sémantique des transitions sur les
//! variables que l'implémentation **porte réellement** (D, O, S, G, C, R).
//!
//! # Les six exercices demandés, et où ils sont exercés
//!
//! | exercice | épreuve |
//! |---|---|
//! | deux opérations à contenu identique | `k_est_une_identite_stable_distincte_de_s_et_de_g` (K ≠ D) |
//! | une opération, plusieurs générations | idem (K(0,g1) puis K(0,g2), la périmée refusée) |
//! | deux backends, le même verdict | `b_est_une_autorite_distincte_du_verdict` (B ≠ V) |
//! | résolution étrangère | idem (verdict d'un backend non porteur) + `sonde_provenances_non_presentables` (autorité non émettrice) |
//! | changement de backend après indétermination | `changement_de_backend_apres_indetermination` (interdit pendant, légitime après absence attestée) |
//! | contenu incompatible | `exploration_couvre_les_dimensions_demandees` (réception) + S12 (résolution) |
//!
//! # Ce que A-2 ne dit pas
//!
//! Le modèle ne dit rien de la **durabilité** : `Cycle`, `K` et `B` vivent ici en
//! mémoire pure, comme dans l'implémentation. Il ne dit rien non plus d'une
//! concurrence *réelle* entre deux écrivains — il explore des séquences, pas des
//! entrelacements simultanés. Ces deux questions exigent les contrats manquants
//! nommés ailleurs (unicité de propriété, écriture conditionnelle, enregistrement
//! durable de la tentative en vol), pas une extension de ce fichier.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::OnceLock;

use this_harness::attribution as reel;
use this_harness::attribution::{
    Definition, Etat as EtatReel, Observation, Resultat, RésolutionBackend,
};

// ===========================================================================
// 1. Variables du modèle
// ===========================================================================

/// D — identifiant d'opération.
type Dec = u8;

/// P — identité d'autorité : qui émet une tentative.
type Aut = u8;

/// B — identité d'un backend : l'autorité qui peut attester un effet.
type Back = u8;

/// L'autorité du monde que l'on explore. Les provenances qui portent une AUTRE
/// autorité sont étrangères et doivent être refusées.
const AUTORITE: Aut = 0;

/// Le backend de l'effet. Le rejeu contre l'implémentation réelle n'utilise QUE
/// celui-ci : l'implémentation ne porte aucune autorité backend, donc tout ce
/// qui est engagé ailleurs n'est pas exprimable (voir `perimetre_du_rejeu`).
const BACKEND: Back = 0;

const DECISIONS: [Dec; 2] = [0, 1];
const AUTORITES: [Aut; 2] = [0, 1];
const BACKENDS: [Back; 2] = [0, 1];

/// Les rangs de publication qu'une provenance peut *présenter*. Le cycle, lui,
/// peut monter plus haut : c'est ce qui rend une provenance « périmée ».
const RANGS_PRESENTES: [u64; 2] = [1, 2];

/// K — l'identité logique d'une **publication**. Une publication, c'est un
/// `reserver` : l'acte d'engager l'effet d'une opération dans un backend.
///
/// K n'est **ni le contenu** (`Def`) — deux opérations de contenu identique ont
/// deux K distinctes —, **ni le rang** — `rang = 1` sur deux opérations est le
/// même nombre et deux publications différentes —, **ni l'état** — la même K est
/// vivante en `EnCours`, en `Indetermine`, puis attestée. C'est un couple, parce
/// qu'un nombre seul n'identifie rien : il faut dire *de quel compteur* il est
/// le rang.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct K {
    /// l'opération que cette publication sert
    dec: Dec,
    /// le rang de la publication dans cette opération
    rang: u64,
}

/// D — le contenu d'une opération : ce qui doit rester identique pour un rejeu.
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

/// S — l'état opérationnel. (K, l'identité de la publication, n'est PAS ici.)
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum S {
    Ouvert,
    Recu,
    EnCours,
    Indetermine,
    Confirme,
    Abandonne,
}

/// E — l'effet externe, côté environnement, **pour un backend donné**.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum E {
    /// Aucun effet, et c'est certain.
    Aucun,
    /// L'effet existe et il est attesté.
    Present,
    /// On ne sait pas : une publication est engagée, aucun verdict n'est arrivé.
    Inconnu,
}

/// P — provenance d'une tentative : l'autorité qui l'émet et l'identité de la
/// publication qu'elle prétend régler. **Elle ne dit rien du backend** : c'est
/// l'affaire de B, portée par l'action d'attestation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Prov {
    aut: Aut,
    k: K,
}

/// R — la connaissance attestable : quelle publication a été attestée, et par
/// quel backend. Le backend est dans R et non dans K : l'attestation dit *qui a
/// répondu*, l'identité dit *quoi*.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
struct R {
    effet: Option<K>,
    absence: Option<K>,
    /// le backend qui a attesté (`Some` dès qu'une attestation est posée)
    par: Option<Back>,
}

/// L'état d'un cycle.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Cycle {
    def: Def,
    o: Option<Obs>,
    s: S,
    /// G — compteur de publications de cette opération. Il survit aux
    /// publications ; c'est lui qui donne à la suivante un rang neuf.
    g: u64,
    /// B — le backend qui porte l'effet de cette opération : celui de la
    /// publication courante, et celui de la dernière publication réglée après
    /// elle. Il n'est jamais effacé (une attestation doit rester lisible).
    b: Option<Back>,
    c: bool,
    /// K — l'identité de la publication VIVANTE (`None` ⟺ aucune publication
    /// engagée). Elle n'existe que pendant la vie de la publication.
    vivante: Option<K>,
    r: R,
}

impl Cycle {
    fn neuf(def: Def) -> Self {
        Self {
            def,
            o: None,
            s: S::Ouvert,
            g: 0,
            b: None,
            c: false,
            vivante: None,
            r: R::default(),
        }
    }

    fn terminal(&self) -> bool {
        matches!(self.s, S::Confirme | S::Abandonne)
    }
}

/// L'état du système : les cycles + l'effet de l'environnement, **par backend**.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
struct Monde {
    cycles: BTreeMap<Dec, Cycle>,
    effets: BTreeMap<(Dec, Back), E>,
}

impl Monde {
    fn effet(&self, d: Dec, b: Back) -> E {
        self.effets.get(&(d, b)).copied().unwrap_or(E::Aucun)
    }

    /// The backend of the operation's effect, s'il y en a eu un.
    fn backend_de(&self, d: Dec) -> Option<Back> {
        self.cycles.get(&d).and_then(|c| c.b)
    }
}

// ===========================================================================
// 2. Alphabet de transitions
// ===========================================================================

/// V — verdict du backend pour une résolution. **Ce n'est pas B** : c'est ce que
/// l'autorité backend *dit*, pas l'autorité qui le dit.
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

/// Une transition. Deux opérations, deux autorités, deux backends, deux rangs et
/// deux contenus se croisent dans cet alphabet.
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
    /// Engager l'effet de `d` dans le backend `backend` → émet une K neuve.
    Reserver {
        d: Dec,
        backend: Back,
    },
    /// Attester l'effet. `par` est l'autorité backend qui répond.
    Confirmer {
        d: Dec,
        p: Prov,
        par: Back,
    },
    EchecCertain {
        d: Dec,
        p: Prov,
        par: Back,
    },
    /// Déclarer l'issue inconnue. Ce n'est **pas** une attestation : aucun
    /// backend n'est nommé, et c'est pour cela que l'action n'en porte pas.
    Indeterminer {
        d: Dec,
        p: Prov,
    },
    Resoudre {
        d: Dec,
        p: Prov,
        par: Back,
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

/// La sémantique **de référence** est celle qui respecte les invariants. Chaque
/// drapeau reproduit une confusion : les trois premiers sont des familles
/// historiquement corrigées, les deux derniers sont les confusions propres à
/// A-2 (K ≡ G et B ≡ V), rendues exécutables pour prouver que le modèle les
/// distingue.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Semantique {
    /// ⚑ `e1e6e611` (a) — `abandonner(id, Tentative)` : `EnCours|Indetermine →
    /// Abandonne` sans attestation.
    abandon_direct: bool,
    /// ⚑ `e1e6e611` (b) — `resoudre(Indetermine) => true` sans validation.
    indetermine_sans_validation: bool,
    /// ⚑ `dd6382be` — `resoudre(Indetermine|Incompatible)` sans vérifier QUELLE
    /// publication règle. Le commit parlait de « génération » ; en A-2 c'est
    /// l'identité K qui manque, et le rang n'en est qu'une moitié.
    identite_non_verifiee: bool,
    /// ⚑ A-2 — B confondu avec V : n'importe quel backend peut attester.
    attestation_sans_backend: bool,
    /// ⚑ A-2 — K confondu avec G : l'identité est réduite au rang, donc le
    /// nombre. C'est la conflation que la table d'A-1 écrivait noir sur blanc.
    identite_reduite_au_rang: bool,
}

impl Semantique {
    const REFERENCE: Self = Self {
        abandon_direct: false,
        indetermine_sans_validation: false,
        identite_non_verifiee: false,
        attestation_sans_backend: false,
        identite_reduite_au_rang: false,
    };

    const ABANDON_DIRECT: Self = Self {
        abandon_direct: true,
        ..Self::REFERENCE
    };

    const INDETERMINE_SANS_VALIDATION: Self = Self {
        indetermine_sans_validation: true,
        ..Self::REFERENCE
    };

    const IDENTITE_NON_VERIFIEE: Self = Self {
        identite_non_verifiee: true,
        ..Self::REFERENCE
    };

    const ATTESTATION_SANS_BACKEND: Self = Self {
        attestation_sans_backend: true,
        ..Self::REFERENCE
    };

    const IDENTITE_REDUITE_AU_RANG: Self = Self {
        identite_reduite_au_rang: true,
        ..Self::REFERENCE
    };
}

// ===========================================================================
// 4. Transitions
// ===========================================================================

impl Monde {
    /// D : enregistrer une opération. Un contenu différent sur la même opération
    /// est un CONFLIT, jamais un écrasement.
    fn record(&mut self, d: Dec, def: Def) -> Issue {
        match self.cycles.get(&d) {
            Some(c) if c.def == def => Issue::Rejeu,
            Some(_) => Issue::Conflit,
            None => {
                self.cycles.insert(d, Cycle::neuf(def));
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

    /// G + K + B : engager une publication dans un backend. Un seul écrivain à la
    /// fois, et seulement après une observation. La publication étant ENGAGÉE,
    /// l'effet devient inconnu **dans ce backend-là** : c'est le fait causal que
    /// le cycle doit ensuite respecter.
    fn reserver(&mut self, d: Dec, backend: Back) -> Option<K> {
        let c = self.cycles.get_mut(&d)?;
        if c.s != S::Recu {
            return None;
        }
        c.g = c.g.checked_add(1)?; // débordement → refus
        c.s = S::EnCours;
        // B : le backend de l'effet est celui de la publication courante. Il
        // peut changer d'une publication à l'autre — mais seulement depuis
        // `Recu`, c'est-à-dire après qu'une absence a été ATTESTÉE (S8).
        c.b = Some(backend);
        // K : une identité NEUVE à chaque publication, jamais recyclée.
        let k = K { dec: d, rang: c.g };
        c.vivante = Some(k);
        self.effets.insert((d, backend), E::Inconnu);
        Some(k)
    }

    /// P : la provenance nomme une opération du monde que l'on explore. Dans la
    /// variante `identite_reduite_au_rang`, elle ne nomme plus RIEN qu'une
    /// autorité : l'identité est réduite à son rang, donc l'opération sort de la
    /// comparaison — c'est exactement ce que veut dire « confondre K avec G ».
    fn provenance_valide(&self, d: Dec, p: Prov, sem: &Semantique) -> bool {
        p.aut == AUTORITE && (sem.identite_reduite_au_rang || p.k.dec == d)
    }

    /// P × K : la provenance présentée désigne la publication VIVANTE. Même
    /// autorité, même opération, **même identité de publication** — ou, dans la
    /// variante `identite_reduite_au_rang`, même rang seulement.
    fn vivante_meme(&self, d: Dec, p: Prov, sem: &Semantique) -> bool {
        let Some(k) = self.cycles.get(&d).and_then(|c| c.vivante) else {
            return false;
        };
        if sem.identite_reduite_au_rang {
            k.rang == p.k.rang
        } else {
            k == p.k
        }
    }

    /// B : le verdict présenté vient du backend qui porte l'effet de cette
    /// opération. Un autre backend parle d'un autre magasin : sa réponse ne dit
    /// rien de ce fait-ci.
    fn backend_valide(&self, d: Dec, par: Back) -> bool {
        self.backend_de(d) == Some(par)
    }

    /// L'effet est attesté. Terminal.
    fn confirmer(&mut self, d: Dec, p: Prov, par: Back, sem: &Semantique) -> bool {
        if !self.provenance_valide(d, p, sem) || !self.vivante_meme(d, p, sem) {
            return false;
        }
        // S11 : une attestation n'est valable que dans le magasin où l'effet a
        // été engagé.
        if !sem.attestation_sans_backend && !self.backend_valide(d, par) {
            return false;
        }
        let Some(c) = self.cycles.get_mut(&d) else {
            return false;
        };
        c.s = S::Confirme;
        c.r.effet = Some(p.k);
        c.r.par = Some(par);
        c.vivante = None;
        self.effets.insert((d, par), E::Present);
        true
    }

    /// L'absence d'effet est certaine et définitive : la réservation est
    /// relâchée, une nouvelle tentative devient possible.
    fn echec_certain(&mut self, d: Dec, p: Prov, par: Back, sem: &Semantique) -> bool {
        if !self.provenance_valide(d, p, sem) || !self.vivante_meme(d, p, sem) {
            return false;
        }
        if !sem.attestation_sans_backend && !self.backend_valide(d, par) {
            return false;
        }
        let Some(c) = self.cycles.get_mut(&d) else {
            return false;
        };
        c.s = S::Recu;
        c.r.absence = Some(p.k);
        c.r.par = Some(par);
        c.vivante = None;
        self.effets.insert((d, par), E::Aucun);
        true
    }

    /// La publication est d'issue inconnue. On ne sait RIEN de plus qu'avant —
    /// et surtout, on ne nomme aucun backend : ce n'est pas une attestation.
    fn indeterminer(&mut self, d: Dec, p: Prov, sem: &Semantique) -> bool {
        if !self.provenance_valide(d, p, sem) || !self.vivante_meme(d, p, sem) {
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
        if !self.provenance_valide(d, p, sem) || !self.vivante_meme(d, p, sem) {
            return false;
        }
        let Some(c) = self.cycles.get_mut(&d) else {
            return false;
        };
        c.s = S::Abandonne;
        c.vivante = None;
        true
    }

    /// Résolution d'une publication indéterminée, selon le backend `par` et son
    /// verdict. Les deux dimensions sont séparées : `par` dit QUI répond,
    /// `verdict` dit CE QU'il répond.
    fn resoudre(&mut self, d: Dec, p: Prov, par: Back, v: Verdict, sem: &Semantique) -> bool {
        match v {
            Verdict::EffetConfirme => self.confirmer(d, p, par, sem),
            Verdict::AbsenceDefinitive => self.echec_certain(d, p, par, sem),
            Verdict::Indetermine => {
                // ⚑ e1e6e611 (b)
                if sem.indetermine_sans_validation {
                    return true;
                }
                // ⚑ dd6382be, bras Indetermine : ni la publication ni même son
                // rang ne sont contrôlés.
                if sem.identite_non_verifiee {
                    if !self.provenance_valide(d, p, sem) || !self.backend_valide(d, par) {
                        return false;
                    }
                    return matches!(
                        self.cycles.get(&d).map(|c| c.s),
                        Some(S::EnCours) | Some(S::Indetermine)
                    );
                }
                if !self.provenance_valide(d, p, sem)
                    || !self.vivante_meme(d, p, sem)
                    || !self.backend_valide(d, par)
                {
                    return false;
                }
                matches!(
                    self.cycles.get(&d).map(|c| c.s),
                    Some(S::EnCours) | Some(S::Indetermine)
                )
            }
            Verdict::Incompatible => {
                // ⚑ dd6382be, bras Incompatible
                if sem.identite_non_verifiee {
                    if !self.provenance_valide(d, p, sem) || !self.backend_valide(d, par) {
                        return false;
                    }
                    if let Some(c) = self.cycles.get_mut(&d)
                        && matches!(c.s, S::EnCours | S::Indetermine)
                    {
                        c.s = S::Indetermine;
                    }
                    return false;
                }
                if !self.provenance_valide(d, p, sem)
                    || !self.vivante_meme(d, p, sem)
                    || !self.backend_valide(d, par)
                {
                    return false;
                }
                let Some(c) = self.cycles.get_mut(&d) else {
                    return false;
                };
                // S12 : le backend a trouvé un effet qui n'est PAS celui de
                // cette opération. Ni attestation, ni reprise.
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
            Action::Reserver { d, backend } => {
                self.reserver(d, backend).map_or(Issue::Non, |_| Issue::Oui)
            }
            Action::Confirmer { d, p, par } => b(self.confirmer(d, p, par, sem)),
            Action::EchecCertain { d, p, par } => b(self.echec_certain(d, p, par, sem)),
            Action::Indeterminer { d, p } => b(self.indeterminer(d, p, sem)),
            Action::Resoudre { d, p, par, verdict } => b(self.resoudre(d, p, par, verdict, sem)),
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
    /// S2 — S = Confirme ⟹ l'effet est attesté **dans le backend qui le porte**,
    /// par ce backend, pour une publication de cette opération.
    S2ConfirmationSansAttestation,
    /// S3 — S = Abandonne ⟹ l'effet n'est inconnu **dans aucun** backend.
    S3AbandonMasqueEffet,
    /// S4 — une réponse POSITIVE porte la provenance de la tentative vivante.
    S4ReponsePositiveSansProvenanceVivante,
    /// S5 — une provenance non vivante (étrangère, croisée, périmée) ne modifie
    /// RIEN. Inclut la localité : une action sur d ne touche que d.
    S5ProvenanceNonVivanteModifie,
    /// S6 — S ∈ {EnCours, Indetermine} ⟺ exactement une publication vivante.
    S6TentativeVivante,
    /// S7 — S = Indetermine ⟹ E = Inconnu dans le backend de l'effet.
    S7IndetermineSansIgnorance,
    /// S8 — pas de ré-publication aveugle : on ne réserve que depuis Recu.
    S8RepublicationAveugle,
    /// S9 — Confirme et Abandonne sont absorbants, et n'obtiennent pas de réponse positive.
    S9TerminalNonAbsorbant,
    /// S10 — l'annulation est orthogonale : elle ne touche ni S, ni O, ni B, ni
    /// K, ni R, ni E.
    S10AnnulationNonOrthogonale,
    /// S11 — **A-2** — une attestation ne vaut que pour le backend qui a engagé
    /// la publication ; une attestation étrangère ne modifie rien.
    S11AttestationNonLocale,
    /// S12 — **A-2** — un contenu incompatible n'atteste jamais l'effet et ne
    /// rend jamais le cycle ré-publiable.
    S12IncompatibleNattestePas,
    /// S13 — **A-2** — un effet n'est jamais présent dans deux backends.
    S13EffetDuplique,
    /// L1 — pas de blocage : tout état non terminal a une transition effective.
    L1Blocage,
    /// L2 — absence attestée ⟹ nouvelle tentative possible.
    L2PasDeReprise,
    /// L3 — un état terminal est atteignable depuis tout état exploré.
    L3TerminalInatteignable,
    /// L4 — le rejeu (même opération, même observation) est idempotent.
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
            Invariant::S11AttestationNonLocale => "S11 attestation non locale au backend",
            Invariant::S12IncompatibleNattestePas => "S12 contenu incompatible attesté",
            Invariant::S13EffetDuplique => "S13 effet présent dans deux backends",
            Invariant::L1Blocage => "L1  blocage",
            Invariant::L2PasDeReprise => "L2  pas de reprise après absence attestée",
            Invariant::L3TerminalInatteignable => "L3  terminal inatteignable",
            Invariant::L4RejeuNonIdempotent => "L4  rejeu non idempotent",
            Invariant::L5TentativeSansObservation => "L5  tentative sans observation",
        }
    }
}

const INVARIANTS_SURETE: [Invariant; 13] = [
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
    Invariant::S11AttestationNonLocale,
    Invariant::S12IncompatibleNattestePas,
    Invariant::S13EffetDuplique,
];

const INVARIANTS_PROGRESSION: [Invariant; 5] = [
    Invariant::L1Blocage,
    Invariant::L2PasDeReprise,
    Invariant::L3TerminalInatteignable,
    Invariant::L4RejeuNonIdempotent,
    Invariant::L5TentativeSansObservation,
];

/// Les invariants **d'état** : ce qui doit être vrai de tout état atteignable.
fn violations_etat(m: &Monde) -> Vec<Invariant> {
    let mut v = Vec::new();
    for (d, c) in &m.cycles {
        match c.s {
            S::Confirme => {
                // S2 — l'effet est attesté dans le backend qui le porte, par ce
                // backend, et l'attestation nomme une publication de d.
                let atteste = match (c.b, c.r.effet) {
                    (Some(bk), Some(k)) => {
                        m.effet(*d, bk) == E::Present && k.dec == *d && c.r.par == Some(bk)
                    }
                    _ => false,
                };
                if !atteste {
                    v.push(Invariant::S2ConfirmationSansAttestation);
                }
            }
            S::Abandonne => {
                // S3 — « inconnu nulle part » : on ne déclare pas abandonné ce
                // qui peut exister dans n'importe quel magasin.
                if BACKENDS.iter().any(|b| m.effet(*d, *b) == E::Inconnu) || c.vivante.is_some() {
                    v.push(Invariant::S3AbandonMasqueEffet);
                }
            }
            S::Indetermine => {
                let ignorance = c.b.is_some_and(|bk| m.effet(*d, bk) == E::Inconnu);
                if !ignorance {
                    v.push(Invariant::S7IndetermineSansIgnorance);
                }
            }
            _ => {}
        }
        // S6 : une publication vivante si et seulement si une est engagée.
        let attendu = matches!(c.s, S::EnCours | S::Indetermine);
        if attendu != c.vivante.is_some() {
            v.push(Invariant::S6TentativeVivante);
        }
    }
    // S13 — pas de duplication d'effet entre magasins.
    for d in m.cycles.keys() {
        let presents = BACKENDS
            .iter()
            .filter(|b| m.effet(*d, **b) == E::Present)
            .count();
        if presents > 1 {
            v.push(Invariant::S13EffetDuplique);
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
        for ((autre, bk), e) in &avant.effets {
            if *autre != d && apres.effets.get(&(*autre, *bk)) != Some(e) {
                v.push(Invariant::S5ProvenanceNonVivanteModifie);
            }
        }
    }

    // S4 et S5 — pour les actions qui PORTENT une provenance.
    if let Some(p) = provenance_portee(a)
        && let Some(d) = decision_visee(a)
    {
        let vivante = avant.cycles.get(&d).and_then(|c| c.vivante);
        if issue.positive() && vivante != Some(p.k) {
            v.push(Invariant::S4ReponsePositiveSansProvenanceVivante);
        }
        if vivante != Some(p.k) && avant != apres {
            v.push(Invariant::S5ProvenanceNonVivanteModifie);
        }
    }

    // S11 — une attestation ne vaut que dans le backend qui a engagé la
    // publication ; réciproquement, une attestation étrangère ne modifie rien.
    if let Some((d, par)) = attestation(a) {
        let porteur = avant.cycles.get(&d).and_then(|c| c.b);
        if porteur != Some(par) {
            if issue.positive() {
                v.push(Invariant::S11AttestationNonLocale);
            }
            if avant != apres {
                v.push(Invariant::S11AttestationNonLocale);
            }
        }
    }

    // S12 — un contenu incompatible ni n'atteste, ni ne rend ré-publiable.
    if let Action::Resoudre {
        d,
        verdict: Verdict::Incompatible,
        ..
    } = a
    {
        let avant_c = avant.cycles.get(&d);
        let apres_c = apres.cycles.get(&d);
        if let Some(c) = apres_c {
            let avant_s = avant_c.map(|x| x.s);
            // « Ne rend jamais ré-publiable » : le seul passage interdit est
            // celui qui OUVRE la reprise (Recu) ou l'attestation (Confirme). Un
            // cycle déjà Recu ou déjà Confirme n'est pas un franchissement.
            let ouvre_la_reprise = c.s == S::Recu && avant_s != Some(S::Recu);
            let atteste = c.s == S::Confirme && avant_s != Some(S::Confirme);
            if ouvre_la_reprise || atteste {
                v.push(Invariant::S12IncompatibleNattestePas);
            }
            if Some(c.r) != avant_c.map(|x| x.r) {
                v.push(Invariant::S12IncompatibleNattestePas);
            }
        }
    }

    // S8 — on ne réserve que depuis Recu.
    if let Action::Reserver { d, .. } = a
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
        | Action::Reserver { d, .. }
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

/// Les actions qui ATTESTENT : elles nomment un backend et prétendent dire ce
/// que contient son magasin.
fn attestation(a: Action) -> Option<(Dec, Back)> {
    match a {
        Action::Confirmer { d, par, .. } | Action::EchecCertain { d, par, .. } => Some((d, par)),
        Action::Resoudre {
            d,
            par,
            verdict: Verdict::EffetConfirme | Verdict::AbsenceDefinitive,
            ..
        } => Some((d, par)),
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
            for rang in RANGS_PRESENTES {
                v.push(Prov {
                    aut,
                    k: K { dec, rang },
                });
            }
        }
    }
    v
}

fn provenances_propres(d: Dec) -> Vec<Prov> {
    RANGS_PRESENTES
        .iter()
        .map(|&rang| Prov {
            aut: AUTORITE,
            k: K { dec: d, rang },
        })
        .collect()
}

/// Les transitions qui PORTENT une provenance donnée. Elle peut être vivante,
/// périmée, étrangère ou croisée : c'est l'action, pas le modèle, qui décide.
/// Chaque action d'attestation est proposée avec CHAQUE backend : c'est là que
/// B et V se séparent.
fn actions_de_provenance(d: Dec, p: Prov) -> Vec<Action> {
    let mut v = Vec::new();
    for par in BACKENDS {
        v.push(Action::Confirmer { d, p, par });
        v.push(Action::EchecCertain { d, p, par });
        for verdict in VERDICTS {
            v.push(Action::Resoudre { d, p, par, verdict });
        }
    }
    // `indeterminer` et `abandonner` ne nomment aucun backend : ce ne sont pas
    // des attestations.
    v.push(Action::Indeterminer { d, p });
    v.push(Action::Abandonner { d, p });
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
        for backend in BACKENDS {
            a.push(Action::Reserver { d, backend });
        }
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

    /// Index (état source, action) → réponse, pour interroger deux variantes
    /// d'une même action depuis le même état.
    fn index(&self) -> BTreeMap<(usize, Action), Issue> {
        self.aretes
            .iter()
            .map(|(i, a, _, issue)| ((*i, *a), *issue))
            .collect()
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

    /// Deux actions de MÊME VERDICT mais d'AUTORITÉ BACKEND DIFFÉRENTE, depuis
    /// le même état, avec des réponses différentes. C'est la preuve mesurée que
    /// B n'est pas V : si B était le verdict, ces deux actions seraient la même.
    fn meme_verdict_deux_backends(&self) -> Option<(usize, Action, Action)> {
        let index = self.index();
        for i in 0..self.noeuds.len() {
            if !self.noeuds[i]
                .monde
                .cycles
                .values()
                .any(|c| c.vivante.is_some())
            {
                continue;
            }
            for (d, c) in &self.noeuds[i].monde.cycles {
                let Some(k) = c.vivante else { continue };
                let p = Prov { aut: AUTORITE, k };
                for verdict in VERDICTS {
                    let a0 = Action::Resoudre {
                        d: *d,
                        p,
                        par: BACKEND,
                        verdict,
                    };
                    let a1 = Action::Resoudre {
                        d: *d,
                        p,
                        par: 1 - BACKEND,
                        verdict,
                    };
                    let (Some(i0), Some(i1)) = (index.get(&(i, a0)), index.get(&(i, a1))) else {
                        continue;
                    };
                    if i0.positive() != i1.positive() {
                        return Some((i, a0, a1));
                    }
                }
            }
        }
        None
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
        // L'alphabet de chaque état inclut SA publication vivante : sans quoi le
        // cycle cesse d'avancer dès que son rang dépasse ceux de l'alphabet fixe.
        for (d, c) in &avant.cycles {
            if let Some(k) = c.vivante {
                actions.extend(actions_de_provenance(*d, Prov { aut: AUTORITE, k }));
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
        Action::Reserver { d, backend } => format!("reserver(d{d},b{backend})"),
        Action::Confirmer { d, p, par } => {
            format!("confirmer(d{d},{},b{par})", prov_lisible(p))
        }
        Action::EchecCertain { d, p, par } => {
            format!("echec_certain(d{d},{},b{par})", prov_lisible(p))
        }
        Action::Indeterminer { d, p } => format!("indeterminer(d{d},{})", prov_lisible(p)),
        Action::Resoudre { d, p, par, verdict } => {
            let v = match verdict {
                Verdict::EffetConfirme => "effet_confirme",
                Verdict::AbsenceDefinitive => "absence_definitive",
                Verdict::Indetermine => "indetermine",
                Verdict::Incompatible => "incompatible",
            };
            format!("resoudre(d{d},{},b{par},{v})", prov_lisible(p))
        }
        Action::Abandonner { d, p } => format!("abandonner(d{d},{})", prov_lisible(p)),
        Action::AbandonSansTentative { d } => format!("abandon_sans_tentative(d{d})"),
        Action::DemanderAnnulation { d } => format!("demander_annulation(d{d})"),
    }
}

/// K se lit comme une identité : l'opération et le rang de la publication, pas
/// un simple numéro.
fn prov_lisible(p: &Prov) -> String {
    format!("a{}/k({},g{})", p.aut, p.k.dec, p.k.rang)
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

/// Projection comparable d'un état : (opération, S, O, C). C'est exactement ce
/// que l'implémentation porte et expose ; ni K, ni B, ni G, ni R n'y figurent,
/// parce qu'elle ne les porte pas de façon lisible.
type Projection = Vec<(Dec, S, Option<(String, String, String)>, bool)>;

fn projection(m: &Monde) -> Projection {
    m.cycles
        .iter()
        .map(|(d, c)| (*d, c.s, c.o.map(|o| cle_obs_modele(&o)), c.c))
        .collect()
}

/// Pourquoi une action du modèle n'est pas exprimable contre l'implémentation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum NonExprimable {
    /// L'implémentation ne porte aucune autorité backend : seul `BACKEND` est
    /// exprimable.
    Backend,
    /// `Tentative` est opaque : une provenance que cette instance n'a pas
    /// émise ne peut pas être présentée.
    Provenance,
}

/// L'implémentation réelle, vue par le modèle. Les `Tentative` sont opaques :
/// on ne les fabrique pas, on conserve seulement celles que `reserver` a
/// réellement émises, indexées par l'identité K que le modèle leur donne.
struct Reel {
    a: reel::Attribution,
    t: BTreeMap<Prov, reel::Tentative>,
    rang: BTreeMap<Dec, u64>,
}

impl Reel {
    fn neuf() -> Self {
        Self {
            a: reel::Attribution::new(),
            t: BTreeMap::new(),
            rang: BTreeMap::new(),
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

    fn appliquer(&mut self, a: Action) -> Result<Issue, NonExprimable> {
        // Tout backend nommé autre que celui que l'implémentation porte est hors
        // périmètre : elle n'a pas de variable pour le recevoir.
        if let Some(par) = backend_nomme(a)
            && par != BACKEND
        {
            return Err(NonExprimable::Backend);
        }
        Ok(match a {
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
            Action::Reserver { d, .. } => match self.a.reserver(d as u128) {
                Some(t) => {
                    let rang = self.rang.entry(d).or_insert(0);
                    *rang += 1;
                    self.t.insert(
                        Prov {
                            aut: AUTORITE,
                            k: K {
                                dec: d,
                                rang: *rang,
                            },
                        },
                        t,
                    );
                    Issue::Oui
                }
                None => Issue::Non,
            },
            Action::Confirmer { d, p, .. } => {
                let t = *self.t.get(&p).ok_or(NonExprimable::Provenance)?;
                b(self.a.confirmer(d as u128, t))
            }
            Action::EchecCertain { d, p, .. } => {
                let t = *self.t.get(&p).ok_or(NonExprimable::Provenance)?;
                b(self.a.echec_certain(d as u128, t))
            }
            Action::Indeterminer { d, p } => {
                let t = *self.t.get(&p).ok_or(NonExprimable::Provenance)?;
                b(self.a.indetermine(d as u128, t))
            }
            Action::Resoudre { d, p, verdict, .. } => {
                let t = *self.t.get(&p).ok_or(NonExprimable::Provenance)?;
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

/// Le backend qu'une action NOMME, s'il en nomme un. C'est le point de contrôle
/// unique du périmètre : l'implémentation n'en porte qu'un.
fn backend_nomme(a: Action) -> Option<Back> {
    match a {
        Action::Reserver { backend, .. } => Some(backend),
        Action::Confirmer { par, .. }
        | Action::EchecCertain { par, .. }
        | Action::Resoudre { par, .. } => Some(par),
        _ => None,
    }
}

fn rejouer(trace: &[Action]) -> Result<Reel, NonExprimable> {
    let mut r = Reel::neuf();
    for a in trace {
        r.appliquer(*a)?;
    }
    Ok(r)
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

/// Profondeur d'exploration. Elle doit laisser apparaître le contre-exemple de
/// `dd6382be` (4 transitions) et celui de la conflation K/G (7 transitions,
/// deux opérations), tout en restant bornée.
const PROFONDEUR: usize = 10;
const PROFONDEUR_DEUX: usize = 7;

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

/// L'exploration de référence avec DEUX opérations : indispensable pour
/// distinguer K de D (deux opérations de contenu identique) et K de G (deux
/// publications de même rang).
fn reference_deux() -> &'static Exploration {
    static REF: OnceLock<Exploration> = OnceLock::new();
    REF.get_or_init(|| {
        let mut provenances = Vec::new();
        for d in DECISIONS {
            provenances.extend(provenances_propres(d));
        }
        explorer(
            &Semantique::REFERENCE,
            &DECISIONS,
            &provenances,
            PROFONDEUR_DEUX,
        )
    })
}

fn variante(sem: &Semantique) -> Exploration {
    explorer(sem, &[0], &toutes_provenances(), PROFONDEUR)
}

fn variante_deux(sem: &Semantique) -> Exploration {
    let mut provenances = Vec::new();
    for d in DECISIONS {
        provenances.extend(provenances_propres(d));
    }
    explorer(sem, &DECISIONS, &provenances, PROFONDEUR_DEUX)
}

fn manques(e: &Exploration, invariants: &[Invariant]) -> Vec<String> {
    invariants
        .iter()
        .filter_map(|inv| {
            e.contre_exemple(*inv)
                .map(|cx| format!("{} → {}", inv.nom(), cx.lisible()))
        })
        .collect()
}

#[test]
fn modele_reference_ne_viole_aucun_invariant() {
    let e = reference();
    assert!(
        e.noeuds.len() > 50,
        "exploration suspicieusement vide : {} états",
        e.noeuds.len()
    );
    let surete = manques(e, &INVARIANTS_SURETE);
    assert!(
        surete.is_empty(),
        "la référence doit satisfaire S1–S13 :\n{}",
        surete.join("\n")
    );
    let progression = manques(e, &INVARIANTS_PROGRESSION);
    assert!(
        progression.is_empty(),
        "la référence doit satisfaire L1–L5 :\n{}",
        progression.join("\n")
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
        "un contenu différent sur la même opération doit produire un CONFLIT"
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
    // …et une observation d'un AUTRE contenu est refusée comme incompatible
    assert!(
        e.aretes
            .iter()
            .any(|(_, a, _, i)| matches!(a, Action::Recevoir { .. }) && *i == Issue::Incompatible),
        "une observation dont le contenu ne correspond pas à l'action servie doit être incompatible"
    );

    // 2 autorités (P) : une provenance étrangère n'est JAMAIS acceptée
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

    // 2 rangs (G) : une opération avec plusieurs publications
    assert!(
        e.etat_atteint(|m| m.cycles.values().any(|c| c.g >= 2)),
        "le rang 2 doit être atteint"
    );
    assert!(
        e.action_acceptee(|a| provenance_portee(a).is_some_and(|p| p.k.rang == 2)),
        "une publication de rang 2 doit pouvoir être attestée"
    );

    // 2 opérations : une provenance croisée est refusée
    assert!(
        !e.action_acceptee(
            |a| provenance_portee(a).is_some_and(|p| p.k.dec != decision_visee(a).unwrap())
        ),
        "une provenance croisée (autre opération) ne doit jamais être acceptée"
    );

    // 2 backends (B) : le second est réellement exploré, mais aucune attestation
    // portée par un backend qui n'est pas celui de la publication n'est acceptée.
    assert!(
        e.action_acceptee(|a| matches!(a, Action::Reserver { backend: 1, .. })),
        "le second backend doit être engageable"
    );
    let backends_refuses = e.aretes.iter().filter(|(i, a, _, issue)| {
        let Some((d, par)) = attestation(*a) else {
            return false;
        };
        !issue.positive() && e.noeuds[*i].monde.backend_de(d) != Some(par)
    });
    assert!(
        backends_refuses.count() > 0,
        "l'alphabet doit réellement présenter des attestations d'un autre backend"
    );
    // …et AUCUNE attestation acceptée ne vient d'un backend qui ne porte pas
    // l'effet de cette opération-LÀ (le backend porteur dépend de l'opération et
    // de sa publication courante : c'est pourquoi on le lit dans l'état source).
    let attestations_etrangeres: Vec<String> = e
        .aretes
        .iter()
        .filter(|(i, a, _, issue)| {
            let Some((d, par)) = attestation(*a) else {
                return false;
            };
            issue.positive() && e.noeuds[*i].monde.backend_de(d) != Some(par)
        })
        .map(|(i, a, _, _)| format!("{} ← {}", action_lisible(a), trace_lisible(&e.trace_de(*i))))
        .collect();
    assert!(
        attestations_etrangeres.is_empty(),
        "des attestations d'un backend qui ne porte pas l'effet ont été acceptées :\n{}",
        attestations_etrangeres.join("\n")
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

    // contenu incompatible : jamais accepté, jamais attesté (S12)
    assert!(
        !e.action_acceptee(|a| matches!(
            a,
            Action::Resoudre {
                verdict: Verdict::Incompatible,
                ..
            }
        )),
        "un contenu incompatible ne doit jamais obtenir de réponse positive"
    );
    assert!(
        e.aretes.iter().any(|(_, a, _, _)| matches!(
            a,
            Action::Resoudre {
                verdict: Verdict::Incompatible,
                ..
            }
        )),
        "l'alphabet doit réellement présenter des verdicts Incompatible"
    );

    // annulation
    assert!(
        e.etat_atteint(|m| m.cycles.values().any(|c| c.c)),
        "l'annulation doit être demandable"
    );

    // réponse périmée : une provenance de rang 1 refusée alors que la publication
    // vivante est de rang 2.
    let perimee = e.aretes.iter().any(|(i, a, _, issue)| {
        provenance_portee(*a).is_some_and(|p| p.k.rang == 1 && p.aut == AUTORITE && p.k.dec == 0)
            && !issue.positive()
            && e.noeuds[*i]
                .monde
                .cycles
                .get(&0)
                .is_some_and(|c| c.g == 2 && c.s == S::EnCours)
    });
    assert!(
        perimee,
        "une réponse périmée doit être refusée sur une publication vivante plus récente"
    );
}

#[test]
fn k_est_une_identite_stable_distincte_de_s_et_de_g() {
    let sem = Semantique::REFERENCE;
    let p = |k| Prov { aut: AUTORITE, k };

    // ---- K survit aux changements d'état (K ≠ S) ----
    let mut m = Monde::default();
    m.record(0, CONTENUS[0]);
    m.recevoir(0, OBSERVATIONS[0]);
    let k1 = m.reserver(0, BACKEND).expect("publication");
    assert_eq!(k1, K { dec: 0, rang: 1 });
    assert_eq!(m.cycles[&0].s, S::EnCours);
    assert_eq!(m.cycles[&0].vivante, Some(k1));
    assert!(m.indeterminer(0, p(k1), &sem));
    assert_eq!(m.cycles[&0].s, S::Indetermine);
    assert_eq!(
        m.cycles[&0].vivante,
        Some(k1),
        "la même identité K survit au changement d'état"
    );
    assert!(m.resoudre(0, p(k1), BACKEND, Verdict::EffetConfirme, &sem));
    assert_eq!(m.cycles[&0].s, S::Confirme);
    assert_eq!(
        m.cycles[&0].r.effet,
        Some(k1),
        "l'attestation nomme l'identité de publication engagée pour de vrai"
    );
    println!(
        "K stable : K({},g{}) vivante en EnCours, en Indetermine, et attestée en Confirme",
        k1.dec, k1.rang
    );

    // ---- K ≠ D : deux opérations de contenu IDENTIQUE ----
    let mut m = Monde::default();
    m.record(0, CONTENUS[0]);
    m.record(1, CONTENUS[0]);
    m.recevoir(0, OBSERVATIONS[0]);
    m.recevoir(1, OBSERVATIONS[0]);
    assert_eq!(
        m.cycles[&0].def, m.cycles[&1].def,
        "les deux opérations ont bien le MÊME contenu"
    );
    let a = m.reserver(0, BACKEND).unwrap();
    let b = m.reserver(1, BACKEND).unwrap();
    assert_ne!(a, b, "deux publications, deux identités, même contenu");
    assert_eq!(a.rang, b.rang, "…et le même rang : G ne les sépare pas");
    println!(
        "K ≠ D : deux opérations de contenu identique → K({},{}) et K({},{})",
        a.dec, a.rang, b.dec, b.rang
    );

    // ---- K ≠ G : le rang seul ne désigne pas une publication ----
    // La publication vivante de l'opération 1 est K(1,1). Présenter K(0,1) — même
    // rang, autre opération — ne la règle pas.
    assert!(
        !m.confirmer(1, p(K { dec: 0, rang: 1 }), BACKEND, &sem),
        "un rang identique dans une autre opération ne désigne pas cette publication"
    );
    assert_eq!(
        m.cycles[&1].vivante,
        Some(b),
        "la publication de l'opération 1 est intacte"
    );
    assert!(m.confirmer(1, p(b), BACKEND, &sem));
    println!("K ≠ G : K(0,1) présentée à l'opération 1 (vivante K(1,1)) → refusée");

    // ---- une opération, plusieurs générations ----
    let mut m = Monde::default();
    m.record(0, CONTENUS[0]);
    m.recevoir(0, OBSERVATIONS[0]);
    let g1 = m.reserver(0, BACKEND).unwrap();
    assert!(m.echec_certain(0, p(g1), BACKEND, &sem));
    assert_eq!(m.cycles[&0].s, S::Recu);
    let g2 = m.reserver(0, BACKEND).unwrap();
    assert_ne!(g1, g2, "une nouvelle publication est une identité neuve");
    assert_eq!((g1.dec, g2.dec), (0, 0), "…de la même opération");
    assert_eq!(
        (g1.rang, g2.rang),
        (1, 2),
        "…et le compteur G a bien avancé"
    );
    assert_eq!(m.cycles[&0].g, 2);
    // Une réponse périmée (rang 1) ne règle pas la publication vivante (rang 2).
    assert!(
        !m.confirmer(0, p(g1), BACKEND, &sem),
        "une réponse périmée ne règle pas la publication vivante"
    );
    assert!(m.confirmer(0, p(g2), BACKEND, &sem));
    println!("Une opération, deux publications : K(0,g1) puis K(0,g2), la périmée est refusée");

    // ---- sur l'exploration : les deux opérations coexistent, publications
    //      distinctes et contenu identique ----
    let e = reference_deux();
    assert!(
        e.etat_atteint(|m| {
            m.cycles.len() == 2
                && m.cycles[&0].def == m.cycles[&1].def
                && m.cycles[&0].vivante.is_some()
                && m.cycles[&1].vivante.is_some()
                && m.cycles[&0].vivante != m.cycles[&1].vivante
        }),
        "un état atteint doit porter deux publications distinctes de contenu identique"
    );
}

#[test]
fn changement_de_backend_apres_indetermination() {
    // L'exercice, dans ses deux versants : ce qui est interdit pendant
    // l'indétermination, et ce qui devient légitime après une absence ATTESTÉE.
    let sem = Semantique::REFERENCE;
    let p = |k| Prov { aut: AUTORITE, k };
    let autre = 1 - BACKEND;

    let mut m = Monde::default();
    m.record(0, CONTENUS[0]);
    m.recevoir(0, OBSERVATIONS[0]);
    let k1 = m
        .reserver(0, BACKEND)
        .expect("publication sur le backend 0");
    assert!(m.indeterminer(0, p(k1), &sem));
    assert_eq!(m.cycles[&0].s, S::Indetermine);
    let avant = m.clone();

    // (i) Re-publier pendant l'indétermination : refusé, et pas seulement sur le
    // même backend. L'effet du premier magasin n'est pas réglé ; en engager un
    // second dupliquerait l'effet (S8 + S13).
    let mut tentative = avant.clone();
    for backend in BACKENDS {
        assert!(
            tentative.reserver(0, backend).is_none(),
            "aucune ré-publication pendant Indetermine, même vers le backend {backend}"
        );
    }
    assert_eq!(tentative, avant, "et aucune n'a rien modifié");

    // (ii) Un verdict de l'AUTRE backend ne règle rien et ne modifie rien…
    let mut etranger = avant.clone();
    for verdict in VERDICTS {
        assert!(
            !etranger.resoudre(0, p(k1), autre, verdict, &sem),
            "le backend {autre} n'a pas engagé la publication : {verdict:?} ne la règle pas"
        );
    }
    assert_eq!(etranger, avant, "…et l'indétermination reste entière");
    // …alors que le même verdict, du backend porteur, la règle.
    let mut porteur = avant.clone();
    assert!(porteur.resoudre(0, p(k1), BACKEND, Verdict::AbsenceDefinitive, &sem));
    assert_eq!(porteur.cycles[&0].s, S::Recu);

    // (iii) Après une absence attestée PAR LE PORTEUR, changer de backend est
    // légitime : rien n'existe dans le premier magasin.
    assert_eq!(
        porteur.effet(0, BACKEND),
        E::Aucun,
        "le porteur a attesté l'absence"
    );
    let k2 = porteur
        .reserver(0, autre)
        .expect("publication sur l'autre backend");
    assert_eq!(
        k2,
        K { dec: 0, rang: 2 },
        "une publication NEUVE, jamais un recyclage d'identité"
    );
    assert_eq!(
        porteur.cycles[&0].b,
        Some(autre),
        "le backend de l'effet a changé"
    );
    assert_eq!(
        porteur.effet(0, autre),
        E::Inconnu,
        "le second magasin est maintenant engagé"
    );
    assert_eq!(
        porteur.effet(0, BACKEND),
        E::Aucun,
        "le premier magasin n'a jamais reçu cet effet — d'où S13"
    );
    // L'attestation doit venir du NOUVEAU porteur : l'ancien a perdu la parole
    // sur cet effet.
    assert!(
        !porteur.confirmer(0, p(k2), BACKEND, &sem),
        "l'ancien backend ne peut plus attester l'effet engagé ailleurs"
    );
    assert!(porteur.confirmer(0, p(k2), autre, &sem));
    assert_eq!(porteur.cycles[&0].r.par, Some(autre));

    let presents: Vec<Back> = BACKENDS
        .iter()
        .copied()
        .filter(|b| porteur.effet(0, *b) == E::Present)
        .collect();
    assert_eq!(
        presents,
        vec![autre],
        "l'effet n'existe que dans un magasin"
    );
    println!(
        "changement de backend : refusé pendant Indetermine, légitime après absence attestée \
         (K(0,g1)/b{BACKEND} → K(0,g{} )/b{autre})",
        k2.rang
    );
}

#[test]
fn b_est_une_autorite_distincte_du_verdict() {
    let sem = Semantique::REFERENCE;
    let mut m = Monde::default();
    m.record(0, CONTENUS[0]);
    m.recevoir(0, OBSERVATIONS[0]);
    let k = m.reserver(0, BACKEND).expect("publication");
    let p = Prov { aut: AUTORITE, k };
    let autre = 1 - BACKEND;

    // Le MÊME verdict, rendu par deux backends, n'a pas le même effet : preuve
    // mesurée que B n'est pas V.
    for verdict in VERDICTS {
        let mut chez_le_porteur = m.clone();
        let mut chez_un_autre = m.clone();
        let ici = chez_le_porteur.resoudre(0, p, BACKEND, verdict, &sem);
        let la_bas = chez_un_autre.resoudre(0, p, autre, verdict, &sem);
        assert!(
            !la_bas,
            "le backend {autre} n'a pas engagé cette publication ; son verdict {verdict:?} ne la règle pas"
        );
        assert_eq!(
            chez_un_autre, m,
            "un verdict d'un autre backend ne modifie RIEN ({verdict:?})"
        );
        if verdict != Verdict::Incompatible {
            assert!(
                ici,
                "le backend porteur, lui, règle la publication ({verdict:?})"
            );
        }
    }

    // L'attestation enregistre l'autorité qui a répondu.
    let mut m2 = m.clone();
    assert!(m2.resoudre(0, p, BACKEND, Verdict::EffetConfirme, &sem));
    assert_eq!(m2.cycles[&0].r.par, Some(BACKEND));
    assert_eq!(m2.cycles[&0].r.effet, Some(k));

    // Deux backends portant le MÊME verdict sur DEUX opérations : B distingue ce
    // que V confond.
    let mut m3 = Monde::default();
    m3.record(0, CONTENUS[0]);
    m3.record(1, CONTENUS[0]);
    m3.recevoir(0, OBSERVATIONS[0]);
    m3.recevoir(1, OBSERVATIONS[0]);
    let k0 = m3.reserver(0, 0).unwrap();
    let k1 = m3.reserver(1, 1).unwrap();
    assert!(m3.resoudre(
        0,
        Prov {
            aut: AUTORITE,
            k: k0
        },
        0,
        Verdict::EffetConfirme,
        &sem
    ));
    assert!(m3.resoudre(
        1,
        Prov {
            aut: AUTORITE,
            k: k1
        },
        1,
        Verdict::EffetConfirme,
        &sem
    ));
    assert_eq!(m3.cycles[&0].r.par, Some(0));
    assert_eq!(m3.cycles[&1].r.par, Some(1));
    assert_eq!(
        m3.cycles[&0].r.effet.map(|k| k.dec),
        Some(0),
        "même verdict, même rang, deux publications : B et K disent laquelle"
    );

    // Et sur l'exploration : même verdict, deux backends, réponses différentes.
    let e = reference();
    let (i, a0, a1) = e
        .meme_verdict_deux_backends()
        .expect("l'exploration doit contenir un verdict identique rendu par deux backends");
    println!(
        "B ≠ V : depuis l'état {}, {} et {} n'ont pas la même réponse",
        trace_lisible(&e.trace_de(i)),
        action_lisible(&a0),
        action_lisible(&a1)
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
}

#[test]
fn famille_indetermine_sans_validation_retrouvee_en_s4() {
    // ⚑ e1e6e611 (b) : `resoudre(Indetermine) => true`.
    let e = variante(&Semantique::INDETERMINE_SANS_VALIDATION);
    let cx = e
        .contre_exemple(Invariant::S4ReponsePositiveSansProvenanceVivante)
        .expect("la référence défectueuse doit violer S4");
    // Le défaut est l'absence TOTALE de validation : il se manifeste dès la
    // première transition, sur une opération qui n'existe même pas.
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
fn famille_identite_non_verifiee_retrouvee_en_s5() {
    // ⚑ dd6382be : l'identité de la publication réglée n'est pas contrôlée dans
    // `resoudre` (le commit disait « génération »).
    let e = variante(&Semantique::IDENTITE_NON_VERIFIEE);
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
        "la transition fautive est une résolution à identité non vérifiée : {}",
        cx.lisible()
    );
    // La provenance présentée porte un rang que la publication vivante n'a
    // jamais eu : c'est le contrôle manquant.
    let presentee = cx
        .trace
        .iter()
        .filter_map(|a| provenance_portee(*a))
        .next_back()
        .unwrap();
    assert_ne!(
        presentee.k.rang,
        1,
        "le rang présenté doit différer de celui de la publication vivante : {}",
        cx.lisible()
    );

    // Le scénario exact du commit — une ANCIENNE publication (rang 1) présentée
    // après une nouvelle réservation (rang 2) — est aussi détecté, plus tard.
    let perimee = e.aretes.iter().any(|(i, a, j, _)| {
        matches!(
            a,
            Action::Resoudre {
                verdict: Verdict::Incompatible,
                ..
            }
        ) && provenance_portee(*a).is_some_and(|p| p.k.rang == 1 && p.aut == AUTORITE)
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
        "une ancienne publication ne doit pas pouvoir modifier le cycle de la vivante"
    );
}

#[test]
fn famille_attestation_sans_backend_retrouvee_en_s11() {
    // ⚑ A-2 : B confondu avec V — n'importe quel backend peut attester.
    let e = variante(&Semantique::ATTESTATION_SANS_BACKEND);
    let cx = e
        .contre_exemple(Invariant::S11AttestationNonLocale)
        .expect("la variante doit violer S11");
    assert_eq!(
        cx.profondeur(),
        4,
        "contre-exemple minimal attendu en 4 transitions : {}",
        cx.lisible()
    );
    let (_, par) = attestation(cx.trace[3]).expect("la transition fautive est une attestation");
    assert_ne!(
        par,
        BACKEND,
        "l'attestation fautive vient d'un backend qui n'a pas engagé la publication : {}",
        cx.lisible()
    );
    // Le backend porteur est celui qui a réservé, et c'est un AUTRE.
    let reserve = cx
        .trace
        .iter()
        .find_map(|a| match a {
            Action::Reserver { backend, .. } => Some(*backend),
            _ => None,
        })
        .expect("une réservation précède l'attestation");
    assert_eq!(
        reserve, BACKEND,
        "la publication a été engagée dans le backend 0"
    );
    assert_ne!(reserve, par, "…et l'attestation vient du backend 1");
    println!(
        "famille A-2 « attestation sans backend » : S11 en {} transitions → {}",
        cx.profondeur(),
        cx.lisible()
    );
}

#[test]
fn conflation_k_g_est_detectee() {
    // ⚑ A-2 : K réduit au rang — exactement la conflation que la table d'A-1
    // écrivait (« K = Cycle::s ») et qu'aucune de ses variantes ne pouvait
    // révéler, faute d'identité de publication.
    let e = variante_deux(&Semantique::IDENTITE_REDUITE_AU_RANG);
    let cx = e
        .contre_exemple(Invariant::S4ReponsePositiveSansProvenanceVivante)
        .or_else(|| e.contre_exemple(Invariant::S5ProvenanceNonVivanteModifie))
        .expect("réduire l'identité au rang doit être détecté");
    assert_eq!(
        cx.profondeur(),
        4,
        "contre-exemple minimal attendu en 4 transitions : {}",
        cx.lisible()
    );
    assert!(
        matches!(cx.trace[3], Action::Confirmer { .. }),
        "la transition fautive est une attestation : {}",
        cx.lisible()
    );
    // Le contre-exemple est plus court que prévu, et c'est le résultat : il
    // suffit de présenter une publication d'une AUTRE opération, de même rang,
    // pour que le contrôle réduit au rang l'accepte. Le rang ne dit pas de quel
    // compteur il est le rang — c'est tout le propos.
    let presentee = cx
        .trace
        .iter()
        .filter_map(|a| provenance_portee(*a))
        .next_back()
        .expect("une provenance");
    let cible = decision_visee(cx.trace[3]).unwrap();
    assert_ne!(
        presentee.k.dec,
        cible,
        "la publication attestée appartient à une AUTRE opération — et le rang seul ne le voit pas : {}",
        cx.lisible()
    );
    assert_eq!(
        presentee.k.rang,
        1,
        "…mais son rang est celui de la publication vivante : {}",
        cx.lisible()
    );
    // Dans la référence, la même action est refusée : c'est la comparaison
    // d'identité, et non le rang, qui protège.
    let mut m = Monde::default();
    let sem = Semantique::REFERENCE;
    m.record(cible, CONTENUS[0]);
    m.recevoir(cible, OBSERVATIONS[0]);
    let k = m.reserver(cible, BACKEND).unwrap();
    assert_eq!(k.rang, presentee.k.rang);
    assert!(
        !m.confirmer(
            cible,
            Prov {
                aut: AUTORITE,
                k: presentee.k
            },
            BACKEND,
            &sem
        ),
        "la référence refuse la même présentation : K sépare, G non"
    );
    println!(
        "famille A-2 « identité réduite au rang » : détectée en {} transitions → {}",
        cx.profondeur(),
        cx.lisible()
    );
}

#[test]
fn les_trois_familles_historiques_sont_distinctes() {
    let a = variante(&Semantique::ABANDON_DIRECT);
    let b = variante(&Semantique::INDETERMINE_SANS_VALIDATION);
    let c = variante(&Semantique::IDENTITE_NON_VERIFIEE);

    assert!(a.violate(Invariant::S3AbandonMasqueEffet));
    assert!(!a.violate(Invariant::S5ProvenanceNonVivanteModifie));
    assert!(!a.violate(Invariant::S11AttestationNonLocale));

    assert!(b.violate(Invariant::S4ReponsePositiveSansProvenanceVivante));
    assert!(!b.violate(Invariant::S3AbandonMasqueEffet));
    assert!(!b.violate(Invariant::S11AttestationNonLocale));

    assert!(c.violate(Invariant::S5ProvenanceNonVivanteModifie));
    assert!(!c.violate(Invariant::S3AbandonMasqueEffet));
    assert!(!c.violate(Invariant::S11AttestationNonLocale));

    // Discrimination entre (b) et (c) : (b) ne valide RIEN — il répond
    // positivement dès la première transition, sur une opération inexistante.
    // (c) valide encore l'autorité et l'existence du cycle ; seule l'identité de
    // la publication manque, donc il se trahit plus tard.
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
        p_c.k.rang,
        1,
        "(c) ne se trahit que sur un rang qui n'est pas celui de la publication vivante : {}",
        s4_c.lisible()
    );
}

#[test]
fn les_deux_familles_propres_a_a2_sont_distinctes() {
    let b = variante(&Semantique::ATTESTATION_SANS_BACKEND);
    let k = variante_deux(&Semantique::IDENTITE_REDUITE_AU_RANG);

    // B ≡ V se trahit sur S11, et sur S11 seulement : la provenance est vivante,
    // l'autorité est la bonne, il ne manque QUE l'identité du backend.
    assert!(b.violate(Invariant::S11AttestationNonLocale));
    assert!(
        !b.violate(Invariant::S4ReponsePositiveSansProvenanceVivante),
        "la provenance est vivante : ce n'est pas la famille de e1e6e611 (b)"
    );
    assert!(
        !b.violate(Invariant::S5ProvenanceNonVivanteModifie),
        "la provenance est vivante : ce n'est pas la famille de dd6382be"
    );
    // Elle finit PAR sur S3, et c'est une conséquence, pas une confusion : une
    // absence attestée par un autre backend laisse le vrai magasin inconnu, donc
    // l'abandon qui suit masque un effet possible. Le contre-exemple ne doit pas
    // passer par la transition supprimée en e1e6e611 (a).
    if let Some(cx) = b.contre_exemple(Invariant::S3AbandonMasqueEffet) {
        assert!(
            !cx.trace
                .iter()
                .any(|a| matches!(a, Action::Abandonner { .. })),
            "la famille B ne doit pas emprunter l'abandon direct : {}",
            cx.lisible()
        );
    }

    // K ≡ G se trahit sur S4/S5, jamais sur S11 : le backend est le bon, seule
    // l'identité de publication est réduite à un nombre.
    assert!(
        k.violate(Invariant::S4ReponsePositiveSansProvenanceVivante)
            || k.violate(Invariant::S5ProvenanceNonVivanteModifie)
    );
    assert!(
        !k.violate(Invariant::S11AttestationNonLocale),
        "le backend est respecté : ce n'est pas la famille de A-2 sur B"
    );
    assert!(!k.violate(Invariant::S3AbandonMasqueEffet));
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
                ..
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
                    "pas de reprise (L2) après absence attestée en d{d} rang {}",
                    p.k.rang
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
        if let Action::Reserver { d, .. } = a
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
    let e = reference_deux();

    let manques: Vec<String> = INVARIANTS_SURETE
        .iter()
        .chain(INVARIANTS_PROGRESSION.iter())
        .filter_map(|inv| {
            e.contre_exemple(*inv)
                .map(|cx| format!("{} → {}", inv.nom(), cx.lisible()))
        })
        .collect();
    assert!(
        manques.is_empty(),
        "violations avec deux opérations :\n{}",
        manques.join("\n")
    );
    assert!(
        e.etat_atteint(|m| m.cycles.len() == 2),
        "les deux opérations doivent coexister dans un état atteint"
    );
    assert!(
        e.etat_atteint(|m| {
            m.cycles.len() == 2
                && m.cycles.values().any(|c| c.s == S::Confirme)
                && m.cycles.values().any(|c| c.s == S::Recu)
        }),
        "les deux opérations doivent pouvoir être dans des états différents"
    );
    // …et chacune dans un backend différent.
    assert!(
        e.etat_atteint(|m| {
            m.cycles.len() == 2
                && m.cycles[&0].b.is_some()
                && m.cycles[&1].b.is_some()
                && m.cycles[&0].b != m.cycles[&1].b
        }),
        "les deux opérations doivent pouvoir vivre dans deux backends différents"
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
        let Ok(mut r) = rejouer(&trace) else {
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
        let Ok(issue_reelle) = r.appliquer(*a) else {
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
        comparees > 200,
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

    // Les trois familles HISTORIQUES : l'implémentation livrée les refuse toutes.
    // Les deux familles propres à A-2 (B ≡ V, K ≡ G) n'ont pas de contrepartie
    // rejouable : l'implémentation ne porte aucune autorité backend, et son
    // descripteur de tentative porte `decision_id`, donc elle ne réduit pas
    // l'identité au rang. Voir `perimetre_du_rejeu`.
    let familles: [(Semantique, &str); 3] = [
        (Semantique::ABANDON_DIRECT, "e1e6e611 (a) abandon direct"),
        (
            Semantique::INDETERMINE_SANS_VALIDATION,
            "e1e6e611 (b) Indetermine non validé",
        ),
        (
            Semantique::IDENTITE_NON_VERIFIEE,
            "dd6382be identité de publication non vérifiée",
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
            let Ok(mut r) = rejouer(&trace) else {
                continue;
            };
            // Le préfixe peut lui-même contenir une transition défectueuse : dans
            // ce cas l'implémentation réelle n'est pas dans l'état `avant` et la
            // comparaison n'aurait pas de sens.
            if r.projection() != projection(&avant) {
                continue;
            }
            let Ok(issue_reelle) = r.appliquer(*a) else {
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
    // opérations, les tentatives de l'une présentées à l'autre.
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

    // Décision croisée : la tentative de l'opération 1 présentée à l'opération 0
    // de la MÊME instance.
    let mut c = reel::Attribution::new();
    c.record(1, def_reelle(CONTENUS[0]));
    c.recevoir(1, obs_reelle(OBSERVATIONS[0]));
    let t1 = c.reserver(1).expect("réservation sur l'opération 1");
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

    // Backend étranger : l'implémentation ne porte AUCUNE autorité backend, donc
    // la sonde ne peut pas la sonder — elle constate le contrat manquant. Ce qui
    // est présentable, c'est un verdict dont on ne sait pas qui l'a rendu.
    println!(
        "sonde backend : `Attribution` n'expose aucun champ d'autorité backend ; \
         `RésolutionBackend` est un verdict sans émetteur — S11 est hors du \
         périmètre du rejeu (voir perimetre_du_rejeu)"
    );
}

#[test]
fn perimetre_du_rejeu() {
    let e = reference();
    let mut exprimes = 0usize;
    let mut par_backend = 0usize;
    let mut par_provenance = 0usize;

    for (i, a, _, _) in &e.aretes {
        let trace = e.trace_de(*i);
        match rejouer(&trace) {
            Err(NonExprimable::Backend) => par_backend += 1,
            Err(NonExprimable::Provenance) => par_provenance += 1,
            Ok(mut r) => match r.appliquer(*a) {
                Ok(_) => exprimes += 1,
                Err(NonExprimable::Backend) => par_backend += 1,
                Err(NonExprimable::Provenance) => par_provenance += 1,
            },
        }
    }
    let total = e.aretes.len();
    assert_eq!(
        exprimes + par_backend + par_provenance,
        total,
        "tout transition non rejouée doit avoir une cause nommée"
    );

    // Les deux causes d'inexprimable doivent être réelles : sinon la borne
    // annoncée dans l'entête serait fausse.
    assert!(
        par_backend > 0,
        "l'implémentation ne porterait donc aucune dimension backend ? vérifier la mesure"
    );
    assert!(
        par_provenance > 0,
        "`Tentative` serait donc transparente ? vérifier la mesure"
    );
    // Le socle rejouable ne doit pas s'être RÉTRÉCI en séparant K et B : ce sont
    // les mêmes transitions qu'en A-1 (celles qui ne nomment que le backend 0 et
    // ne présentent qu'une provenance réellement émise). La proportion, elle,
    // baisse mécaniquement puisque l'exploration a gagné la dimension B.
    assert!(
        exprimes > 1500,
        "le socle rejouable a fondu : {exprimes} transitions (A-1 en mesurait 1935)"
    );

    println!(
        "périmètre du rejeu : {exprimes}/{total} transitions rejouables \
         ({:.1} %) ; {par_backend} hors périmètre (backend absent de \
         l'implémentation) ; {par_provenance} hors périmètre (`Tentative` opaque)",
        100.0 * exprimes as f64 / total as f64
    );

    // Ce qui est hors périmètre l'est pour une raison NOMMÉE, et ces deux raisons
    // sont exactement les deux asymétries de l'entête. Ce qui est dans le
    // périmètre ne diverge pas (`le_reel_est_conforme_au_modele_de_reference`).
}

#[test]
fn rapport() {
    let e = reference();
    println!("\n=== Modèle causal A-2 — exploration de référence ===");
    let g_max = e
        .noeuds
        .iter()
        .flat_map(|n| n.monde.cycles.values().map(|c| c.g))
        .max()
        .unwrap_or(0);
    let prof_max = e.noeuds.iter().map(|n| n.profondeur).max().unwrap_or(0);
    println!(
        "profondeur ≤ {}, {} états atteints (profondeur réelle {}), {} transitions examinées, {} acceptées, rang max {}",
        e.profondeur_max,
        e.noeuds.len(),
        prof_max,
        e.aretes.len(),
        e.acceptees,
        g_max
    );
    for inv in INVARIANTS_SURETE
        .iter()
        .chain(INVARIANTS_PROGRESSION.iter())
    {
        match e.contre_exemple(*inv) {
            None => println!("  ok   {}", inv.nom()),
            Some(cx) => println!("  KO   {} ← {}", inv.nom(), cx.lisible()),
        }
    }

    let e2 = reference_deux();
    println!(
        "\ndeux opérations : {} états, {} transitions",
        e2.noeuds.len(),
        e2.aretes.len()
    );

    for (nom, v) in [
        (
            "e1e6e611 (a) abandon direct",
            variante(&Semantique::ABANDON_DIRECT),
        ),
        (
            "e1e6e611 (b) Indetermine non validé",
            variante(&Semantique::INDETERMINE_SANS_VALIDATION),
        ),
        (
            "dd6382be identité non vérifiée",
            variante(&Semantique::IDENTITE_NON_VERIFIEE),
        ),
        (
            "A-2 attestation sans backend",
            variante(&Semantique::ATTESTATION_SANS_BACKEND),
        ),
        (
            "A-2 identité réduite au rang",
            variante_deux(&Semantique::IDENTITE_REDUITE_AU_RANG),
        ),
    ] {
        println!("\n--- variante : {nom} ---");
        let mut rien = true;
        for inv in INVARIANTS_SURETE
            .iter()
            .chain(INVARIANTS_PROGRESSION.iter())
        {
            if let Some(cx) = v.contre_exemple(*inv) {
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
