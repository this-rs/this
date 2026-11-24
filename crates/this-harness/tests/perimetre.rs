//! Le périmètre d'une mémoire : une personne, un projet — et ce qui change les invalide.
//!
//! Carte (d) du ticket #290 : « Suppression/révocation d'accès et changement de projet
//! invalident les contenus concernés ; pas de mémoire croisée entre personnes. »
//!
//! Le harnais ne détient aucun contenu (ni note, ni décision, ni vecteur) : il ne peut
//! donc pas supprimer la source. Ce qu'il détient, c'est le **registre des identifiants
//! lus** — la seule pièce qui transforme une citation en preuve. C'est elle qu'un
//! changement de périmètre ou une révocation doit invalider, sinon la preuve survit à
//! l'accès qui l'avait produite.
//!
//! Ces tests passent par le VRAI chemin d'écriture (une boucle de run, un outil qui
//! déclare sa lecture par `Outcome::reading`) : un test qui remplirait le registre à la
//! main ne dirait rien du câblage.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use futures::executor::block_on;
use serde_json::json;

use this_agent::cite::{Provenance, SeenLedger};
use this_harness::{
    Anchors, Cancel, Delta, Event, Fact, Failure, Finish, Harness, Invalidation, Limits, Message,
    Outcome, Perimetre, Recall, Reply, Request, ToolCall, ToolSpec, Toolbox, Transport, anchors,
    rappel_avec,
};

const NOTE: &str = "#note:11111111-1111-1111-1111-111111111111";
const AUTRE: &str = "#note:22222222-2222-2222-2222-222222222222";

/// Un registre PARTAGÉ par deux harnais : c'est la situation dangereuse (un hôte qui
/// tient un outillage pour deux sessions, ou qui bascule de projet sans redémarrer).
type Registre = Rc<RefCell<SeenLedger>>;

/// Un outil qui déclare lire l'id qu'on lui donne — le contrat `Outcome::reading`.
struct Lit {
    id: String,
    registre: Registre,
}

#[async_trait(?Send)]
impl Toolbox for Lit {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "lire".into(),
            description: "lit une note".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    fn vus(&self) -> Option<Registre> {
        Some(self.registre.clone())
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok(format!("contenu de {}", self.id)).reading(format!("contenu de {}", self.id))
    }
}

/// Un moteur qui demande l'outil à CHAQUE tour : chaque `send` exécute donc exactement
/// une lecture, ce qui rend l'état du registre attribuable à un envoi précis.
struct ChaqueFois;

#[async_trait(?Send)]
impl Transport for ChaqueFois {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        Ok(Reply {
            // Ce faux moteur ne mesure rien : `usage` reste `None` (voir #288).
            usage: None,
            served: None,
            text: String::new(),
            calls: vec![ToolCall {
                id: "c1".into(),
                name: "lire".into(),
                arguments: json!({}),
            }],
            finish: Finish::Tools,
        })
    }
}

fn harnais(registre: &Registre, id: &str, sink: impl FnMut(Event) + 'static) -> Harness {
    Harness::new(
        "consigne",
        Limits {
            max_tours: 1,
            max_relances: 0,
            budget_ms: None,
            ..Limits::default()
        },
        Rc::new(ChaqueFois),
        Rc::new(Lit {
            id: id.to_string(),
            registre: registre.clone(),
        }),
        sink,
    )
}

fn registre_declare(personne: &str, projet: &str) -> Registre {
    Rc::new(RefCell::new(SeenLedger::pour(Perimetre::nouveau(
        personne, projet,
    ))))
}

/// Un `Recall` qui connaît nos deux notes : il rend un fait pour tout id demandé.
struct Memoire;

#[async_trait(?Send)]
impl Recall for Memoire {
    async fn recall(&self, ids: &[String]) -> Vec<Fact> {
        ids.iter()
            .map(|id| Fact {
                id: id.clone(),
                kind: "note".into(),
                fact: "corps de la note".into(),
                as_of: "2026-11-24".into(),
                source: "obrain".into(),
            })
            .collect()
    }
}

/// Le bloc d'une entrée du rappel : sa ligne, plus les lignes de continuation (la marque
/// de provenance est écrite sur une ligne à part, sous l'entrée).
fn entree(texte: &str, id: &str) -> String {
    let mut lignes = texte.lines();
    let debut = lignes
        .position(|l| l.starts_with("- ") && l.contains(id))
        .unwrap_or_else(|| panic!("{id} absent du rappel : {texte}"));
    let toutes: Vec<&str> = texte.lines().collect();
    let mut bloc = String::from(toutes[debut]);
    for ligne in &toutes[debut + 1..] {
        if ligne.starts_with("- ") || ligne.starts_with('[') || ligne.starts_with("Entités") {
            break;
        }
        bloc.push('\n');
        bloc.push_str(ligne);
    }
    bloc
}

fn ancres(id: &str) -> Anchors {
    anchors(&[Message::user(format!("regarde {id}"))])
}

/// Rends le message de rappel pour `id`, vu à travers `registre`.
fn rappel_de(id: &str, registre: &Registre) -> String {
    let registre = registre.borrow();
    block_on(rappel_avec(&ancres(id), &Memoire, Some(&registre)))
        .expect("un rappel pour un id cité")
        .text()
        .to_string()
}

#[test]
fn deux_personnes_ne_partagent_pas_les_lectures() {
    let registre = registre_declare("alice", "projet-a");
    let refus: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));
    let journal = refus.clone();

    // Alice lit la note par l'outil : c'est une preuve.
    let a = harnais(&registre, NOTE, move |e| journal.borrow_mut().push(e));
    let _ = a.perimetre(Perimetre::nouveau("alice", "projet-a"));
    block_on(a.send("lis la note"));
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Direct),
        "une lecture déclarée par l'outil doit être une preuve"
    );

    // Bob ouvre SA session sur le MÊME registre (même outillage partagé).
    let b = harnais(&registre, AUTRE, |_| {});
    let invalidation = b.perimetre(Perimetre::nouveau("bob", "projet-a"));
    assert!(
        matches!(invalidation, Invalidation::Vide { lectures: 1, .. }),
        "la déclaration doit dire ce qu'elle oublie : {invalidation:?}"
    );
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Never),
        "la lecture d'Alice ne survit pas à l'arrivée de Bob"
    );
    assert!(
        entree(&rappel_de(NOTE, &registre), NOTE).contains("CITÉ SEULEMENT"),
        "et elle ne se présente pas comme une preuve dans le rappel de Bob"
    );

    // Alice continue de travailler : son registre est celui de Bob, elle n'y écrit plus.
    let refus_avant = refus.borrow().len();
    block_on(a.send("relis la note"));
    block_on(a.send("relis encore la note"));
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Never),
        "une session ne peut pas écrire dans le registre d'une autre personne"
    );
    let dires: Vec<String> = refus.borrow()[refus_avant..]
        .iter()
        .filter_map(|e| match e {
            Event::Hint { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        dires.len(),
        1,
        "le refus se dit, et une seule fois (pas à chaque résultat d'outil) : {dires:?}"
    );
    assert!(
        dires[0].contains("périmètre"),
        "le refus doit nommer sa cause : {dires:?}"
    );
}

#[test]
fn changer_de_projet_invalide_les_lectures_et_le_rappel_le_dit() {
    let registre = registre_declare("alice", "projet-a");
    let h = harnais(&registre, NOTE, |_| {});
    h.perimetre(Perimetre::nouveau("alice", "projet-a"));
    block_on(h.send("lis la note"));
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Direct)
    );
    // Avant la bascule : le fait du graphe est rendu SANS la marque « cité seulement ».
    let avant = rappel_de(NOTE, &registre);
    assert!(
        !entree(&avant, NOTE).contains("CITÉ SEULEMENT"),
        "une lecture de ce projet est une preuve : {}",
        entree(&avant, NOTE)
    );

    let invalidation = h.perimetre(Perimetre::nouveau("alice", "projet-b"));
    assert_eq!(
        invalidation,
        Invalidation::Vide {
            lectures: 1,
            citations: 0
        }
    );
    assert_eq!(
        h.perimetre_actuel(),
        Some(Perimetre::nouveau("alice", "projet-b")),
        "le harnais porte le périmètre courant"
    );
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Never),
        "changer de projet invalide ce que l'ancien avait lu"
    );
    assert!(
        entree(&rappel_de(NOTE, &registre), NOTE).contains("CITÉ SEULEMENT"),
        "le rappel ne présente plus la lecture de l'ancien projet comme une preuve"
    );

    // Se redéclarer dans le même périmètre n'oublie rien : une session ne se vide pas
    // à chaque tour.
    block_on(h.send("lis la note"));
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Direct)
    );
    assert_eq!(
        h.perimetre(Perimetre::nouveau("alice", "projet-b")),
        Invalidation::Aucune
    );
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Direct),
        "re-déclarer le même périmètre ne détruit pas la preuve"
    );
}

#[test]
fn la_reprise_rebatit_le_registre_dans_le_perimetre_de_la_session() {
    let registre = registre_declare("alice", "projet-a");
    let h = harnais(&registre, NOTE, |_| {});
    h.perimetre(Perimetre::nouveau("alice", "projet-a"));
    block_on(h.send("lis la note"));
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Direct)
    );

    // Reprise conservatrice : ce que le fil ne prouve pas ne devient pas une preuve.
    let snapshot = h.snapshot();
    h.restore(snapshot);
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Quoted),
        "après une reprise par défaut, une lecture du fil redevient « cité seulement »"
    );
    assert_eq!(
        registre.borrow().perimetre(),
        Some(&Perimetre::nouveau("alice", "projet-a")),
        "le registre rebâti doit rester DANS le périmètre de la session : sinon la \
         session muette suivante écrirait dans un registre non déclaré (donc sans garde)"
    );

    // Et la session continue d'y écrire normalement.
    block_on(h.send("relis la note"));
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Direct),
        "le registre rebâti accepte les lectures de SA session"
    );
}

#[test]
fn une_revocation_d_acces_retire_la_preuve() {
    let registre = registre_declare("alice", "projet-a");
    let h = harnais(&registre, NOTE, |_| {});
    h.perimetre(Perimetre::nouveau("alice", "projet-a"));
    block_on(h.send("lis la note"));
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Direct)
    );

    // L'hôte apprend que l'accès est révoqué : il le dit au registre (c'est lui qui
    // détient le magasin ; le harnais ne sait rien des droits).
    let oublies = registre.borrow_mut().revoquer(&[NOTE.to_string()]);
    assert_eq!(oublies, 1);
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Never),
        "un id révoqué redevient « jamais vu »"
    );
    assert!(
        entree(&rappel_de(NOTE, &registre), NOTE).contains("CITÉ SEULEMENT"),
        "et le rappel le dit au lieu de le présenter comme une preuve"
    );
}

#[test]
fn une_session_sans_perimetre_declare_garde_le_comportement_d_avant() {
    // Non-régression : un hôte qui ne déclare rien (aucun `Harness::perimetre`, registre
    // sans périmètre) n'est pas puni — il ne gagne simplement aucune garantie.
    let registre: Registre = Rc::new(RefCell::new(SeenLedger::new()));
    let h = harnais(&registre, NOTE, |_| {});
    block_on(h.send("lis la note"));
    assert_eq!(
        registre.borrow().provenance_token(NOTE),
        Some(Provenance::Direct)
    );
    assert_eq!(registre.borrow().perimetre(), None);
    // Et un registre déclaré refuse une session muette : elle ne peut pas prouver
    // qu'elle est la même personne.
    let muet_apres: Registre = registre_declare("alice", "projet-a");
    let h2 = harnais(&muet_apres, NOTE, |_| {});
    block_on(h2.send("lis la note"));
    assert_eq!(
        muet_apres.borrow().provenance_token(NOTE),
        Some(Provenance::Never),
        "devant un registre déclaré, une session qui ne déclare rien n'écrit rien"
    );
}
