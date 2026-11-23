//! Le catalogue d'outils d'auteur face à un moteur RÉEL — opt-in.
//!
//! Ce que les tests scriptés (`tests/catalogue.rs`) ne peuvent pas dire : un vrai modèle
//! cherche-t-il avant de créer, et réutilise-t-il un outil au lieu d'en recréer un ? Ils
//! prouvent la mécanique ; celui-ci MESURE le comportement.
//!
//! Il ne fait donc pas semblant d'asserter ce qui est stochastique. Les invariants de
//! SÛRETÉ sont assertés (aucune étape hors de l'hôte, aucun doublon de recette au catalogue,
//! le catalogue relu à l'identique) ; le COMPORTEMENT (chercher avant de créer, réutiliser)
//! est imprimé en lignes `RACCORDEMENT catalogue …` pour le rapport daté, sans secret.
//!
//! `#[ignore]` : une suite ordinaire le compte comme IGNORÉ. Il ne tourne qu'avec
//! `--ignored` ET `THIS_RACCORDEMENT=deepseek`, la clé étant injectée par le coffre :
//!
//! ```text
//! THIS_RACCORDEMENT=deepseek \
//!   orchestrator secret exec -e DEEPSEEK_API_KEY=deepseek-api-key -- \
//!   cargo test -p this-harness --features providers --test catalogue_reel -- \
//!     --ignored --test-threads=1 --nocapture
//! ```

#![cfg(all(feature = "providers", not(target_arch = "wasm32")))]

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::{Value, json};
use this_agent::evidence::Declaration;
use this_agent::untrusted::PREAMBLE;
use this_harness::providers::ProviderTransport;
use this_harness::{
    AUTHOR, Catalog, CatalogStore, CatalogToolbox, Event, FIND, Harness, Limits, Outcome, PREFIX,
    ToolCall, ToolSpec, Toolbox,
};
use this_llm::{CredentialRef, Preset};
use this_llm_providers::{
    Clock as LlmClock, Hub, MemoryStore, ProviderClient, ProviderConfig, ProviderId, Secrets,
};

const VARIABLE_DE_CLE: &str = "DEEPSEEK_API_KEY";
const MODELE: &str = "deepseek-flash";

struct Jour;
impl LlmClock for Jour {
    fn day(&self) -> u64 {
        0
    }
}

fn demande(nom: &str) -> bool {
    let actif = std::env::var("THIS_RACCORDEMENT").as_deref() == Ok("deepseek");
    if !actif {
        eprintln!("{nom} : sauté (THIS_RACCORDEMENT=deepseek non posé)");
    }
    actif
}

fn client() -> ProviderClient {
    let mut config = ProviderConfig::new(
        ProviderId::new("deepseek").unwrap(),
        "DeepSeek",
        Preset::DeepSeek,
        CredentialRef::Env(VARIABLE_DE_CLE.to_string()),
    );
    config.model = Some(MODELE.to_string());
    let hub = Hub::open(MemoryStore::default(), Secrets::env_only(), Jour).unwrap();
    hub.upsert(config.clone()).unwrap();
    hub.set_active(Some(&config.id)).unwrap();
    hub.client(&config.id).expect("client")
}

/// Les outils de l'hôte : une lecture et une écriture, et le journal de ce qu'ils ont fait.
#[derive(Default)]
struct Hote {
    appels: RefCell<Vec<(String, Value)>>,
}

#[async_trait(?Send)]
impl Toolbox for Hote {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "task_list".into(),
                description: "Liste les tâches d'un plan.".into(),
                parameters: json!({"type": "object",
                    "properties": {"plan": {"type": "string"}}, "required": ["plan"]}),
            },
            ToolSpec {
                name: "note_add".into(),
                description: "Ajoute une note.".into(),
                parameters: json!({"type": "object",
                    "properties": {"text": {"type": "string"}}, "required": ["text"]}),
            },
        ]
    }
    fn declarations(&self) -> Vec<Declaration> {
        vec![
            Declaration::reads("task_list"),
            Declaration::writes("note_add"),
        ]
    }
    async fn call(&self, call: &ToolCall) -> Outcome {
        self.appels
            .borrow_mut()
            .push((call.name.clone(), call.arguments.clone()));
        match call.name.as_str() {
            "task_list" => Outcome::ok(format!(
                "tâches de {} : t1, t2",
                call.arguments
                    .get("plan")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
            )),
            _ => Outcome::ok("note ajoutée"),
        }
    }
}

#[derive(Default)]
struct Range(RefCell<Option<String>>);
impl CatalogStore for Range {
    fn save(&self, json: &str) -> Result<(), String> {
        *self.0.borrow_mut() = Some(json.to_string());
        Ok(())
    }
}

struct Session {
    outils: Vec<(String, bool)>,
    arret: String,
}

fn consigne() -> String {
    format!(
        "Tu disposes d'outils. Règles pour les outils que tu construis toi-même :\n\
         1. Avant d'en créer un, cherche avec {FIND} (action=search). \n\
         2. Si un outil convient, appelle-le par son nom ({PREFIX}…) au tour suivant la \
            recherche, au lieu d'en créer un autre.\n\
         3. Un outil est une suite d'au plus 8 appels à tes outils existants, avec des arguments \
            « {{{{param}}}} » ; crée-le avec {AUTHOR} (action=create, description, parameters, steps).\n\
         Réponds brièvement.\n\n{PREAMBLE}"
    )
}

async fn jouer(tb: Rc<CatalogToolbox>, demande: &str) -> Session {
    let journal: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink = journal.clone();
    let harness = Harness::new(
        consigne(),
        Limits::default(),
        Rc::new(ProviderTransport::new(client())),
        tb,
        move |event| sink.borrow_mut().push(event),
    );
    harness.send(demande).await;
    let events = journal.borrow();
    let mut refus: Vec<(String, bool)> = Vec::new();
    for e in events.iter() {
        if let Event::ToolResult { tool, refused, .. } = e {
            refus.push((tool.clone(), *refused));
        }
    }
    let arret = events
        .iter()
        .find_map(|e| match e {
            Event::Stopped { stop, .. } => Some(format!("{stop:?}")),
            _ => None,
        })
        .unwrap_or_else(|| "aucun Stopped".into());
    Session {
        outils: refus,
        arret,
    }
}

fn decrire(label: &str, s: &Session) {
    let suite: Vec<String> = s
        .outils
        .iter()
        .map(|(t, refused)| format!("{t}{}", if *refused { "(refusé)" } else { "" }))
        .collect();
    println!(
        "RACCORDEMENT catalogue {label} : {} | arrêt {}",
        suite.join(" → "),
        s.arret
    );
}

fn invariants(hote: &Hote, catalogue: &Catalog) {
    // Aucune étape hors de l'hôte : l'hôte n'a vu que ses deux outils.
    let permis: HashSet<&str> = ["task_list", "note_add"].into();
    for (nom, _) in hote.appels.borrow().iter() {
        assert!(permis.contains(nom.as_str()), "appel hors hôte : {nom}");
    }
    // Aucune paire d'outils ACTIFS de même recette.
    let recettes: Vec<String> = catalogue
        .active()
        .map(|r| serde_json::to_string(&r.current().steps).unwrap())
        .collect();
    let uniques: HashSet<&String> = recettes.iter().collect();
    assert_eq!(
        recettes.len(),
        uniques.len(),
        "doublon de recette : {recettes:?}"
    );
}

#[tokio::test]
#[ignore = "moteur réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn un_vrai_modele_cherche_avant_de_creer_et_reutilise_au_lieu_de_recreer() {
    if !demande("catalogue réel") {
        return;
    }
    let hote = Rc::new(Hote::default());
    let range = Rc::new(Range::default());

    // Session 1 : catalogue vide. On voit ce que le modèle construit.
    let tb = Rc::new(CatalogToolbox::new(
        hote.clone(),
        Catalog::new(),
        Some(range.clone()),
    ));
    let s1 = jouer(
        tb,
        "Consulte le plan alpha avec task_list, puis note avec note_add que tu l'as consulté. \
         Tu devras refaire exactement cela pour d'autres plans : construis-toi un outil pour ça \
         (paramètre « plan »), puis utilise-le sur alpha.",
    )
    .await;
    decrire("session 1 (catalogue vide)", &s1);
    let apres_1 = Catalog::from_json(range.0.borrow().as_deref().unwrap_or("{\"format\":1}"))
        .expect("le catalogue rangé se relit");
    let crees_1 = apres_1.active().count();
    println!("RACCORDEMENT catalogue session 1 : {crees_1} outil(s) au catalogue");
    invariants(&hote, &apres_1);

    // Session 2 : un AUTRE harnais, le même catalogue rangé, rien de chargé. La demande est
    // la même sur un autre plan : un modèle qui se souvient de chercher réutilise.
    let tb2 = Rc::new(CatalogToolbox::new(
        hote.clone(),
        apres_1.clone(),
        Some(range.clone()),
    ));
    let s2 = jouer(
        tb2,
        "Même chose pour le plan beta : consulte-le avec task_list puis note que tu l'as consulté.",
    )
    .await;
    decrire("session 2 (catalogue relu)", &s2);
    let apres_2 = Catalog::from_json(range.0.borrow().as_deref().unwrap()).unwrap();
    let crees_2 = apres_2.active().count().saturating_sub(crees_1);
    let a_cherche = s2.outils.iter().any(|(t, _)| t == FIND);
    let a_reutilise = s2.outils.iter().any(|(t, r)| t.starts_with(PREFIX) && !*r);
    println!(
        "RACCORDEMENT catalogue session 2 : a cherché={a_cherche} a réutilisé={a_reutilise} \
         outils créés en plus={crees_2}"
    );
    invariants(&hote, &apres_2);
}
