//! Un [`Transport`] adossé au client de `this-llm-providers` : le moteur d'un provider
//! (OpenAI-compatible) répond à un tour du harnais.
//!
//! Ce que ce transport DÉCLARE et ce qu'il ne fait pas (#288) :
//! - il honore la paire routée : il envoie le modèle de la requête, et renvoie dans
//!   [`Reply::served`] le modèle que le moteur NOMME dans sa réponse. Le harnais ne réputé
//!   la paire atteinte que si les deux concordent ;
//! - il ne rend pas le quota (`Failure::quota`) : le client ne distingue pas encore un
//!   quota d'un refus ordinaire. Un 429 reste un `Http(429)` ; le harnais ne devine rien ;
//! - les outils sont exécutés par le harnais (ce n'est pas une famille A) : aucune
//!   permission n'est donnée au moteur.

use async_trait::async_trait;
use serde_json::Value;
use this_llm_providers::{
    ErrorKind, Finish as LlmFinish, ProviderClient, ProviderError, TurnReply, TurnRequest,
};

use crate::ports::{Cancel, Delta, Transport};
use crate::wire::{Code, Failure, Finish, Reply, Request, ToolCall};

/// Un transport qui parle au moteur par le client de `this-llm-providers`.
pub struct ProviderTransport {
    client: ProviderClient,
}

impl ProviderTransport {
    pub fn new(client: ProviderClient) -> Self {
        Self { client }
    }
}

#[async_trait(?Send)]
impl Transport for ProviderTransport {
    async fn stream(
        &self,
        request: &Request,
        cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        let mut turn_request =
            TurnRequest::new(request.openai_messages()).with_model(request.model());
        if let Some(tools) = request.openai_tools() {
            turn_request = turn_request.with_tools(tools);
        }
        let mut turn = self
            .client
            .open_turn(turn_request)
            .await
            .map_err(failure_of)?;
        loop {
            if cancel.is_cancelled() {
                // Lâcher le tour rend sa réservation de budget (testé côté client).
                return Err(Failure::aborted());
            }
            match turn.next().await.map_err(failure_of)? {
                Some(piece) => delta(&piece),
                None => break,
            }
        }
        reply_of(turn.complete().map_err(failure_of)?)
    }

    fn honors_route(&self) -> bool {
        true
    }
}

/// La réponse du client, dans la forme du harnais. Le modèle servi est celui que le moteur a
/// nommé ; un appel d'outil dont les arguments ne se lisent pas est un refus, pas un appel
/// exécuté à moitié.
fn reply_of(reply: TurnReply) -> Result<Reply, Failure> {
    let calls = reply
        .tool_calls
        .iter()
        .map(call_of)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Reply {
        text: reply.content,
        calls,
        finish: finish_of(&reply.finish),
        served: reply.served,
    })
}

fn finish_of(finish: &LlmFinish) -> Finish {
    match finish {
        LlmFinish::Stop => Finish::Stop,
        LlmFinish::ToolCalls => Finish::Tools,
        LlmFinish::Length => Finish::Length,
        // Une raison que le moteur nomme sans que nous la connaissions : on ne suppose pas
        // une fin normale.
        LlmFinish::Other(_) => Finish::Open,
    }
}

/// Un appel au format OpenAI (`id`, `function.name`, `function.arguments` en chaîne JSON).
fn call_of(value: &Value) -> Result<ToolCall, Failure> {
    let id = value["id"]
        .as_str()
        .ok_or_else(|| Failure::new(Code::Unreadable, "appel d'outil sans identifiant"))?;
    let name = value["function"]["name"]
        .as_str()
        .ok_or_else(|| Failure::new(Code::Unreadable, "appel d'outil sans nom"))?;
    let raw = value["function"]["arguments"].as_str().unwrap_or("");
    let arguments = if raw.trim().is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(raw).map_err(|error| {
            Failure::new(
                Code::Unreadable,
                format!("arguments de l'outil {name} illisibles : {error}"),
            )
        })?
    };
    Ok(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
    })
}

/// Une panne du client, dans la famille que le harnais sait commuter.
fn failure_of(error: ProviderError) -> Failure {
    let message = error.to_string();
    let (code, status) = match error.kind() {
        ErrorKind::Unreachable => (Code::Unreachable, None),
        ErrorKind::Http(status) => (Code::Http, Some(status)),
        ErrorKind::Truncated => (Code::Truncated, None),
        ErrorKind::Unreadable => (Code::Unreadable, None),
        ErrorKind::Timeout => (Code::Timeout, None),
        ErrorKind::Provider => (Code::Provider, None),
    };
    let mut failure = Failure::new(code, message);
    failure.status = status;
    failure
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn turn(
        content: &str,
        tool_calls: Vec<Value>,
        finish: LlmFinish,
        served: Option<&str>,
    ) -> TurnReply {
        TurnReply {
            content: content.to_string(),
            tool_calls,
            finish,
            model: "demande".to_string(),
            served: served.map(str::to_owned),
            usage: Default::default(),
        }
    }

    #[test]
    fn la_reponse_garde_le_modele_servi_et_le_texte() {
        let reply =
            reply_of(turn("bonjour", vec![], LlmFinish::Stop, Some("m2"))).expect("réponse");
        assert_eq!(reply.text, "bonjour");
        assert_eq!(reply.served.as_deref(), Some("m2"));
        assert_eq!(reply.finish, Finish::Stop);
    }

    #[test]
    fn une_reponse_sans_modele_nomme_reste_sans_modele_servi() {
        let reply = reply_of(turn("ok", vec![], LlmFinish::Stop, None)).expect("réponse");
        assert_eq!(reply.served, None);
    }

    #[test]
    fn les_raisons_de_fin_se_traduisent_sans_supposer_une_fin_normale() {
        assert_eq!(finish_of(&LlmFinish::Stop), Finish::Stop);
        assert_eq!(finish_of(&LlmFinish::ToolCalls), Finish::Tools);
        assert_eq!(finish_of(&LlmFinish::Length), Finish::Length);
        assert_eq!(
            finish_of(&LlmFinish::Other("content_filter".into())),
            Finish::Open
        );
    }

    #[test]
    fn un_appel_d_outil_relit_ses_arguments_json() {
        let call = call_of(&json!({
            "id": "call_1", "type": "function",
            "function": { "name": "lire", "arguments": "{\"path\":\"a.txt\"}" }
        }))
        .expect("appel");
        assert_eq!(call.id, "call_1");
        assert_eq!(call.name, "lire");
        assert_eq!(call.arguments, json!({ "path": "a.txt" }));
    }

    #[test]
    fn des_arguments_vides_valent_un_objet_vide() {
        let call = call_of(&json!({ "id": "c", "function": { "name": "ping", "arguments": "" } }))
            .expect("appel");
        assert_eq!(call.arguments, json!({}));
    }

    #[test]
    fn des_arguments_illisibles_sont_un_refus_pas_un_appel_partiel() {
        let failure = call_of(&json!({
            "id": "c", "function": { "name": "lire", "arguments": "{\"path\":" }
        }))
        .expect_err("refus");
        assert_eq!(failure.code, Code::Unreadable);
        assert!(failure.message.contains("lire"), "{}", failure.message);
    }

    #[test]
    fn chaque_panne_du_client_arrive_dans_sa_famille() {
        let http = failure_of(ProviderError::Status {
            code: 503,
            excerpt: "indisponible".into(),
        });
        assert_eq!(http.code, Code::Http);
        assert_eq!(http.status, Some(503));

        assert_eq!(
            failure_of(ProviderError::Transport("refusée".into())).code,
            Code::Unreachable
        );
        assert_eq!(
            failure_of(ProviderError::Timeout("silence".into())).code,
            Code::Timeout
        );
        assert_eq!(failure_of(ProviderError::StreamCut).code, Code::Truncated);
        assert_eq!(
            failure_of(ProviderError::Parse("json".into())).code,
            Code::Unreadable
        );
        assert_eq!(
            failure_of(ProviderError::NoModel("x".into())).code,
            Code::Provider
        );
        // Une panne qui n'est pas une panne réseau reste hors d'`Unreachable`.
        assert_ne!(
            failure_of(ProviderError::NoModel("x".into())).code,
            Code::Unreachable
        );
    }

    #[test]
    fn un_appel_sans_identifiant_ou_sans_nom_est_refuse() {
        let sans_id = call_of(&json!({ "function": { "name": "x", "arguments": "{}" } }));
        assert_eq!(sans_id.expect_err("refus").code, Code::Unreadable);
        let sans_nom = call_of(&json!({ "id": "c", "function": { "arguments": "{}" } }));
        assert_eq!(sans_nom.expect_err("refus").code, Code::Unreadable);
    }
}
