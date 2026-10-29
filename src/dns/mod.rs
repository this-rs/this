//! **Délégation DNS** : ce que l'application déclare, ce que le serveur
//! autoritaire doit publier, et l'écart entre les deux.
//!
//! # Pourquoi un *plan* avant toute écriture
//!
//! Un framework capable d'écrire dans une zone DNS est un framework capable de
//! **couper un domaine**. La règle est donc la même que pour la délégation à
//! Caddy : on **décrit** d'abord — [`plan`] rend la liste exacte des créations,
//! modifications et suppressions — et on n'applique que sur demande explicite.
//!
//! Le plan est une **fonction pure** : il se teste sans réseau, et il se relit.
//!
//! # Ce que le framework s'autorise à toucher
//!
//! Uniquement, et par construction :
//!
//! * les enregistrements **A**, **AAAA** et **CNAME** — jamais MX, TXT, SRV, NS :
//!   une application ne doit pas pouvoir effacer la messagerie d'un domaine en
//!   réconciliant « ses » enregistrements ;
//! * les noms qui font partie des **sites déclarés** ;
//! * et la **suppression** seulement si elle est demandée (`prune`), jamais par
//!   défaut : un enregistrement absent de la déclaration est *signalé*, pas
//!   effacé.

use serde::{Deserialize, Serialize};

pub mod powerdns;

pub use powerdns::PdnsConfig;

#[cfg(feature = "powerdns")]
pub use powerdns::{Applied, PdnsClient};

/// Un enregistrement DNS tel que le framework le décrit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRecord {
    /// Nom qualifié, **avec** le point final (`app.exemple.fr.`).
    pub name: String,
    /// Type d'enregistrement (`A`, `AAAA`, `CNAME`).
    pub record_type: String,
    /// Contenu (adresse, ou cible d'un CNAME — avec son point final).
    pub content: String,
    /// Durée de vie.
    pub ttl: u32,
}

impl DnsRecord {
    /// L'identité d'un enregistrement : son **nom et son type**. Le contenu est
    /// la valeur, pas l'identité — c'est ce qui permet de dire « modifié »
    /// plutôt que « supprimé puis recréé ».
    pub fn key(&self) -> (String, String) {
        (
            self.name.to_ascii_lowercase(),
            self.record_type.to_ascii_uppercase(),
        )
    }
}

/// Les types que le framework accepte de gérer.
pub const MANAGED_TYPES: [&str; 3] = ["A", "AAAA", "CNAME"];

/// Le type est-il géré par le framework ?
pub fn is_managed_type(record_type: &str) -> bool {
    MANAGED_TYPES
        .iter()
        .any(|t| t.eq_ignore_ascii_case(record_type))
}

/// **Le plan** : ce qu'il faudrait changer pour que le serveur autoritaire
/// rejoigne la déclaration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsPlan {
    /// La zone concernée.
    pub zone: String,
    /// Enregistrements à créer.
    pub creates: Vec<DnsRecord>,
    /// Enregistrements dont le contenu change.
    pub updates: Vec<DnsRecord>,
    /// Enregistrements à supprimer (seulement si `prune`).
    pub deletes: Vec<DnsRecord>,
    /// Enregistrements hors périmètre du framework (MX, TXT…) : **jamais
    /// touchés**, toujours signalés.
    pub untouched: Vec<DnsRecord>,
    /// Déjà conformes.
    pub unchanged: usize,
}

impl DnsPlan {
    /// Le plan est-il vide ?
    pub fn is_empty(&self) -> bool {
        self.creates.is_empty() && self.updates.is_empty() && self.deletes.is_empty()
    }

    /// Combien de changements seraient appliqués.
    pub fn change_count(&self) -> usize {
        self.creates.len() + self.updates.len() + self.deletes.len()
    }

    /// Une phrase lisible — pour un journal ou une revue.
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return format!(
                "zone {} : à jour ({} enregistrement(s) déjà conformes)",
                self.zone, self.unchanged
            );
        }
        format!(
            "zone {} : {} création(s), {} modification(s), {} suppression(s), {} inchangé(s)",
            self.zone,
            self.creates.len(),
            self.updates.len(),
            self.deletes.len(),
            self.unchanged
        )
    }
}

/// **Compare la déclaration à l'état** et rend le plan.
///
/// * `desired` : ce que l'application déclare (dérivé de ses sites) ;
/// * `current` : ce que le serveur autoritaire publie **dans cette zone** ;
/// * `prune` : supprimer les enregistrements gérés qui ne sont plus déclarés ?
///   `false` par défaut — un enregistrement oublié est **signalé**, pas effacé.
pub fn plan(zone: &str, desired: &[DnsRecord], current: &[DnsRecord], prune: bool) -> DnsPlan {
    let mut result = DnsPlan {
        zone: zone.to_string(),
        ..Default::default()
    };

    // **Tout est comparé en forme pleinement qualifiée** (avec le point final).
    // Une version antérieure comparait un nom qualifié (`app.exemple.fr.`) à un
    // suffixe qui ne l'était pas (`.exemple.fr`) : rien ne correspondait, et la
    // zone entière était classée « hors périmètre » — donc jamais réconciliée.
    // Ce sont les tests de ce module qui l'ont dit.
    let zone_fqdn = ensure_dot(zone).to_ascii_lowercase();
    let zone_suffix = format!(".{zone_fqdn}");

    for record in current {
        // Hors périmètre : type non géré, ou nom hors de la zone. Signalé, et
        // jamais touché — c'est ce qui empêche un réconciliateur d'effacer la
        // messagerie d'un domaine.
        let in_zone = {
            let name = record.name.to_ascii_lowercase();
            name == zone_fqdn || name.ends_with(&zone_suffix)
        };
        if !is_managed_type(&record.record_type) || !in_zone {
            result.untouched.push(record.clone());
            continue;
        }

        match desired.iter().find(|d| d.key() == record.key()) {
            Some(wanted) if wanted.content == record.content => result.unchanged += 1,
            Some(wanted) => {
                let mut updated = wanted.clone();
                updated.ttl = wanted.ttl;
                result.updates.push(updated);
            }
            None => {
                if prune {
                    result.deletes.push(record.clone());
                } else {
                    result.untouched.push(record.clone());
                }
            }
        }
    }

    for wanted in desired {
        if !current.iter().any(|c| c.key() == wanted.key()) {
            result.creates.push(wanted.clone());
        }
    }

    result
}

/// **Les enregistrements que l'application déclare**, à partir de ses sites.
///
/// Chaque hôte servi reçoit **un** enregistrement vers la cible : une adresse
/// (`A`/`AAAA`) ou un nom (`CNAME`).
pub fn desired_records(
    hosts: &[String],
    record_type: &str,
    target: &str,
    ttl: u32,
) -> Vec<DnsRecord> {
    let content = match record_type.to_ascii_uppercase().as_str() {
        "CNAME" => ensure_dot(target),
        _ => target.trim().to_string(),
    };
    hosts
        .iter()
        .map(|host| DnsRecord {
            name: ensure_dot(host),
            record_type: record_type.to_ascii_uppercase(),
            content: content.clone(),
            ttl,
        })
        .collect()
}

/// `exemple.fr` → `exemple.fr.` (un nom DNS qualifié finit par un point).
pub fn ensure_dot(name: &str) -> String {
    let name = name.trim();
    if name.ends_with('.') {
        name.to_string()
    } else {
        format!("{name}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, kind: &str, content: &str) -> DnsRecord {
        DnsRecord {
            name: ensure_dot(name),
            record_type: kind.to_string(),
            content: content.to_string(),
            ttl: 300,
        }
    }

    /// **Rien à faire quand tout est conforme** — un réconciliateur qui « change »
    /// ce qui est déjà bon est un réconciliateur qu'on n'ose pas lancer.
    #[test]
    fn a_converged_zone_produces_an_empty_plan() {
        let desired = vec![record("app.exemple.fr", "A", "203.0.113.10")];
        let current = desired.clone();
        let dns_plan = plan("exemple.fr", &desired, &current, true);

        assert!(dns_plan.is_empty(), "{dns_plan:?}");
        assert_eq!(dns_plan.unchanged, 1);
        assert!(
            dns_plan.summary().contains("à jour"),
            "{}",
            dns_plan.summary()
        );
    }

    /// Une création, une modification — et la modification est **dite** telle,
    /// pas déguisée en suppression/création.
    #[test]
    fn a_changed_content_is_an_update_not_a_replacement() {
        let desired = vec![
            record("app.exemple.fr", "A", "203.0.113.10"),
            record("new.exemple.fr", "A", "203.0.113.11"),
        ];
        let current = vec![record("app.exemple.fr", "A", "203.0.113.99")];

        let dns_plan = plan("exemple.fr", &desired, &current, true);
        assert_eq!(dns_plan.updates.len(), 1, "{dns_plan:?}");
        assert_eq!(dns_plan.updates[0].content, "203.0.113.10");
        assert_eq!(dns_plan.creates.len(), 1);
        assert!(dns_plan.deletes.is_empty());
        assert_eq!(dns_plan.change_count(), 2);
    }

    /// **Les types non gérés ne sont JAMAIS touchés**, et la suppression n'a pas
    /// lieu sans `prune` : une application ne doit pas pouvoir effacer la
    /// messagerie d'un domaine en réconciliant ses sites.
    #[test]
    fn records_outside_the_perimeter_are_never_touched() {
        let desired = vec![record("app.exemple.fr", "A", "203.0.113.10")];
        let current = vec![
            record("app.exemple.fr", "A", "203.0.113.10"),
            // Hors périmètre par le type.
            record("exemple.fr", "MX", "10 mail.exemple.fr."),
            record("exemple.fr", "TXT", "v=spf1 -all"),
            // Dans la zone, type géré, mais plus déclaré : signalé, pas effacé.
            record("vieux.exemple.fr", "A", "203.0.113.5"),
            // Hors zone : jamais touché, même si le type est géré.
            record("autre-zone.fr", "A", "198.51.100.1"),
        ];

        let dns_plan = plan("exemple.fr", &desired, &current, false);
        assert!(
            dns_plan.deletes.is_empty(),
            "sans `prune`, rien n'est supprimé"
        );
        assert_eq!(dns_plan.untouched.len(), 4, "{:?}", dns_plan.untouched);
        assert!(
            dns_plan
                .untouched
                .iter()
                .any(|r| r.record_type == "MX" && r.content.contains("mail")),
            "l'enregistrement de messagerie doit être signalé intact"
        );

        // Avec `prune`, seul l'enregistrement GÉRÉ et dans la ZONE est supprimé.
        let dns_plan = plan("exemple.fr", &desired, &current, true);
        assert_eq!(dns_plan.deletes.len(), 1, "{dns_plan:?}");
        assert_eq!(dns_plan.deletes[0].name, "vieux.exemple.fr.");
        assert!(
            dns_plan.untouched.iter().any(|r| r.record_type == "MX"),
            "même avec `prune`, un MX n'est pas supprimé"
        );
        assert!(
            dns_plan
                .untouched
                .iter()
                .any(|r| r.name == "autre-zone.fr."),
            "et un nom hors zone non plus"
        );
    }

    /// La déclaration dérive des **sites** : c'est ce qui relie la configuration
    /// d'exposition au DNS, sans seconde liste à tenir.
    #[test]
    fn the_desired_records_come_from_the_declared_sites() {
        let hosts = vec![
            "www.exemple.fr".to_string(),
            "explorer.exemple.fr".to_string(),
        ];
        let records = desired_records(&hosts, "A", "203.0.113.10", 300);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "www.exemple.fr.");
        assert!(records.iter().all(|r| r.record_type == "A"));

        // Un CNAME porte un nom qualifié, avec son point.
        let cnames = desired_records(&hosts, "CNAME", "edge.exemple.fr", 300);
        assert_eq!(cnames[0].content, "edge.exemple.fr.");
    }

    /// La casse et le point final ne créent pas de faux changements.
    #[test]
    fn case_and_trailing_dot_do_not_create_phantom_changes() {
        let desired = vec![record("App.Exemple.FR", "A", "203.0.113.10")];
        let current = vec![DnsRecord {
            name: "app.exemple.fr.".to_string(),
            record_type: "a".to_string(),
            content: "203.0.113.10".to_string(),
            ttl: 60,
        }];
        let dns_plan = plan("exemple.fr", &desired, &current, true);
        assert!(dns_plan.is_empty(), "aucun changement réel : {dns_plan:?}");
    }

    #[test]
    fn only_address_and_alias_types_are_managed() {
        for managed in ["A", "a", "AAAA", "CNAME", "cname"] {
            assert!(is_managed_type(managed), "{managed}");
        }
        for other in ["MX", "TXT", "NS", "SRV", "SOA", "PTR"] {
            assert!(!is_managed_type(other), "{other} ne doit pas être géré");
        }
    }
}
