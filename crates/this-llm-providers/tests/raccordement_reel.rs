//! Raccordement RÉEL à un moteur, opt-in (this#288, case C5 de
//! `docs/harnais-capacites-moteurs.md`).
//!
//! Ces tests sont `#[ignore]` : une suite ordinaire (la CI) les compte comme IGNORÉS, jamais
//! comme réussis sans avoir rien fait. Ils ne tournent qu'avec `--ignored` ET
//! `THIS_RACCORDEMENT=deepseek` ; lancés par `--ignored` sans la variable, chacun dit qu'il
//! est sauté. La clé n'est JAMAIS dans le dépôt ni dans une fixture : elle
//! est lue dans la variable `DEEPSEEK_API_KEY` par la référence `CredentialRef::Env` — le
//! chemin de production —, typiquement injectée par un coffre :
//!
//! ```text
//! THIS_RACCORDEMENT=deepseek \
//! THIS_RACCORDEMENT_CLE_FAUSSE=sk-0000000000000000000000000000000 \
//!   orchestrator secret exec -e DEEPSEEK_API_KEY=deepseek-api-key -- \
//!   cargo test -p this-llm-providers --test raccordement_reel -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Ce qu'ils prouvent, et que les doublures ne prouvent pas : le moteur réel rend du texte,
//! un USAGE réel (des jetons comptés par le moteur, pas estimés), un appel d'outil
//! structuré, et une clé refusée est classée comme une réponse du moteur (`Http(401)`),
//! jamais comme une panne réseau. Le modèle servi se lit dans le champ `model` de la RÉPONSE
//! BRUTE du moteur (test dédié). Le client `this-llm-providers` recopie, lui, le modèle DEMANDÉ
//! et n'expose pas encore le modèle servi : c'est le manque C5 (`docs/harnais-capacites-moteurs.md`).
//! Chaque test imprime une ligne `RACCORDEMENT …` pour le rapport daté ; aucune ne contient de secret.
//!
//! La vraie clé vient du coffre (`DEEPSEEK_API_KEY`, jamais écrite). La clé FAUSSE
//! (`THIS_RACCORDEMENT_CLE_FAUSSE`, une fixture sans valeur) est posée dans l'environnement AVANT
//! le lancement, jamais par le test : un `set_var` en cours de processus serait une course avec les
//! autres threads. La commande ci-dessus pose les deux ; sans la fausse, le test de clé refusée
//! échoue en le disant.

#![cfg(feature = "http")]

use serde_json::json;
use this_llm::{CredentialRef, Preset};
use this_llm_providers::{
    ChatRequest, Clock, ErrorKind, Finish, Hub, MemoryStore, ProviderConfig, ProviderError,
    ProviderId, Secrets, TurnRequest,
};

const VARIABLE_DE_CLE: &str = "DEEPSEEK_API_KEY";

struct Jour;

impl Clock for Jour {
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

fn deepseek(variable: &str) -> ProviderConfig {
    ProviderConfig::new(
        ProviderId::new("deepseek").unwrap(),
        "DeepSeek",
        Preset::DeepSeek,
        CredentialRef::Env(variable.to_string()),
    )
}

fn client(config: ProviderConfig) -> this_llm_providers::ProviderClient {
    let hub = Hub::open(MemoryStore::default(), Secrets::env_only(), Jour).unwrap();
    hub.upsert(config.clone()).unwrap();
    hub.set_active(Some(&config.id)).unwrap();
    hub.client(&config.id).expect("client")
}

#[tokio::test]
#[ignore = "raccordement réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn le_moteur_reel_rend_du_texte_et_un_usage_compte_par_lui() {
    if !demande("texte+usage") {
        return;
    }
    assert!(
        std::env::var(VARIABLE_DE_CLE).is_ok_and(|v| !v.is_empty()),
        "THIS_RACCORDEMENT est posé mais {VARIABLE_DE_CLE} est vide : injecte-la par le coffre"
    );
    let mut requete = ChatRequest::new("Tu réponds en un mot.", "Réponds exactement : pong");
    // Un plafond large : un modèle qui raisonne dépense ses jetons de sortie AVANT le texte
    // (voir le test suivant).
    requete.max_output_tokens = Some(1024);
    let reponse = client(deepseek(VARIABLE_DE_CLE))
        .chat(requete)
        .await
        .expect("le moteur répond");
    // `reponse.model` est le modèle DEMANDÉ recopié par le client : ce n'est pas une preuve du
    // modèle servi (voir le test `le_modele_nomme_par_la_reponse_du_moteur_est_egal_au_modele_demande`).
    println!(
        "RACCORDEMENT texte+usage : modèle demandé (recopié)={} entrée={} sortie={} tronqué={} texte={:?}",
        reponse.model,
        reponse.usage.input_tokens,
        reponse.usage.output_tokens,
        reponse.truncated,
        reponse.text
    );
    assert!(!reponse.text.trim().is_empty(), "du texte");
    assert!(
        !reponse.model.is_empty(),
        "le client nomme le modèle demandé"
    );
    assert!(
        reponse.usage.input_tokens > 0 && reponse.usage.output_tokens > 0,
        "un usage réel, compté par le moteur : {:?}",
        reponse.usage
    );
}

#[tokio::test]
#[ignore = "raccordement réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn un_plafond_epuise_par_le_raisonnement_se_dit_tronque_pas_fini() {
    // MESURÉ le 21/11/2026 : `deepseek-flash` sous un plafond de 16 jetons rend un texte
    // VIDE — les jetons de sortie partent dans le raisonnement. Un hôte qui lirait « texte
    // vide, pas d'erreur » comme une réponse finie afficherait une bulle muette. La garantie
    // à tenir : `truncated` le dit.
    if !demande("plafond épuisé") {
        return;
    }
    let mut requete = ChatRequest::new("Tu réponds en un mot.", "Réponds exactement : pong");
    requete.max_output_tokens = Some(16);
    let reponse = client(deepseek(VARIABLE_DE_CLE))
        .chat(requete)
        .await
        .expect("le moteur répond");
    println!(
        "RACCORDEMENT plafond épuisé : modèle={} sortie={} tronqué={} texte={:?}",
        reponse.model, reponse.usage.output_tokens, reponse.truncated, reponse.text
    );
    if reponse.text.trim().is_empty() {
        assert!(
            reponse.truncated,
            "un texte vide sous plafond doit se dire tronqué : {reponse:?}"
        );
    }
}

#[tokio::test]
#[ignore = "raccordement réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn le_moteur_reel_rend_un_appel_d_outil_structure() {
    if !demande("outil") {
        return;
    }
    let messages = json!([
        {"role": "system", "content": "Tu n'écris pas de texte : tu appelles l'outil demandé."},
        {"role": "user", "content": "Appelle l'outil ping avec cible=\"atelier\"."}
    ]);
    let outils = json!([{
        "type": "function",
        "function": {
            "name": "ping",
            "description": "Envoie un ping.",
            "parameters": {
                "type": "object",
                "properties": {"cible": {"type": "string"}},
                "required": ["cible"]
            }
        }
    }]);
    let mut tour = TurnRequest::new(messages).with_tools(outils);
    // Pas 64 : un modèle qui raisonne dépense sa sortie avant l'appel. Mesuré le 21/11/2026 :
    // à 64, l'appel `ping` est bien émis mais le tour finit en `Length` et le test échoue.
    tour.max_output_tokens = Some(1024);
    let reponse = client(deepseek(VARIABLE_DE_CLE))
        .send_turn(tour)
        .await
        .expect("le moteur répond");
    let noms: Vec<String> = reponse
        .tool_calls
        .iter()
        .filter_map(|c| c.pointer("/function/name")?.as_str().map(str::to_string))
        .collect();
    println!(
        "RACCORDEMENT outil : modèle={} fin={} appels={noms:?}",
        reponse.model,
        reponse.finish.as_str()
    );
    assert_eq!(reponse.finish, Finish::ToolCalls, "{reponse:?}");
    assert_eq!(noms, vec!["ping".to_string()]);
    let arguments = reponse.tool_calls[0]
        .pointer("/function/arguments")
        .and_then(|a| a.as_str())
        .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
        .expect("des arguments JSON");
    assert_eq!(arguments["cible"], "atelier", "{arguments}");
}

#[tokio::test]
#[ignore = "raccordement réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn une_cle_refusee_est_une_reponse_du_moteur_pas_une_panne_reseau() {
    if !demande("clé refusée") {
        return;
    }
    // Une clé fausse, posée AVANT le lancement (voir l'en-tête) : le moteur la refuse.
    const FAUSSE: &str = "THIS_RACCORDEMENT_CLE_FAUSSE";
    assert!(
        std::env::var(FAUSSE).is_ok_and(|v| !v.is_empty()),
        "{FAUSSE} doit être posée avant le lancement (voir l'en-tête du fichier)"
    );
    let erreur = client(deepseek(FAUSSE))
        .chat(ChatRequest::new("sys", "bonjour"))
        .await
        .expect_err("une clé fausse est refusée");
    println!("RACCORDEMENT clé refusée : kind={:?}", erreur.kind());
    assert!(
        matches!(erreur, ProviderError::Status { .. }),
        "le moteur a répondu un statut : {erreur:?}"
    );
    assert_eq!(erreur.kind(), ErrorKind::Http(401), "{erreur:?}");
    assert!(
        !matches!(erreur.kind(), ErrorKind::Unreachable | ErrorKind::Timeout),
        "un refus d'authentification n'est pas une panne réseau"
    );
}

/// Le modèle que le MOTEUR dit avoir exécuté, lu dans le champ `model` de SA réponse.
///
/// `TurnReply::model` ne prouve rien : le client y recopie le modèle DEMANDÉ (`prepare` →
/// `model`), pas celui de la réponse. Une première version de ce test comparait donc le modèle
/// demandé à lui-même (tautologique, relevé par une revue indépendante le 21/11/2026). Ici, appel
/// `chat/completions` direct, le JSON brut du moteur est lu.
async fn modele_dit_par_le_moteur(demande: &str) -> String {
    let cle = std::env::var(VARIABLE_DE_CLE).expect("clé injectée par le coffre");
    let reponse: serde_json::Value = reqwest::Client::new()
        .post("https://api.deepseek.com/chat/completions")
        .bearer_auth(cle)
        .json(&json!({
            "model": demande,
            "messages": [{"role": "user", "content": "Réponds exactement : pong"}],
            "max_tokens": 1024,
        }))
        .send()
        .await
        .expect("le moteur répond")
        .json()
        .await
        .expect("JSON");
    reponse["model"]
        .as_str()
        .unwrap_or_else(|| panic!("la réponse du moteur n'a pas de champ model : {reponse}"))
        .to_string()
}

#[tokio::test]
#[ignore = "raccordement réel : THIS_RACCORDEMENT=deepseek et --ignored"]
async fn le_modele_nomme_par_la_reponse_du_moteur_est_egal_au_modele_demande() {
    if !demande("modèle servi par le moteur") {
        return;
    }
    let client = client(deepseek(VARIABLE_DE_CLE));
    let annonces: Vec<String> = client
        .list_models()
        .await
        .expect("le moteur annonce ses modèles")
        .into_iter()
        .map(|m| m.id)
        .collect();
    println!("RACCORDEMENT modèles annoncés : {annonces:?}");
    assert!(
        annonces.len() >= 2,
        "il faut deux modèles pour distinguer : {annonces:?}"
    );
    let mut dits = Vec::new();
    for demande in annonces.iter().take(2) {
        let dit = modele_dit_par_le_moteur(demande).await;
        println!("RACCORDEMENT moteur : demandé={demande} DIT={dit}");
        dits.push((demande.clone(), dit));
    }
    // Chaque modèle servi est celui que le moteur nomme dans SA réponse : égalité STRICTE, non
    // vide. Un alias n'est admis que s'il est déclaré ici, explicitement (aucun connu à ce jour) ;
    // un `starts_with` accepterait la chaîne vide et tout préfixe.
    const ALIAS: &[(&str, &str)] = &[];
    for (demande, dit) in &dits {
        assert!(
            !dit.is_empty(),
            "le moteur nomme un modèle (demandé {demande})"
        );
        let alias = ALIAS.iter().any(|(a, b)| a == demande && b == dit);
        assert!(
            dit == demande || alias,
            "demandé {demande}, le moteur dit {dit} (ni égal, ni alias déclaré)"
        );
    }
    assert_ne!(
        dits[0].1, dits[1].1,
        "deux modèles demandés, deux modèles dits : {dits:?}"
    );
}
