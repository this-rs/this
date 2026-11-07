//! # `this-harness` — la boucle d'agent, branchable
//!
//! Un agent n'est pas un `for` dans un gestionnaire de clic. C'est un **run** :
//! un objectif, des tours, des étapes observées, une raison d'arrêt nommée —
//! et, pendant qu'il tourne, une interface qui continue de vivre.
//!
//! Ce crate tient la boucle. Il ne tient ni le réseau, ni les outils, ni le
//! stockage : trois traits ([`Transport`], [`Toolbox`]) et une sortie
//! d'événements, que l'application fournit. C'est ce qui permet au même crate
//! de faire tourner la conversation dans une page wasm et dans une coquille
//! native, sans deuxième implémentation qui divergera.
//!
//! ## Les deux moitiés de la bidirectionnalité
//!
//! Une discussion est bidirectionnelle quand deux choses sont vraies :
//!
//! ```text
//! [l'agent travaille] --écrit un message--> [file d'attente] --tour suivant--> [l'agent en tient compte]
//! [l'agent travaille] --interrupt--------> [le run s'arrête]  --et le dit----> [Stopped { Cancelled }]
//! ```
//!
//! La première est [`Handle::enqueue`] : écrire pendant un run ne bloque rien,
//! le message part à la frontière du tour. La seconde est
//! [`Handle::interrupt`] : l'arrêt est un état, pas un espoir — le transport le
//! lit pour couper sa connexion, la boucle le lit entre deux outils.
//!
//! ## Ce que la boucle garantit
//!
//! - Un tour qui ne dit rien est relancé, **avec le motif** : un modèle relancé
//!   sans motif répète la même action.
//! - Un refus n'est pas une fin ; un silence n'est pas une fin ; un budget
//!   épuisé est une fin, et elle se dit.
//! - Une réponse tronquée (`finish_reason = length`) n'est pas une conclusion.
//! - Une action qui n'a rien changé à l'état n'est pas un progrès : le
//!   [`Toolbox`] rend une empreinte avant/après, et la boucle la compare.
//! - Tout arrêt passe par un événement [`Event::Stopped`] : l'interface ne
//!   devine jamais pourquoi la bulle s'est arrêtée.
//!
//! ## Exemple
//!
//! ```ignore
//! let harness = Harness::new(brief, Limits::default(), transport, toolbox, move |event| {
//!     match event {
//!         Event::Delta { text } => live.update(|d| d.push_str(&text)),
//!         Event::Stopped { stop, .. } => busy.set(false),
//!         _ => {}
//!     }
//! });
//! // L'interface garde la poignée, pas le harnais.
//! let handle = harness.handle();
//! handle.enqueue("ajoute une alerte de rupture");   // pendant un run : tenu
//! handle.interrupt();                               // ou arrêté, et dit
//! ```

mod event;
mod ports;
mod session;
mod wire;

pub use event::{Event, Outcome, Pending, Snapshot};
pub use ports::{Cancel, Delta, Toolbox, Toolboxes, Transport};
pub use session::{Handle, Harness, Limits, Running, Shared};
pub use wire::{
    Code, Failure, Finish, ImagePart, ImageSource, Message, Reply, Request, ToolCall, ToolSpec,
};

/// Les décisions pures de la boucle, réexportées : un utilisateur du harnais a
/// besoin de `Stop` et de `Step` pour lire ce que le harnais raconte.
pub use this_agent::{Decision, DefaultPolicy, Goal, Policy, Run, Step, Stop};

/// L'heure du target qui exécute vraiment le code.
///
/// Une page a `Date.now`, une coquille a l'horloge système. Demander l'une à
/// l'autre est une erreur de compilation — d'où deux implémentations et pas
/// une abstraction de plus.
#[cfg(target_arch = "wasm32")]
pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duree| duree.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}
