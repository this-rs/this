//! Ce qui traverse le fil : les messages, les appels d'outils, la réponse.
//!
//! Rien ici ne connaît de protocole. `Message` est la forme que le harnais
//! manipule ; c'est le [`Transport`](crate::Transport) de l'application qui
//! la traduit dans le dialecte de son moteur (OpenAI, CLI, autre). Un harnais
//! qui parlerait directement l'API d'un fournisseur serait un harnais qui
//! change de fournisseur en même temps que de version.

use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use this_llm::{ImagePart, ImageSource};

/// Un message de la conversation, tel que le modèle le lira.
///
/// La variante `Tool` porte l'identifiant de l'appel qu'elle répond : sans lui,
/// un moteur refuse la conversation, et deux outils appelés dans le même tour
/// deviennent indiscernables.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
        /// Les images jointes au message. Absent du JSON quand il n'y en a pas : un
        /// message texte seul s'écrit exactement comme avant ce champ (test
        /// `a_text_only_user_message_is_byte_for_byte_unchanged`).
        ///
        /// La source dit OÙ sont les octets, pas comment les envoyer : c'est le
        /// transport qui les traduit dans le dialecte du moteur.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImagePart>,
        /// Un message écrit par le harnais (une relance après un refus, un
        /// silence, une panne), pas par l'utilisateur : il ne doit pas passer
        /// pour la question. Sans cette marque, la borne de contexte épinglait
        /// la relance comme « dernier message utilisateur » et pouvait retirer
        /// la vraie question. Additif : absent du JSON quand il est faux, donc
        /// un message existant se relit et s'écrit à l'identique.
        #[serde(default, skip_serializing_if = "is_false")]
        synthetic: bool,
    },
    Assistant {
        #[serde(default)]
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        calls: Vec<ToolCall>,
    },
    Tool {
        id: String,
        content: String,
    },
}

fn is_false(valeur: &bool) -> bool {
    !*valeur
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
            images: Vec::new(),
            synthetic: false,
        }
    }
    /// Une relance du harnais, rangée au rôle utilisateur (les moteurs n'en ont
    /// pas d'autre) mais marquée `synthetic` : ce n'est pas la question.
    pub fn hint(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
            images: Vec::new(),
            synthetic: true,
        }
    }
    /// Un message de l'utilisateur avec ses images.
    pub fn user_with_images(content: impl Into<String>, images: Vec<ImagePart>) -> Self {
        Self::User {
            content: content.into(),
            images,
            synthetic: false,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: content.into(),
            calls: Vec::new(),
        }
    }
    pub fn tool(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Tool {
            id: id.into(),
            content: content.into(),
        }
    }
    /// Le texte visible d'un message, quel qu'il soit.
    pub fn text(&self) -> &str {
        match self {
            Self::System { content } | Self::User { content, .. } | Self::Tool { content, .. } => {
                content
            }
            Self::Assistant { content, .. } => content,
        }
    }
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }
    /// Vrai pour une relance du harnais ([`Message::hint`]).
    pub fn is_synthetic(&self) -> bool {
        matches!(
            self,
            Self::User {
                synthetic: true,
                ..
            }
        )
    }
    /// Refuse, **à la construction**, un fil que le moteur refuserait : un id vide,
    /// deux appels de même id dans un `Assistant`, un `Tool` sans appel en attente
    /// (ou une seconde réponse au même appel).
    /// DÉFAUT corrigé : rien de cela n'était vérifié ici ; l'erreur n'arrivait qu'au
    /// tour suivant, depuis le moteur distant, sans dire quel message.
    pub fn check_thread(messages: &[Message]) -> Result<(), ThreadDefect> {
        // Appels du dernier `Assistant` sans réponse encore.
        let mut pending: Vec<&str> = Vec::new();
        for (index, m) in messages.iter().enumerate() {
            match m {
                Self::Assistant { calls, .. } => {
                    let mut seen = std::collections::HashSet::new();
                    for c in calls {
                        if c.id.is_empty() {
                            return Err(ThreadDefect::EmptyId { index });
                        }
                        if !seen.insert(c.id.as_str()) {
                            return Err(ThreadDefect::DuplicateCallId { index });
                        }
                    }
                    pending = calls.iter().map(|c| c.id.as_str()).collect();
                }
                Self::Tool { id, .. } => {
                    if id.is_empty() {
                        return Err(ThreadDefect::EmptyId { index });
                    }
                    match pending.iter().position(|p| p == id) {
                        Some(at) => {
                            pending.swap_remove(at);
                        }
                        None => return Err(ThreadDefect::OrphanTool { index }),
                    }
                }
                Self::User { .. } => pending.clear(),
                Self::System { .. } => {}
            }
        }
        Ok(())
    }
    /// Les images du message (aucune hors message utilisateur).
    pub fn images(&self) -> &[ImagePart] {
        match self {
            Self::User { images, .. } => images,
            _ => &[],
        }
    }
}

/// Pourquoi un fil est invalide ([`Message::check_thread`]) ; `index` est celui du message fautif.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadDefect {
    /// Un identifiant d'appel ou de réponse est vide.
    EmptyId { index: usize },
    /// Deux appels d'un même `Assistant` portent le même id.
    DuplicateCallId { index: usize },
    /// Un `Tool` ne répond à aucun appel en attente.
    OrphanTool { index: usize },
}

/// Un outil demandé par le modèle.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Déjà analysé : un argument mal formé est un refus, pas une panique au
    /// fond de l'exécution.
    #[serde(default)]
    pub arguments: Value,
}

/// Un outil tel qu'on le présente au modèle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// Un JSON Schema (`{"type":"object","properties":{...}}`).
    #[serde(default)]
    pub parameters: Value,
}

/// Une demande de complétion : tout ce qu'il faut pour un tour, et rien de plus.
///
/// Les champs sont privés **exprès**. Un transport qui ne lit que `messages`
/// oublie la consigne système — c'est exactement le bug qui a laissé l'Atelier
/// sans brief pendant une version entière : le modèle recevait la conversation
/// sans savoir quels outils il avait, ni qu'il devait agir au lieu de décrire.
/// Ici, le seul accès au fil est [`Request::thread`], qui met la consigne en
/// tête. L'oubli n'est plus possible : il n'y a pas d'autre porte.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    system: String,
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
    /// La paire choisie pour ce tour par le routeur (mode Auto). `None` : aucun
    /// routeur n'est branché, le transport choisit comme il l'a toujours fait.
    route: Option<crate::routing::Route>,
}

impl Request {
    pub(crate) fn new(system: String, messages: Vec<Message>, tools: Vec<ToolSpec>) -> Self {
        Self {
            system,
            messages,
            tools,
            route: None,
        }
    }

    /// Pose la paire du tour (voir [`crate::Harness::with_router`]).
    pub(crate) fn with_route(mut self, route: Option<crate::routing::Route>) -> Self {
        self.route = route;
        self
    }

    /// Ajoute le contexte éphémère d'un tour : un `Message::system` juste avant
    /// le dernier message utilisateur — là où le modèle le lira comme « ce qui
    /// vaut pour cette demande ». Sans message utilisateur, il va en fin de fil.
    /// `None` ou un texte vide : la requête est rendue intacte.
    pub(crate) fn with_context(mut self, context: Option<String>) -> Self {
        let Some(context) = context.filter(|c| !c.trim().is_empty()) else {
            return self;
        };
        let at = self
            .messages
            .iter()
            .rposition(Message::is_user)
            .unwrap_or(self.messages.len());
        self.messages.insert(at, Message::system(context));
        self
    }

    /// Le fil tel que le modèle doit le lire : la consigne système, puis les
    /// messages. C'est la seule façon d'obtenir les messages.
    pub fn thread(&self) -> Vec<Message> {
        let mut all = Vec::with_capacity(self.messages.len() + 1);
        if !self.system.trim().is_empty() {
            all.push(Message::system(&self.system));
        }
        all.extend(self.messages.iter().cloned());
        all
    }

    pub fn system(&self) -> &str {
        &self.system
    }

    pub fn tools(&self) -> &[ToolSpec] {
        &self.tools
    }

    /// Le modèle sur lequel ce tour doit partir, quand un routeur l'a choisi.
    ///
    /// `None` : rien n'est imposé, le transport garde son modèle. Jamais la
    /// sentinelle `auto` — ce n'est pas un nom de modèle, un moteur la rejette —
    /// et c'est garanti par construction ([`crate::routing::Route`]).
    pub fn model(&self) -> Option<&str> {
        self.route.as_ref().and_then(crate::routing::Route::model)
    }

    /// L'instance de fournisseur sur laquelle ce tour doit partir, quand un
    /// routeur l'a choisie. `None` : aucun routeur n'est branché.
    pub fn provider(&self) -> Option<&str> {
        self.route.as_ref().map(crate::routing::Route::provider)
    }

    /// Rien à envoyer : pas même un message utilisateur.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Pourquoi le moteur a cessé de parler.
///
/// La distinction n'est pas décorative : `Length` veut dire « la réponse est
/// coupée, elle n'a pas conclu », et la traiter comme une fin fait croire à
/// l'utilisateur que le travail est terminé alors qu'il s'est arrêté au milieu
/// d'une phrase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finish {
    /// Le moteur n'a rien dit de sa fin — un flux coupé ressemble à ça.
    #[default]
    Open,
    /// Fin normale.
    Stop,
    /// Budget de tokens atteint : la réponse est tronquée.
    Length,
    /// Le modèle demande des outils et attend leurs résultats.
    Tools,
    /// Le transport a interrompu le flux (annulation).
    Aborted,
}

/// Ce que le moteur a répondu pour un tour.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub finish: Finish,
    /// Le modèle que le moteur NOMME dans sa réponse (#288, routé ≠ servi). Il doit être celui
    /// que CETTE tentative a demandé — ou un instantané de celui-ci, voir [`served_matches`] —
    /// pour que la paire routée soit réputée atteinte. `None` : la réponse n'en nomme aucun, et le
    /// harnais n'apprend alors rien de ce tour.
    pub served: Option<String>,
}

/// Le modèle que le moteur a NOMMÉ est-il celui qu'on lui a demandé ?
///
/// Vrai pour le même nom, ou pour le nom demandé suivi d'un suffixe d'**instantané** purement
/// numérique — séparateur `-`, `:` ou `@`, puis des chiffres, points et tirets commençant par un
/// chiffre : `gpt-4o` → `gpt-4o-2024-08-06`, `deepseek-v3` → `deepseek-v3-0324`. Les moteurs
/// résolvent un alias en instantané daté et renvoient CE nom ; une égalité stricte rendrait
/// l'apprentissage muet sur eux, sans signal.
///
/// Faux pour tout le reste, et c'est voulu : un suffixe alphabétique (`-mini`, `:7b`, `-turbo`)
/// nomme un AUTRE modèle ; un alias servi pour un instantané demandé est un écart (la personne a
/// demandé une version précise) ; un nom vide ne prouve rien.
pub fn served_matches(asked: &str, served: &str) -> bool {
    if asked.is_empty() || served.is_empty() {
        return false;
    }
    if asked == served {
        return true;
    }
    let Some(rest) = served.strip_prefix(asked) else {
        return false;
    };
    let mut chars = rest.chars();
    if !matches!(chars.next(), Some('-' | ':' | '@')) {
        return false;
    }
    let suffix = chars.as_str();
    suffix.starts_with(|c: char| c.is_ascii_digit())
        && suffix
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
}

/// La famille d'une panne de transport. Un code se commute, un message se lit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// Le moteur n'a pas répondu du tout (DNS, refus de connexion).
    Unreachable,
    /// Le moteur a répondu, mais en erreur.
    Http,
    /// Le flux s'est arrêté sans dire qu'il était fini.
    Truncated,
    /// La réponse ne se lit pas.
    Unreadable,
    /// Le délai est dépassé.
    Timeout,
    /// L'appel a été annulé par l'utilisateur.
    Aborted,
    /// Un refus raconté par le fournisseur lui-même.
    Provider,
}

impl Code {
    /// Le nom stable de la variante, tel que serde l'écrit.
    ///
    /// C'est LA forme qui part sur le fil : le jour où une variante porte deux
    /// mots (`RateLimited`), `Debug` écrirait `ratelimited` là où serde écrit
    /// `rate_limited` — deux noms pour la même chose. `as_str` est aligné sur
    /// serde, pas sur `Debug` ; un test le prouve variante par variante.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Http => "http",
            Self::Truncated => "truncated",
            Self::Unreadable => "unreadable",
            Self::Timeout => "timeout",
            Self::Aborted => "aborted",
            Self::Provider => "provider",
        }
    }
}

/// Ce que refuse un quota épuisé : un seul modèle de l'instance, ou tout le compte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaScope {
    /// Un seul modèle refuse : un autre modèle de la même instance peut servir.
    Model,
    /// Le compte entier refuse : tout modèle de l'instance refuserait aussi.
    Account,
}

/// Le refus d'un quota, tel que le moteur le dit — jamais deviné par le harnais.
///
/// Un transport qui sait distinguer un quota épuisé d'un autre refus (en-têtes de
/// limite, corps d'erreur du fournisseur) le pose sur [`Failure::quota`]. Sans lui, un
/// refus de quota reste une panne ordinaire (`Http`, 429…) : le harnais ne bascule JAMAIS
/// de modèle sur une supposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaRefusal {
    pub scope: QuotaScope,
    /// L'instant où le quota sert de nouveau (ms depuis l'époque), quand le moteur le dit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<u64>,
}

/// Combien de temps un modèle passagèrement indisponible (serveur en erreur, silence) est laissé
/// de côté avant d'être réessayé.
pub const TEMPORARY_UNAVAILABLE_MS: u64 = 120_000;

/// Combien de temps un modèle dont le quota est épuisé est laissé de côté. Plus long qu'une
/// panne : un quota ne revient pas en quelques secondes, et le rejouer coûte un refus de plus.
pub const QUOTA_UNAVAILABLE_MS: u64 = 600_000;

/// Pourquoi, et pour combien de temps, UN modèle ne sert plus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailability {
    /// Passagère : le modèle est laissé de côté `cooldown_ms`, puis réessayé.
    Temporary { cooldown_ms: u64 },
    /// Complète : le moteur ne connaît pas (ou plus) ce modèle. Il reste de côté jusqu'à ce qu'on
    /// le remette (réglage de l'instance changé, catalogue qui le ré-annonce, geste de la personne)
    /// — aucun délai ne le fera revenir d'un modèle qui n'existe pas.
    Complete,
}

/// Une panne lue comme l'indisponibilité d'UN modèle : sa durée et sa raison courte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelDown {
    pub kind: Unavailability,
    /// Un code court et stable (`http 404`, `http 503`, `timeout`, `quota`), jamais le texte du
    /// moteur : il va dans les journaux et l'interface.
    pub reason: String,
}

/// Une panne de transport, avec sa famille et sa phrase.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub code: Code,
    pub message: String,
    /// Le statut HTTP, quand il y en a un.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Le quota qui refuse, quand le transport sait le dire. `None` : un refus ordinaire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaRefusal>,
}

impl Failure {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
            quota: None,
        }
    }
    pub fn http(status: u16) -> Self {
        Self {
            code: Code::Http,
            message: format!("le moteur a répondu {status}"),
            status: Some(status),
            quota: None,
        }
    }
    /// Le moteur refuse parce qu'un quota est épuisé (voir [`QuotaRefusal`]).
    pub fn quota_exhausted(scope: QuotaScope, resets_at_ms: Option<u64>) -> Self {
        Self {
            code: Code::Http,
            message: match scope {
                QuotaScope::Model => "le moteur refuse : quota épuisé pour ce modèle".to_string(),
                QuotaScope::Account => "le moteur refuse : quota du compte épuisé".to_string(),
            },
            status: Some(429),
            quota: Some(QuotaRefusal {
                scope,
                resets_at_ms,
            }),
        }
    }
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self::new(Code::Unreachable, message)
    }
    pub fn truncated(said: &str) -> Self {
        Self::new(
            Code::Truncated,
            format!("le flux s'est arrêté sans fin (ni [DONE] ni finish_reason). Reçu :\n{said}"),
        )
    }
    pub fn aborted() -> Self {
        Self::new(Code::Aborted, "appel annulé")
    }
    /// Vrai quand l'utilisateur a demandé l'arrêt : ce n'est pas un échec.
    pub fn is_aborted(&self) -> bool {
        self.code == Code::Aborted
    }

    /// Cette panne dit-elle qu'UN modèle ne sert pas, alors qu'un autre modèle de la même instance
    /// le pourrait ? Lu des faits que le transport énonce, jamais d'une supposition :
    ///
    /// - un quota de MODÈLE dit épuisé → passager (long) ;
    /// - `404` / `410` : le moteur ne connaît pas ce modèle → complète ;
    /// - un `5xx` ou un délai dépassé → passager.
    ///
    /// Tout le reste rend `None` : une annulation, un quota de COMPTE (tout modèle de l'instance
    /// refuserait), `401`/`403` (la clé, pas le modèle), `400` (la requête, pas le modèle), un `429`
    /// nu (limite de débit ou quota, rien ne le dit), un moteur injoignable (l'instance, pas le
    /// modèle), une réponse coupée ou illisible.
    pub fn model_down(&self) -> Option<ModelDown> {
        if self.is_aborted() {
            return None;
        }
        if let Some(quota) = self.quota {
            return (quota.scope == QuotaScope::Model).then(|| ModelDown {
                kind: Unavailability::Temporary {
                    cooldown_ms: QUOTA_UNAVAILABLE_MS,
                },
                reason: "quota".to_string(),
            });
        }
        match (self.code, self.status) {
            (Code::Http, Some(status @ (404 | 410))) => Some(ModelDown {
                kind: Unavailability::Complete,
                reason: format!("http {status}"),
            }),
            (Code::Http, Some(status)) if (500..=599).contains(&status) => Some(ModelDown {
                kind: Unavailability::Temporary {
                    cooldown_ms: TEMPORARY_UNAVAILABLE_MS,
                },
                reason: format!("http {status}"),
            }),
            (Code::Timeout, _) => Some(ModelDown {
                kind: Unavailability::Temporary {
                    cooldown_ms: TEMPORARY_UNAVAILABLE_MS,
                },
                reason: "timeout".to_string(),
            }),
            _ => None,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

#[cfg(test)]
mod tests {
    use super::*;

    /// L'exigence dure : ajouter `images` ne change RIEN au JSON d'un message texte
    /// seul. Les chaînes attendues sont écrites à la main, telles que la version
    /// précédente du contrat les produisait.
    #[test]
    fn a_text_only_user_message_is_byte_for_byte_unchanged() {
        let message = Message::user("bonjour « là » \n\"x\"");
        let written = serde_json::to_string(&message).unwrap();
        assert_eq!(
            written,
            r#"{"role":"user","content":"bonjour « là » \n\"x\""}"#
        );
        assert_eq!(
            serde_json::to_vec(&Message::user("")).unwrap(),
            br#"{"role":"user","content":""}"#.to_vec()
        );
        // Une conversation entière, comme `Snapshot` la sauvegarde.
        let thread = vec![
            Message::system("brief"),
            Message::user("q"),
            Message::assistant("r"),
            Message::tool("c1", "ok"),
        ];
        assert_eq!(
            serde_json::to_string(&thread).unwrap(),
            r#"[{"role":"system","content":"brief"},{"role":"user","content":"q"},{"role":"assistant","content":"r"},{"role":"tool","id":"c1","content":"ok"}]"#
        );
    }

    /// Le champ `synthetic` est additif : absent quand il est faux (octets
    /// inchangés), relu à `false` s'il manque, écrit et relu quand il est vrai.
    #[test]
    fn synthetic_is_additive_on_the_wire() {
        let old: Message = serde_json::from_str(r#"{"role":"user","content":"q"}"#).unwrap();
        assert!(!old.is_synthetic());
        assert_eq!(old, Message::user("q"));

        let hint = Message::hint("relance");
        let written = serde_json::to_string(&hint).unwrap();
        assert_eq!(
            written,
            r#"{"role":"user","content":"relance","synthetic":true}"#
        );
        let back: Message = serde_json::from_str(&written).unwrap();
        assert_eq!(back, hint);
        assert!(back.is_user() && back.is_synthetic());
    }

    #[test]
    fn an_old_user_message_reads_with_no_image() {
        let message: Message = serde_json::from_str(r#"{"role":"user","content":"q"}"#).unwrap();
        assert_eq!(message, Message::user("q"));
        assert!(message.images().is_empty());
    }

    #[test]
    fn a_user_message_with_images_round_trips_and_names_its_sources() {
        let message = Message::user_with_images(
            "regarde",
            vec![
                ImagePart::new("image/png", ImageSource::Ref("ab".repeat(32))).named("a.png"),
                ImagePart::new("image/jpeg", ImageSource::Base64("AAAA".into())),
                ImagePart::new("image/webp", ImageSource::Url("https://x/y.webp".into())),
            ],
        );
        let written = serde_json::to_value(&message).unwrap();
        assert_eq!(
            written,
            serde_json::json!({
                "role": "user",
                "content": "regarde",
                "images": [
                    { "mime": "image/png", "source": { "ref": "ab".repeat(32) }, "name": "a.png" },
                    { "mime": "image/jpeg", "source": { "base64": "AAAA" } },
                    { "mime": "image/webp", "source": { "url": "https://x/y.webp" } },
                ]
            })
        );
        let back: Message = serde_json::from_value(written).unwrap();
        assert_eq!(back, message);
        assert_eq!(back.text(), "regarde");
    }

    // ── model_down : une panne qui dit « CE modèle ne sert pas » ──────────────────────

    #[test]
    fn model_down_lit_ce_que_le_transport_enonce() {
        let temporaire = |ms, raison: &str| {
            Some(ModelDown {
                kind: Unavailability::Temporary { cooldown_ms: ms },
                reason: raison.to_string(),
            })
        };
        // Un modèle que le moteur ne connaît pas : complet.
        assert_eq!(
            Failure::http(404).model_down(),
            Some(ModelDown {
                kind: Unavailability::Complete,
                reason: "http 404".to_string()
            })
        );
        assert_eq!(
            Failure::http(410).model_down().map(|d| d.kind),
            Some(Unavailability::Complete)
        );
        // Un serveur en erreur, un délai : passagers.
        for status in [500, 502, 503, 504, 529] {
            assert_eq!(
                Failure::http(status).model_down(),
                temporaire(TEMPORARY_UNAVAILABLE_MS, &format!("http {status}"))
            );
        }
        assert_eq!(
            Failure::new(Code::Timeout, "silence").model_down(),
            temporaire(TEMPORARY_UNAVAILABLE_MS, "timeout")
        );
        // Un quota de MODÈLE : passager, plus long.
        assert_eq!(
            Failure::quota_exhausted(QuotaScope::Model, None).model_down(),
            temporaire(QUOTA_UNAVAILABLE_MS, "quota")
        );
    }

    #[test]
    fn model_down_ne_dit_rien_de_ce_qui_n_est_pas_la_faute_du_modele() {
        // Le compte entier, la clé, la requête, une limite de débit nue, un moteur injoignable,
        // une réponse coupée ou illisible, un arrêt demandé : aucun autre modèle ne ferait mieux
        // (ou rien ne dit que c'est le modèle).
        let rien = [
            Failure::quota_exhausted(QuotaScope::Account, None),
            Failure::http(401),
            Failure::http(403),
            Failure::http(400),
            Failure::http(422),
            Failure::http(429),
            Failure::new(Code::Unreachable, "dns"),
            Failure::new(Code::Truncated, "flux coupé"),
            Failure::new(Code::Unreadable, "illisible"),
            Failure::new(Code::Aborted, "arrêt"),
            Failure::new(Code::Provider, "refus"),
        ];
        for failure in rien {
            assert_eq!(failure.model_down(), None, "{failure:?}");
        }
    }
}

#[cfg(test)]
mod served_matches_tests {
    use super::served_matches;

    #[test]
    fn le_meme_nom_ou_un_instantane_numerique_du_nom_demande() {
        assert!(served_matches("m2", "m2"));
        assert!(served_matches("gpt-4o", "gpt-4o-2024-08-06"));
        assert!(served_matches("deepseek-v3", "deepseek-v3-0324"));
        assert!(served_matches(
            "claude-sonnet-4-5",
            "claude-sonnet-4-5-20250929"
        ));
        assert!(served_matches("modele", "modele:2"));
        assert!(served_matches("modele", "modele@1.2.3"));
    }

    #[test]
    fn un_autre_modele_ou_un_suffixe_non_numerique_ne_passe_pas() {
        assert!(!served_matches("gpt-4o", "gpt-4o-mini"), "un autre modèle");
        assert!(
            !served_matches("qwen", "qwen:7b"),
            "une taille, pas un instantané"
        );
        assert!(!served_matches("llama", "llama3"), "sans séparateur");
        assert!(!served_matches("m2", "m1"));
        assert!(!served_matches("m2", "autre-modele"));
    }

    #[test]
    fn un_alias_servi_pour_un_instantane_demande_est_un_ecart() {
        assert!(!served_matches("gpt-4o-2024-08-06", "gpt-4o"));
    }

    #[test]
    fn les_bords_vides_ou_mal_formes_ne_prouvent_rien() {
        assert!(!served_matches("", ""));
        assert!(!served_matches("m2", ""));
        assert!(!served_matches("", "m2"));
        assert!(!served_matches("m2", "m2-"), "suffixe vide");
        assert!(
            !served_matches("m2", "m2-.1"),
            "doit commencer par un chiffre"
        );
        assert!(
            !served_matches("m2", "m2-2024-08-06x"),
            "lettre dans le suffixe"
        );
    }
}
