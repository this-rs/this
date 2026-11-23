//! `ProviderTransport` de bout en bout contre un MOTEUR SIMULÉ local (une vraie socket, le
//! dialecte OpenAI-compatible) : ce que le raccordement réel opt-in prouve sur un vrai moteur,
//! ce fichier le prouve EN CI, sans clé ni réseau. Les deux se complètent : ici la forme du
//! dialogue et les familles de panne ; là-bas le fait qu'un vrai moteur parle ce dialecte.
//!
//! Le moteur simulé lit la requête, et choisit sa réponse d'après elle :
//! - `GET /v1/models` : un catalogue d'un modèle, `qwen-test` ;
//! - un modèle autre que `qwen-test` : `400` ;
//! - `SCENARIO:outil` sans résultat d'outil encore dans le fil : un appel `lire {"path":"a"}` ;
//! - sinon : « Bon » + « jour », en flux, avec le modèle NOMMÉ `qwen-test` dans chaque événement ;
//! - `SCENARIO:autre_nom` : la même réponse, mais le moteur nomme `qwen-autre` (routé ≠ servi).

#![cfg(all(feature = "providers", not(target_arch = "wasm32")))]

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::{Value, json};
use this_harness::providers::ProviderTransport;
use this_harness::{
    Cancel, Delta, Event, Failure, Harness, Limits, Outcome, Reply, Request, Running, Stop,
    ToolCall, ToolSpec, Toolbox, Transport,
};
use this_llm::{CredentialRef, Preset};
use this_llm_providers::{
    Clock as LlmClock, Hub, MemoryStore, ProviderClient, ProviderConfig, ProviderId, Secrets,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ── Le moteur simulé ────────────────────────────────────────────────────────

async fn moteur(listener: TcpListener) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let mut brut = Vec::new();
            let mut morceau = [0u8; 4096];
            loop {
                let lu = socket.read(&mut morceau).await.unwrap_or(0);
                if lu == 0 {
                    break;
                }
                brut.extend_from_slice(&morceau[..lu]);
                if let Some(fin) = trouve(&brut, b"\r\n\r\n") {
                    let tete = String::from_utf8_lossy(&brut[..fin]).to_ascii_lowercase();
                    let longueur = tete
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if brut.len() >= fin + 4 + longueur {
                        break;
                    }
                }
            }
            let requete = String::from_utf8_lossy(&brut).to_string();
            let (statut, type_contenu, corps) = route(&requete);
            let reponse = format!(
                "HTTP/1.1 {statut}\r\nContent-Type: {type_contenu}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corps}",
                corps.len()
            );
            let _ = socket.write_all(reponse.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
    }
}

fn trouve(meule: &[u8], aiguille: &[u8]) -> Option<usize> {
    meule
        .windows(aiguille.len())
        .position(|fenetre| fenetre == aiguille)
}

fn sse(evenements: &[Value]) -> String {
    let mut corps: String = evenements
        .iter()
        .map(|e| format!("data: {e}\n\n"))
        .collect();
    corps.push_str("data: [DONE]\n\n");
    corps
}

fn delta(nom: &str, texte: &str, fin: Option<&str>) -> Value {
    json!({
        "model": nom,
        "choices": [{"index": 0, "delta": {"content": texte}, "finish_reason": fin}]
    })
}

fn route(requete: &str) -> (&'static str, &'static str, String) {
    if requete.starts_with("GET /v1/models") {
        return (
            "200 OK",
            "application/json",
            json!({"data": [{"id": "qwen-test", "max_model_len": 4096}]}).to_string(),
        );
    }
    if !requete.starts_with("POST /v1/chat/completions") {
        return (
            "404 Not Found",
            "application/json",
            json!({"error": "?"}).to_string(),
        );
    }
    if !requete.contains("\"model\":\"qwen-test\"") {
        return (
            "400 Bad Request",
            "application/json",
            json!({"error": "modèle inconnu"}).to_string(),
        );
    }
    // Le modèle que le moteur NOMME : le demandé, sauf si le scénario le fait mentir.
    let nom = if requete.contains("SCENARIO:autre_nom") {
        "qwen-autre"
    } else {
        "qwen-test"
    };
    // Un appel d'outil, une seule fois : dès que le fil porte un résultat d'outil, on conclut.
    let outil = requete.contains("SCENARIO:outil") && !requete.contains("\"role\":\"tool\"");
    let evenements = if outil {
        vec![
            json!({"model": nom, "choices": [{"index": 0, "delta": {"tool_calls": [
                {"index": 0, "id": "call_1", "function": {"name": "lire", "arguments": "{\"pa"}}
            ]}, "finish_reason": null}]}),
            json!({"model": nom, "choices": [{"index": 0, "delta": {"tool_calls": [
                {"index": 0, "function": {"arguments": "th\":\"a\"}"}}
            ]}, "finish_reason": null}]}),
            json!({"model": nom, "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
        ]
    } else {
        vec![delta(nom, "Bon", None), delta(nom, "jour", Some("stop"))]
    };
    ("200 OK", "text/event-stream", sse(&evenements))
}

async fn demarre_moteur() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("port libre");
    let adresse = listener.local_addr().expect("adresse locale");
    tokio::spawn(moteur(listener));
    format!("http://{adresse}/v1")
}

// ── Le côté harnais ─────────────────────────────────────────────────────────

struct Jour;

impl LlmClock for Jour {
    fn day(&self) -> u64 {
        0
    }
}

fn client(base_url: &str, modele: &str) -> ProviderClient {
    let mut config = ProviderConfig::new(
        ProviderId::new("local").unwrap(),
        "Moteur simulé",
        Preset::OpenAiCompatible,
        CredentialRef::None,
    );
    config.base_url = base_url.to_string();
    config.model = Some(modele.to_string());
    let hub = Hub::open(MemoryStore::default(), Secrets::env_only(), Jour).unwrap();
    hub.upsert(config.clone()).unwrap();
    hub.set_active(Some(&config.id)).unwrap();
    hub.client(&config.id).expect("client")
}

/// Enveloppe `ProviderTransport` et note `Reply::served` de chaque tour.
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

fn temoin(client: ProviderClient) -> Rc<Temoin> {
    Rc::new(Temoin {
        inner: ProviderTransport::new(client),
        servis: RefCell::new(Vec::new()),
    })
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

/// Un outil `lire` qui note ses appels et rend `bonjour`.
struct Lire(Rc<RefCell<Vec<ToolCall>>>);

#[async_trait(?Send)]
impl Toolbox for Lire {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "lire".into(),
            description: "Lit un fichier".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }]
    }
    async fn call(&self, call: &ToolCall) -> Outcome {
        self.0.borrow_mut().push(call.clone());
        Outcome::ok("bonjour")
    }
}

type Journal = Rc<RefCell<Vec<Event>>>;

fn harness(transport: Rc<dyn Transport>, tools: Rc<dyn Toolbox>) -> (Harness, Journal) {
    let journal: Journal = Rc::default();
    let sink = journal.clone();
    let harness = Harness::new(
        "Tu es bref.",
        Limits::default(),
        transport,
        tools,
        move |e| sink.borrow_mut().push(e),
    );
    (harness, journal)
}

fn stopped(journal: &Journal) -> (Stop, String, Option<String>) {
    journal
        .borrow()
        .iter()
        .find_map(|e| match e {
            Event::Stopped {
                stop, text, code, ..
            } => Some((stop.clone(), text.clone(), code.clone())),
            _ => None,
        })
        .expect("un Stopped")
}

fn deltas(journal: &Journal) -> Vec<String> {
    journal
        .borrow()
        .iter()
        .filter_map(|e| match e {
            Event::Delta { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ── Les preuves ─────────────────────────────────────────────────────────────

/// Le flux du moteur arrive morceau par morceau au harnais, et le modèle que le moteur nomme
/// arrive dans `Reply::served`.
#[tokio::test]
async fn le_flux_du_moteur_arrive_au_harnais_avec_le_modele_nomme() {
    let base = demarre_moteur().await;
    let transport = temoin(client(&base, "qwen-test"));
    let (harness, journal) = harness(transport.clone(), Rc::new(SansOutil));

    let issue = harness.send("Dis bonjour.").await;
    let (stop, text, code) = stopped(&journal);
    assert_eq!(
        issue,
        Running::Ran(Stop::Done),
        "{stop:?} {code:?} {text:?}"
    );
    assert_eq!(
        deltas(&journal),
        vec!["Bon".to_string(), "jour".to_string()]
    );
    assert_eq!(text, "Bonjour");
    assert_eq!(
        transport.servis.borrow().as_slice(),
        &[Some("qwen-test".to_string())]
    );
}

/// Routé ≠ servi : quand le moteur nomme un AUTRE modèle que celui demandé, c'est cet autre
/// nom qui arrive au harnais — ni le demandé, ni rien. Le tour, lui, réussit : la règle
/// d'apprentissage (session.rs) décide ensuite de ne pas créditer la paire.
#[tokio::test]
async fn un_moteur_qui_nomme_un_autre_modele_le_dit_au_harnais() {
    let base = demarre_moteur().await;
    let transport = temoin(client(&base, "qwen-test"));
    let (harness, journal) = harness(transport.clone(), Rc::new(SansOutil));

    let issue = harness.send("SCENARIO:autre_nom").await;
    assert_eq!(issue, Running::Ran(Stop::Done), "{:?}", stopped(&journal));
    assert_eq!(
        transport.servis.borrow().as_slice(),
        &[Some("qwen-autre".to_string())]
    );
}

/// Un appel d'outil fragmenté dans le flux est recollé, relu en JSON, EXÉCUTÉ par le harnais,
/// et le résultat repart au moteur, qui conclut.
#[tokio::test]
async fn un_appel_d_outil_fragmente_est_recolle_et_execute_par_le_harnais() {
    let base = demarre_moteur().await;
    let appels = Rc::new(RefCell::new(Vec::new()));
    let transport = temoin(client(&base, "qwen-test"));
    let (harness, journal) = harness(transport.clone(), Rc::new(Lire(appels.clone())));

    let issue = harness.send("SCENARIO:outil").await;
    let (stop, text, code) = stopped(&journal);
    assert_eq!(
        issue,
        Running::Ran(Stop::Done),
        "{stop:?} {code:?} {text:?}"
    );

    let appels = appels.borrow();
    assert_eq!(appels.len(), 1, "{appels:?}");
    assert_eq!(appels[0].name, "lire");
    assert_eq!(appels[0].id, "call_1");
    assert_eq!(appels[0].arguments, json!({"path": "a"}));

    let (uses, results) = journal.borrow().iter().fold((0, 0), |(u, r), e| match e {
        Event::ToolUse { tool, .. } if tool == "lire" => (u + 1, r),
        Event::ToolResult { tool, text, .. } if tool == "lire" && text == "bonjour" => (u, r + 1),
        _ => (u, r),
    });
    assert_eq!((uses, results), (1, 1));
    assert_eq!(text, "Bonjour", "le moteur conclut après le résultat");
    assert_eq!(
        transport.servis.borrow().len(),
        2,
        "deux tours, deux réponses"
    );
}

/// Un statut d'erreur du moteur est une panne `http` : le moteur a répondu. Jamais `unreachable`.
#[tokio::test]
async fn un_statut_d_erreur_du_moteur_est_une_panne_http() {
    let base = demarre_moteur().await;
    let transport = temoin(client(&base, "modele-absent"));
    let (harness, journal) = harness(transport, Rc::new(SansOutil));

    let issue = harness.send("Dis bonjour.").await;
    let (stop, _, code) = stopped(&journal);
    assert!(matches!(issue, Running::Ran(Stop::Failed(_))), "{issue:?}");
    assert_eq!(code.as_deref(), Some("http"), "{stop:?}");
}

/// Un moteur qu'on ne joint pas est `unreachable` : rien n'a été servi, et la boucle le
/// distingue d'une réponse en erreur.
#[tokio::test]
async fn un_moteur_injoignable_est_une_panne_unreachable() {
    // Un port réservé puis libéré : personne n'écoute.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
        l.local_addr().expect("adresse").port()
    };
    let transport = temoin(client(&format!("http://127.0.0.1:{port}/v1"), "qwen-test"));
    let (harness, journal) = harness(transport.clone(), Rc::new(SansOutil));

    let issue = harness.send("Dis bonjour.").await;
    let (stop, _, code) = stopped(&journal);
    assert!(matches!(issue, Running::Ran(Stop::Failed(_))), "{issue:?}");
    assert_eq!(code.as_deref(), Some("unreachable"), "{stop:?}");
    assert!(transport.servis.borrow().is_empty(), "rien n'a été servi");
}
