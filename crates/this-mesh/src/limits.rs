use crate::MessageType;

/// Tailles maximales, en octets du message **entier** tel qu'il arrive sur le fil.
///
/// Ces messages portent des opérations de liste et des propositions d'agent, pas des
/// fichiers : les plafonds sont volontairement petits. Un message plus gros est
/// refusé avant tout décodage, charge comprise.
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
    /// Mémoire d'idempotence : nombre d'identifiants retenus (les plus anciens sortent).
    pub seen_ids: usize,
    /// Taille du journal des messages ignorés.
    pub journal: usize,
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
            seen_ids: 4096,
            journal: 256,
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
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
    }
}
