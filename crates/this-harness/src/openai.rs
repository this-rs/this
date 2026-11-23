//! Le fil et les outils d'un tour, dans la forme **neutre** que `this-llm-providers` lit
//! (`TurnRequest::messages` et `with_tools`). Le harnais parle son propre vocabulaire
//! ([`Message`], [`ToolSpec`]) ; ce module le traduit une fois, à la frontière, pour que le
//! transport d'un moteur OpenAI-compatible n'ait plus qu'à le passer au client.
//!
//! Règles de forme (celles que le client attend) :
//! - la consigne système est déjà la première entrée de [`Request::thread`] : elle sort en
//!   `role: system`, une seule fois ;
//! - un message utilisateur sans image s'écrit `{role, content}` exactement — pas de clé
//!   `images` vide ; avec images, la clé `images` porte les [`ImagePart`] tels quels ;
//! - les appels d'outils d'un assistant deviennent `tool_calls`, dont les `arguments` sont
//!   une CHAÎNE JSON (ce qu'OpenAI attend) ;
//! - le résultat d'un outil est `role: tool` relié par `tool_call_id` ;
//! - le drapeau `synthetic` (une relance du harnais, pas la question) n'est jamais écrit :
//!   c'est une marque interne, le moteur n'en a pas l'usage.

use serde_json::{Value, json};

use crate::wire::{Message, Request, ToolCall, ToolSpec};

impl Request {
    /// Le fil du tour, prêt pour `TurnRequest::new`. Pour un hôte qui écrit son propre
    /// transport, c'est la traduction à utiliser (une seule place où la forme est décidée).
    pub fn openai_messages(&self) -> Value {
        Value::Array(self.thread().iter().map(message_openai).collect())
    }

    /// Les outils offerts au modèle, prêts pour `TurnRequest::with_tools` ; `None` s'il n'y
    /// en a aucun (le client n'envoie alors pas de champ `tools`).
    pub fn openai_tools(&self) -> Option<Value> {
        let tools = self.tools();
        if tools.is_empty() {
            return None;
        }
        Some(Value::Array(tools.iter().map(tool_openai).collect()))
    }
}

fn message_openai(message: &Message) -> Value {
    match message {
        Message::System { content } => json!({ "role": "system", "content": content }),
        Message::User {
            content, images, ..
        } => {
            let mut user = json!({ "role": "user", "content": content });
            if !images.is_empty() {
                // `ImagePart` est sérialisable : c'est la forme neutre que le client rend.
                user["images"] = serde_json::to_value(images).unwrap_or(Value::Null);
            }
            user
        }
        Message::Assistant { content, calls } => {
            let mut assistant = json!({ "role": "assistant", "content": content });
            if !calls.is_empty() {
                assistant["tool_calls"] = Value::Array(calls.iter().map(call_openai).collect());
            }
            assistant
        }
        Message::Tool { id, content } => {
            json!({ "role": "tool", "tool_call_id": id, "content": content })
        }
    }
}

fn call_openai(call: &ToolCall) -> Value {
    // Un appel sans argument s'écrit `{}`, pas `null` : un moteur refuse un `null` là où il
    // attend un objet JSON.
    let arguments = match &call.arguments {
        Value::Null => "{}".to_string(),
        other => other.to_string(),
    };
    json!({
        "id": call.id,
        "type": "function",
        "function": { "name": call.name, "arguments": arguments },
    })
}

fn tool_openai(tool: &ToolSpec) -> Value {
    let parameters = match &tool.parameters {
        Value::Null => json!({ "type": "object", "properties": {} }),
        schema => schema.clone(),
    };
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": parameters,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_fil_texte_seul_s_ecrit_en_roles_openai() {
        let thread = vec![
            Message::user("Lis a.txt"),
            Message::Assistant {
                content: String::new(),
                calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "lire".into(),
                    arguments: json!({ "path": "a.txt" }),
                }],
            },
            Message::Tool {
                id: "call_1".into(),
                content: "bonjour".into(),
            },
            Message::Assistant {
                content: "Il dit bonjour.".into(),
                calls: vec![],
            },
        ];
        let out = render("Tu es bref.", &thread);
        assert_eq!(
            out,
            json!([
                { "role": "system", "content": "Tu es bref." },
                { "role": "user", "content": "Lis a.txt" },
                { "role": "assistant", "content": "", "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": { "name": "lire", "arguments": "{\"path\":\"a.txt\"}" }
                }]},
                { "role": "tool", "tool_call_id": "call_1", "content": "bonjour" },
                { "role": "assistant", "content": "Il dit bonjour." },
            ])
        );
    }

    #[test]
    fn un_message_utilisateur_sans_image_n_a_aucune_cle_images() {
        let out = render("", &[Message::user("salut")]);
        assert_eq!(out, json!([{ "role": "user", "content": "salut" }]));
    }

    #[test]
    fn le_drapeau_synthetic_n_est_jamais_ecrit() {
        let out = render("", &[Message::hint("relance du harnais")]);
        assert_eq!(
            out,
            json!([{ "role": "user", "content": "relance du harnais" }])
        );
    }

    #[test]
    fn un_appel_sans_argument_s_ecrit_objet_vide() {
        let call = ToolCall {
            id: "c".into(),
            name: "ping".into(),
            arguments: Value::Null,
        };
        assert_eq!(call_openai(&call)["function"]["arguments"], "{}");
    }

    #[test]
    fn sans_outil_aucun_champ_tools_n_est_produit() {
        assert_eq!(
            Request::new(String::new(), vec![], vec![]).openai_tools(),
            None
        );
    }

    #[test]
    fn les_outils_s_ecrivent_en_fonctions_et_sans_outil_pas_de_champ() {
        let tools = [ToolSpec {
            name: "lire".into(),
            description: "Lit un fichier".into(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
        }];
        assert_eq!(
            tool_openai(&tools[0]),
            json!({ "type": "function", "function": {
                "name": "lire", "description": "Lit un fichier",
                "parameters": { "type": "object", "properties": { "path": { "type": "string" } } }
            }})
        );
    }

    /// Le vrai chemin : une `Request` construite comme la session la construit, puis rendue
    /// par `openai_messages` (pas une copie de la logique, qui ne prouverait rien du câblage).
    fn render(system: &str, thread: &[Message]) -> Value {
        Request::new(system.to_string(), thread.to_vec(), Vec::new()).openai_messages()
    }
}
