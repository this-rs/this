//! Les types du fil : requête (§4), réponse (§5), effets (§6) de l'ABI v1.

use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use serde_json::{Map, Value};

/// L'opération demandée. `Call` = opération nommée (`Request::name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Create,
    Get,
    List,
    Update,
    Delete,
    Call,
}

impl Op {
    pub fn parse(s: &str) -> Option<Op> {
        Some(match s {
            "create" => Op::Create,
            "get" => Op::Get,
            "list" => Op::List,
            "update" => Op::Update,
            "delete" => Op::Delete,
            "call" => Op::Call,
            _ => return None,
        })
    }
}

/// Le contexte : le temps et l'identité sont des **données**.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Ctx {
    #[serde(default)]
    pub now_ms: i64,
    /// Le tenant courant. **`null` est normal** : un hôte qui n'isole pas encore par
    /// tenant l'envoie absent. Le typer `String` faisait echouer TOUTE requete d'un tel
    /// hote - mesure sur le premier module Rust reel charge par le framework.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Le compte appelant, meme remarque que `tenant`.
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub config: Value,
    /// `true` si l'hôte a tronqué `rows` (plafond `max_rows`).
    #[serde(default)]
    pub truncated: bool,
}

/// La requête (§4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub op: Op,
    /// Seulement pour `Op::Call`.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub entity: Option<String>,
    #[serde(default)]
    pub input: Value,
    /// Lignes de la portée du tenant, bornées par l'hôte. Ce qui n'y est pas est invisible.
    #[serde(default)]
    pub rows: Vec<Value>,
    #[serde(default)]
    pub ctx: Ctx,
}

impl Request {
    /// Un champ de `input`, s'il existe.
    pub fn input_field(&self, name: &str) -> Option<&Value> {
        self.input.get(name)
    }
    /// Une clé de `ctx.config`, si elle existe.
    pub fn config(&self, key: &str) -> Option<&Value> {
        self.ctx.config.get(key)
    }
    /// La ligne de `rows` dont l'`id` est `id`.
    pub fn row(&self, id: &str) -> Option<&Value> {
        self.rows
            .iter()
            .find(|r| r.get("id").and_then(Value::as_str) == Some(id))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// Un effet (§6) : ensemble fermé, appliqué dans l'ordre par l'hôte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    /// Écrit la ligne : create sans `id`, update avec.
    Put {
        entity: String,
        body: Value,
    },
    Delete {
        entity: String,
        id: String,
    },
    Emit {
        topic: String,
        payload: Value,
    },
    Log {
        level: Level,
        message: String,
    },
}

impl Effect {
    pub fn put(entity: impl Into<String>, body: Value) -> Effect {
        Effect::Put {
            entity: entity.into(),
            body,
        }
    }
    pub fn delete(entity: impl Into<String>, id: impl Into<String>) -> Effect {
        Effect::Delete {
            entity: entity.into(),
            id: id.into(),
        }
    }
    pub fn emit(topic: impl Into<String>, payload: Value) -> Effect {
        Effect::Emit {
            topic: topic.into(),
            payload,
        }
    }
    pub fn log(level: Level, message: impl Into<String>) -> Effect {
        Effect::Log {
            level,
            message: message.into(),
        }
    }
}

/// Un échec métier : une valeur, jamais un panic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
    /// Le champ fautif, s'il y en a un (absent du JSON sinon).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl Error {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Error {
        Error {
            code: code.into(),
            message: message.into(),
            field: None,
        }
    }
    pub fn field(mut self, field: impl Into<String>) -> Error {
        self.field = Some(field.into());
        self
    }
}

/// La réponse (§5).
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Ok { output: Value, effects: Vec<Effect> },
    Err(Error),
}

impl Response {
    /// Succès sans effet.
    pub fn ok(output: Value) -> Response {
        Response::Ok {
            output,
            effects: vec![],
        }
    }
    /// Succès avec effets.
    pub fn ok_with(output: Value, effects: Vec<Effect>) -> Response {
        Response::Ok { output, effects }
    }
    /// Échec : `Response::err("code", "message")`.
    pub fn err(code: impl Into<String>, message: impl Into<String>) -> Response {
        Response::Err(Error::new(code, message))
    }
    /// Échec portant sur un champ.
    pub fn field_err(
        code: impl Into<String>,
        message: impl Into<String>,
        field: impl Into<String>,
    ) -> Response {
        Response::Err(Error::new(code, message).field(field))
    }
    /// Ajoute un effet à un succès (sans effet sur un échec).
    pub fn effect(mut self, e: Effect) -> Response {
        if let Response::Ok { effects, .. } = &mut self {
            effects.push(e);
        }
        self
    }
}

impl From<Error> for Response {
    fn from(e: Error) -> Response {
        Response::Err(e)
    }
}

impl Serialize for Response {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Response::Ok { output, effects } => {
                let mut st = s.serialize_struct("Response", 3)?;
                st.serialize_field("ok", &true)?;
                st.serialize_field("output", output)?;
                st.serialize_field("effects", effects)?;
                st.end()
            }
            Response::Err(e) => {
                let mut st = s.serialize_struct("Response", 2)?;
                st.serialize_field("ok", &false)?;
                st.serialize_field("error", e)?;
                st.end()
            }
        }
    }
}

/// `{ "ok": ... }` -> `Response`, pour les tests d'un module.
impl<'de> Deserialize<'de> for Response {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let m = Map::<String, Value>::deserialize(d)?;
        let take = |k: &str, def: Value| m.get(k).cloned().unwrap_or(def);
        match m.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(Response::Ok {
                output: take("output", Value::Null),
                effects: serde_json::from_value(take("effects", Value::Array(vec![])))
                    .map_err(D::Error::custom)?,
            }),
            Some(false) => Ok(Response::Err(
                serde_json::from_value(take("error", Value::Null)).map_err(D::Error::custom)?,
            )),
            None => Err(D::Error::custom("`ok` manquant")),
        }
    }
}
