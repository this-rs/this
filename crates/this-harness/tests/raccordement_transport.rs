//! Raccordement RÉEL du harnais à un moteur, par `ProviderTransport` — opt-in (#288, C5).
//!
//! Ce que les doublures ne prouvent pas, et que ce fichier prouve : le CHEMIN COMPLET
//! `Harness → ProviderTransport → ProviderClient → moteur réel → Reply`, avec le modèle que
//! le moteur NOMME (`Reply::served`), un appel d'outil exécuté par le harnais, et une clé
//! refusée classée comme réponse du moteur (`http`), jamais comme panne réseau.
//!
//! Ces tests sont `#[ignore]` : une suite ordinaire (la CI) les compte comme IGNORÉS, jamais
//! comme réussis sans avoir rien fait. Ils ne tournent qu'avec `--ignored` ET
//! `THIS_RACCORDEMENT=deepseek`. La clé n'est JAMAIS dans le dépôt : elle est lue dans
//! `DEEPSEEK_API_KEY` par `CredentialRef::Env` (le chemin de production), injectée par un coffre :
//!
//! ```text
//! THIS_RACCORDEMENT=deepseek \
//! THIS_RACCORDEMENT_CLE_FAUSSE=sk-0000000000000000000000000000000 \
//!   orchestrator secret exec -e DEEPSEEK_API_KEY=deepseek-api-key -- \
//!   cargo test -p this-harness --features providers --test raccordement_transport -- \
//!     --ignored --test-threads=1 --nocapture
//! ```
//!
//! Chaque test imprime une ligne `RACCORDEMENT …` pour le rapport daté ; aucune ne contient de
//! secret. Le modèle servi est observé par un transport TÉMOIN qui enveloppe `ProviderTransport`
//! et note `Reply::served` : le harnais ne l'émet pas en événement (il s'en sert pour le routeur).

#![cfg(all(feature = "providers", not(target_arch = "wasm32")))]

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;
use this_harness::providers::ProviderTransport;
use this_harness::{
    Cancel, Delta, Event, Failure, Harness, Limits, Outcome, Reply, Request, Running, Stop,
    ToolCall, ToolSpec, Toolbox, Transport,
};
use this_llm::{CredentialRef, Preset};
use this_llm_providers::{
    Clock as LlmClock, Hub, MemoryStore, ProviderClient, ProviderConfig, ProviderId, Secrets,
};

const VARIABLE_DE_CLE: &str = "DEEPSEEK_API_KEY";
/// Le modèle demandé explicitement : le moteur doit le NOMMER dans sa réponse.
const MODELE: &str = "deepseek-flash";
/// Le dialecte éprouvé ici — le même que déclare le moteur simulé (`transport_moteur_simule.rs`,
/// `DIALECTE`). Une exécution réussie de ce fichier met à jour `VALIDE_CONTRE` là-bas.
const DIALECTE: &str = "openai/chat-completions+sse";

/// Le SHA de `this` au moment de la mesure, pour le rapport daté ; `inconnu` hors d'un dépôt.
fn sha_courant() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|sortie| sortie.status.success())
        .map(|sortie| String::from_utf8_lossy(&sortie.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "inconnu".to_string())
}

struct Jour;

impl LlmClock for Jour {
    fn day(&self) -> u64 {
        0
    }
}

/// Vrai si le raccordement est demandé. Sinon, le test le dit et passe.
fn demande(nom: &str) -> bool {
    let actif = std::env::var("THIS_RACCORDEMENT").as_deref() == Ok("deepseek");
    if !actif {
        eprintln!("{nom} : sauté (THIS_RACCORDEMENT=deepseek non posé)");
    }
    actif
}

fn client(variable: &str) -> ProviderClient {
    let mut config = ProviderConfig::new(
        ProviderId::new("deepseek").unwrap(),
        "DeepSeek",
        Preset::DeepSeek,
        CredentialRef::Env(variable.to_string()),
    );
    config.model = Some(MODELE.to_string());
    let hub = Hub::open(MemoryStore::default(), Secrets::env_only(), Jour).unwrap();
    hub.upsert(config.clone()).unwrap();
    hub.set_active(Some(&config.id)).unwrap();
    hub.client(&config.id).expect("client")
}

/// Enveloppe `ProviderTransport` et note ce que chaque tour a rendu : c'est la seule façon,
/// depuis un test, de lire `Reply::served` sans toucher au harnais.
struct Temoin {
    inner: ProviderTransport,
    servis: RefCell<Vec<Option<String>>>,
}

#[async_trait(?Send)]
impl Transport for Temoin {
    async fn stream(
        &self,
        request: &Request,
        cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        let issue = self.inner.stream(request, cancel, delta).await;
        if let Ok(reply) = &issue {
            self.servis.borrow_mut().push(reply.served.clone());
        }
        issue
    }

    fn honors_route(&self) -> bool {
        self.inner.honors_route()
    }
}

struct SansOutil;

#[async_trait(?Send)]
impl Toolbox for SansOutil {
    fn specs(&self) -> Vec<ToolSpec> {
        Vec::new()
    }
    async fn call(&self, call: &ToolCall) -> Outcome {
        Outcome::refused("refusé", format!("aucun outil : {}", call.name))
    }
}

/// Un seul outil, `ping`, qui répond `pong`.
struct Ping(Rc<RefCell<Vec<ToolCall>>>);

#[async_trait(?Send)]
impl Toolbox for Ping {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "ping".into(),
            description: "Envoie un ping à une cible et rend sa réponse.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "cible": { "type": "string" } },
                "required": ["cible"]
            }),
        }]
    }
    async fn call(&self, call: &ToolCall) -> Outcome {
        self.0.borrow_mut().push(call.clone());
        Outcome::ok("pong")
    }
}

type Journal = Rc<RefCell<Vec<Event>>>;

fn harness(
    system: &str,
    transport: Rc<dyn Transport>,
    tools: Rc<dyn Toolbox>,
) -> (Harness, Journal) {
    let journal: Journal = Rc::default();
    let sink = journal.clone();
    let harness = Harness::new(system, Limits::default(), transport, tools, move |event| {
        sink.borrow_mut().push(event)
    });
    (harness, journal)
}

fn temoin(client: ProviderClient) -> Rc<Temoin> {
    Rc::new(Temoin {
        inner: ProviderTransport::new(client),
        servis: RefCell::new(Vec::new()),
    })
}

fn stopped(journal: &Journal) -> (Stop, String, Option<String>) {
    journal
        .borrow()
        .iter()
        .find_map(|event| match event {
            Event::Stopped {
                stop, text, code, ..
            } => Some((stop.clone(), text.clone(), code.clone())),
            _ => None,
        })
        .expect("un Stopped")
}

#[tokio::test]
#[ignore = "raccordement réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn le_harnais_recoit_le_texte_du_moteur_reel_et_le_modele_qu_il_nomme() {
    if !demande("texte+servi") {
        return;
    }
    let transport = temoin(client(VARIABLE_DE_CLE));
    let (harness, journal) = harness(
        "Tu es bref. Réponds exactement « pong », sans autre mot.",
        transport.clone(),
        Rc::new(SansOutil),
    );

    let issue = harness.send("ping").await;
    let (stop, text, code) = stopped(&journal);
    assert_eq!(
        issue,
        Running::Ran(Stop::Done),
        "{stop:?} {code:?} {text:?}"
    );

    // Le texte est arrivé par le FLUX (des `Delta`), pas seulement dans le bilan.
    let deltas: String = journal
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Delta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(!deltas.trim().is_empty(), "aucun Delta reçu");
    assert!(text.to_lowercase().contains("pong"), "{text:?}");

    // Routé ≠ servi : le moteur NOMME le modèle demandé, et c'est ce nom qui arrive au harnais.
    let servis = transport.servis.borrow().clone();
    assert_eq!(servis.len(), 1, "{servis:?}");
    assert_eq!(
        servis[0].as_deref(),
        Some(MODELE),
        "le moteur nomme un autre modèle"
    );
    let sha = sha_courant();
    eprintln!(
        "RACCORDEMENT transport texte+servi [{DIALECTE} · this {sha}] : stop=Done, servi={:?}, texte={text:?}",
        servis[0]
    );
}

#[tokio::test]
#[ignore = "raccordement réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn le_harnais_execute_l_outil_demande_par_le_moteur_reel() {
    if !demande("outil") {
        return;
    }
    let appels = Rc::new(RefCell::new(Vec::new()));
    let transport = temoin(client(VARIABLE_DE_CLE));
    let (harness, journal) = harness(
        "Tu disposes d'un outil `ping`. Pour répondre, appelle `ping` avec la cible demandée, \
         puis rapporte sa réponse en un mot.",
        transport.clone(),
        Rc::new(Ping(appels.clone())),
    );

    let issue = harness.send("Fais un ping sur la cible « atelier ».").await;
    let (stop, text, code) = stopped(&journal);
    assert_eq!(
        issue,
        Running::Ran(Stop::Done),
        "{stop:?} {code:?} {text:?}"
    );

    // L'appel est passé par le harnais : il a été EXÉCUTÉ (pas seulement annoncé), avec des
    // arguments relus en JSON.
    let appels = appels.borrow();
    assert_eq!(appels.len(), 1, "{appels:?}");
    assert_eq!(appels[0].name, "ping");
    assert_eq!(
        appels[0].arguments["cible"], "atelier",
        "{:?}",
        appels[0].arguments
    );

    let (uses, results) = journal
        .borrow()
        .iter()
        .fold((0, 0), |(u, r), event| match event {
            Event::ToolUse { tool, .. } if tool == "ping" => (u + 1, r),
            Event::ToolResult { tool, .. } if tool == "ping" => (u, r + 1),
            _ => (u, r),
        });
    assert_eq!((uses, results), (1, 1), "ToolUse/ToolResult");

    // Deux tours (l'appel, puis la réponse), et le moteur a nommé le modèle aux deux.
    let servis = transport.servis.borrow().clone();
    assert!(servis.len() >= 2, "{servis:?}");
    assert!(
        servis.iter().all(|s| s.as_deref() == Some(MODELE)),
        "{servis:?}"
    );
    let sha = sha_courant();
    eprintln!(
        "RACCORDEMENT transport outil [{DIALECTE} · this {sha}] : stop=Done, tours={}, arguments={}, texte={text:?}",
        servis.len(),
        appels[0].arguments
    );
}

#[tokio::test]
#[ignore = "raccordement réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn une_cle_refusee_arrete_le_run_en_panne_http_pas_reseau() {
    if !demande("clé refusée") {
        return;
    }
    // La fausse clé est posée dans l'environnement AVANT le lancement, jamais par le test (un
    // `set_var` en cours de processus serait une course avec les autres threads).
    assert!(
        std::env::var("THIS_RACCORDEMENT_CLE_FAUSSE").is_ok(),
        "poser THIS_RACCORDEMENT_CLE_FAUSSE=<une valeur sans valeur> avant de lancer"
    );
    let transport = temoin(client("THIS_RACCORDEMENT_CLE_FAUSSE"));
    let (harness, journal) = harness("Tu es bref.", transport, Rc::new(SansOutil));

    let issue = harness.send("ping").await;
    let (stop, _, code) = stopped(&journal);
    assert!(matches!(issue, Running::Ran(Stop::Failed(_))), "{issue:?}");
    assert_eq!(code.as_deref(), Some("http"), "{stop:?}");
    let sha = sha_courant();
    eprintln!(
        "RACCORDEMENT transport clé refusée [{DIALECTE} · this {sha}] : stop={stop:?}, code={code:?}"
    );
}

/// Enveloppe `ProviderTransport` : après le PREMIER morceau reçu, demande l'interruption du tour.
struct Coupe {
    inner: ProviderTransport,
    handle: Rc<RefCell<Option<this_harness::Handle>>>,
    coupe: Rc<std::cell::Cell<bool>>,
}

#[async_trait(?Send)]
impl Transport for Coupe {
    async fn stream(
        &self,
        request: &Request,
        cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        let handle = self.handle.clone();
        let coupe = self.coupe.clone();
        // Couper seulement après VINGT caractères de texte réel : un morceau vide ou un signe de
        // vie ne compte pas, sinon la coupure tomberait avant tout texte visible.
        let vus = Rc::new(std::cell::Cell::new(0usize));
        let relais: Delta = Rc::new(move |morceau: &str| {
            delta(morceau);
            vus.set(vus.get() + morceau.chars().count());
            if !coupe.get() && vus.get() >= 20 {
                coupe.set(true);
                if let Some(h) = handle.borrow().as_ref() {
                    h.interrupt();
                }
            }
        });
        self.inner.stream(request, cancel, relais).await
    }

    fn honors_route(&self) -> bool {
        true
    }
}

/// Interruption puis reprise sur le moteur RÉEL : un tour long est coupé après le premier morceau,
/// puis le message suivant passe par le même harnais et le même transport.
#[tokio::test]
#[ignore = "raccordement réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn le_moteur_reel_coupe_en_plein_flux_et_le_message_suivant_aboutit() {
    if !demande("interruption+reprise") {
        return;
    }
    let handle = Rc::new(RefCell::new(None));
    let transport = Rc::new(Coupe {
        inner: ProviderTransport::new(client(VARIABLE_DE_CLE)),
        handle: handle.clone(),
        coupe: Rc::new(std::cell::Cell::new(false)),
    });
    let (harness, journal) = harness("Tu es précis.", transport, Rc::new(SansOutil));
    *handle.borrow_mut() = Some(harness.handle());

    // Un tour LONG : sans coupure il produirait plusieurs centaines de jetons.
    let debut = std::time::Instant::now();
    let coupe = harness
        .send("Compte de 1 à 300, un nombre par ligne, sans aucun autre texte.")
        .await;
    let duree = debut.elapsed();
    let (stop, texte, code) = stopped(&journal);
    let recus = journal
        .borrow()
        .iter()
        .filter(|e| matches!(e, Event::Delta { text } if !text.is_empty()))
        .count();
    assert!(
        recus >= 1,
        "aucun texte n'a été reçu avant la coupure : la coupure n'est pas en plein flux"
    );
    assert!(
        matches!(coupe, Running::Ran(Stop::Interrupted | Stop::Cancelled)),
        "{coupe:?} {stop:?} {code:?} {texte:?}"
    );
    let sha = sha_courant();
    eprintln!(
        "RACCORDEMENT transport interruption [{DIALECTE} · this {sha}] : arrêt {stop:?} en {duree:?}, {recus} morceau(x) de texte reçu(s) avant la coupure"
    );

    // La reprise : le message SUIVANT passe par le même harnais, le même transport, le même moteur.
    journal.borrow_mut().clear();
    let suite = harness
        .send("Réponds exactement « pong », sans autre mot.")
        .await;
    let (stop, texte, code) = stopped(&journal);
    assert_eq!(
        suite,
        Running::Ran(Stop::Done),
        "{stop:?} {code:?} {texte:?}"
    );
    assert!(texte.to_lowercase().contains("pong"), "{texte:?}");
    eprintln!(
        "RACCORDEMENT transport reprise [{DIALECTE} · this {sha}] : stop=Done, texte={texte:?}"
    );
}
