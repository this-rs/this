use crate::MessageType;

/// Tailles maximales, en octets du message **entier** tel qu'il arrive sur le fil.
///
/// Ces messages portent des opérations de liste et des propositions d'agent, pas des
/// fichiers : les plafonds sont volontairement petits. Un message plus gros est
/// refusé avant tout décodage, charge comprise. Seuls les appels de service
/// (`service.*`) montent à 1 Mio : ce plafond est aussi le plafond global
/// ([`Limits::ceiling`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// `appairage.sas` : quelques champs (code court, clé publique).
    pub appairage_sas: usize,
    /// `grant.demande` : un module, des droits demandés.
    pub grant_demande: usize,
    /// `grant.reponse` : accord ou refus.
    pub grant_reponse: usize,
    /// `espace.op` : une opération sur une liste d'un espace partagé.
    pub espace_op: usize,
    /// `espace.sync` : un lot d'opérations de rattrapage.
    pub espace_sync: usize,
    /// `agent.propose` : une proposition d'agent.
    pub agent_propose: usize,
    /// `agent.repond` : la réponse à une proposition.
    pub agent_repond: usize,
    /// `service.appel` : un appel de service (entrées d'un moteur, d'un outil).
    pub service_appel: usize,
    /// `service.reponse` : une réponse de service envoyée comme message.
    pub service_reponse: usize,
    /// Mémoire d'idempotence : nombre d'identifiants retenus (les plus anciens sortent).
    pub seen_ids: usize,
    /// Taille du journal des messages ignorés.
    pub journal: usize,
    /// Fraîcheur : écart maximal, en millisecondes, entre l'`horloge` d'un message
    /// (millisecondes depuis l'epoch, comme l'application la produit) et l'heure
    /// locale, **dans un sens comme dans l'autre**. Au-delà, la porte refuse
    /// ([`crate::Raison::Illisible`], texte « horloge hors de la fenêtre de
    /// fraîcheur »).
    ///
    /// `None` (par défaut) : aucun contrôle, l'ancien comportement — l'anti-rejeu
    /// ne repose alors que sur la mémoire [`Limits::seen_ids`], perdue au
    /// redémarrage. Avec `Some(f)`, un message capturé ne redevient admissible
    /// ni après un redémarrage ni après `seen_ids` messages plus récents **une
    /// fois sa fenêtre passée** : il n'est accepté que tant que l'heure locale
    /// est dans `[horloge − f, horloge + f]`, une fenêtre de `2·f`.
    ///
    /// Pour que la fenêtre soit couverte, `seen_ids` doit retenir tous les
    /// messages admis en `2·f`. Recommandation pour l'application : `f` = 10 min
    /// (`Some(600_000)`), soit une fenêtre de 20 min ; avec `seen_ids` = 4096
    /// (défaut), cela couvre jusqu'à ~3,4 messages admis par seconde en moyenne.
    /// Les horloges des appareils doivent s'accorder à mieux que `f` près.
    pub fraicheur_max_ms: Option<u64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            appairage_sas: 2 * 1024,
            grant_demande: 4 * 1024,
            grant_reponse: 4 * 1024,
            espace_op: 64 * 1024,
            espace_sync: 256 * 1024,
            agent_propose: 32 * 1024,
            agent_repond: 64 * 1024,
            service_appel: 1024 * 1024,
            service_reponse: 1024 * 1024,
            seen_ids: 4096,
            journal: 256,
            fraicheur_max_ms: None,
        }
    }
}

impl Limits {
    /// Plafond d'un type donné.
    pub fn max_for(&self, t: MessageType) -> usize {
        match t {
            MessageType::AppairageSas => self.appairage_sas,
            MessageType::GrantDemande => self.grant_demande,
            MessageType::GrantReponse => self.grant_reponse,
            MessageType::EspaceOp => self.espace_op,
            MessageType::EspaceSync => self.espace_sync,
            MessageType::AgentPropose => self.agent_propose,
            MessageType::AgentRepond => self.agent_repond,
            MessageType::ServiceAppel => self.service_appel,
            MessageType::ServiceReponse => self.service_reponse,
        }
    }

    /// Plus grand plafond : borne appliquée avant même de connaître le type.
    pub fn ceiling(&self) -> usize {
        [
            self.appairage_sas,
            self.grant_demande,
            self.grant_reponse,
            self.espace_op,
            self.espace_sync,
            self.agent_propose,
            self.agent_repond,
            self.service_appel,
            self.service_reponse,
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
    }
}
