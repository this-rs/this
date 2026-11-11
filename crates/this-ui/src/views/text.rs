//! Every user-visible string of the view renderer, in one place.
//!
//! There is no i18n machinery in `this-ui` (the descriptor carries message keys the host
//! resolves); these are the fallbacks the renderer needs to say something rather than
//! nothing. Isolating them here is what makes a later translation a single-file change.

/// Shown where a value is missing: never an empty hole.
pub const DASH: &str = "—";
pub const YES: &str = "oui";
pub const NO: &str = "non";

/// Generic empty state.
pub const NOTHING: &str = "Rien à afficher.";
/// Shown while the host fetches.
pub const LOADING: &str = "Chargement…";
/// Retry action on a failed load.
pub const RETRY: &str = "Réessayer";
/// Title of a permission refusal.
pub const DENIED: &str = "Accès refusé";
/// Action offered on a refusal.
pub const ASK_ACCESS: &str = "Demander l'accès";
/// Shown when the host never provided the entity a brick reads.
pub const UNKNOWN_ENTITY: &str = "entité inconnue de l'hôte";
/// Prefix of the readable fallback drawn where a brick is not rendered.
pub const UNRENDERED: &str = "Non rendu";
/// Close button label.
pub const CLOSE: &str = "Fermer";
/// Confirm / cancel on a destructive action.
pub const CONFIRM: &str = "Confirmer";
pub const CANCEL: &str = "Annuler";
/// Previous / next page.
pub const PREVIOUS: &str = "Précédent";
pub const NEXT: &str = "Suivant";
/// Search box label on a filterable list.
pub const SEARCH: &str = "Rechercher";
/// Composer of a chat brick.
pub const SEND: &str = "Envoyer";
pub const COMPOSER: &str = "Votre message";
/// Queue and program captions.
pub const QUEUE_EMPTY: &str = "File vide.";
pub const RUN: &str = "Lancer";
pub const RUN_AUTO: &str = "Exécution automatique à chaque changement";
pub const PROGRAM_RESULT: &str = "Résultat fourni par l'hôte";
/// Relation brick.
pub const NO_LINK: &str = "Aucune relation.";
pub const LINK: &str = "Lier";
pub const UNLINK: &str = "Retirer";
pub const RELATION_UNJOINED: &str = "relation non jointe par l'hôte";
/// Capability request brick.
pub const ALLOW: &str = "Autoriser";
pub const DENY: &str = "Refuser";
/// Read-only field.
pub const READ_ONLY: &str = "lecture seule";
/// A `reference` input that names no target entity.
pub const INCOMPLETE_REFERENCE: &str =
    "Déclaration incomplète : ce champ `reference` ne nomme pas son entité cible.";
/// File field.
pub const NO_FILE: &str = "Aucun fichier";
/// Online / offline indicator.
pub const ONLINE: &str = "En ligne";
pub const OFFLINE: &str = "Hors ligne";
/// Chart brick with no row to plot.
pub const CHART_NO_ROW: &str = "Aucun point à tracer : aucune ligne.";
/// Chart brick whose rows hold no number in the plotted field.
pub const CHART_NO_NUMBER: &str = "Aucun point à tracer : le champ ne porte aucun nombre.";

/// Metrics brick with no rows.
pub const ZERO: &str = "0";
/// Thread window: the button that widens it towards older messages.
pub const OLDER: &str = "Voir les anciens";
pub const OLDER_HIDDEN: &str = "au-dessus";
/// The question card the agent poses inside a thread.
pub const QUESTION: &str = "Question";
pub const ANSWER: &str = "Répondre";
pub const ANSWERED: &str = "répondu";
pub const ANSWER_FREE: &str = "Autre réponse…";
/// A tool step, when the row carries one but says nothing about it.
pub const STEP: &str = "outil";
/// A group of tool steps one of which has not returned yet.
pub const STEP_RUNNING: &str = "en cours";
