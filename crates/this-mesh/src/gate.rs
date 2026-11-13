use crate::envelope::{MESH_VERSION, Wire, signing_bytes, verify};
use crate::{DeviceId, Limits, Message, MessageType, Peers};
use std::collections::{HashSet, VecDeque};
use this_vfs::layout::AnchorKeyState;
use uuid::Uuid;

/// Pourquoi un message est refusé. Une raison par cause, jamais un « non » générique.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Raison {
    #[error("message trop gros : {taille} octets > {max}")]
    TropGros { taille: usize, max: usize },
    #[error("enveloppe illisible : {0}")]
    Illisible(String),
    #[error("version d'enveloppe non prise en charge : {0}")]
    VersionInconnue(u32),
    #[error("`de` ({de}) ne correspond pas à l'expéditeur authentifié ({expediteur})")]
    DeNeCorrespondPas { de: String, expediteur: String },
    #[error("clé inconnue : {0}")]
    CleInconnue(String),
    #[error("clé révoquée : {0}")]
    CleRevoquee(String),
    #[error("signature invalide")]
    SignatureInvalide,
    #[error("charge non canonique (nombre non entier)")]
    ChargeNonCanonique,
}

/// Verdict de la porte.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Authentique, nouveau : à traiter. La charge est la forme canonique vérifiée.
    Admis(Message),
    /// Déjà vu (même `id`) : acquitté, **pas rejoué**.
    DejaVu(Uuid),
    /// Type hors énumération : ignoré et journalisé, charge non interprétée.
    Ignore(String),
    Refuse(Raison),
}

/// Trace d'un message ignoré.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub de: DeviceId,
    pub type_recu: String,
    pub taille: usize,
}

/// La porte : refus par défaut.
///
/// L'`expéditeur` est l'identité que le canal a authentifiée (ce que le transport,
/// quel qu'il soit, affirme) ; le message doit dire la même chose dans `de`, et sa
/// signature doit valoir pour la clé enregistrée sous cet identifiant.
#[derive(Debug)]
pub struct Gate {
    peers: Peers,
    limits: Limits,
    seen: HashSet<Uuid>,
    order: VecDeque<Uuid>,
    journal: VecDeque<JournalEntry>,
    charges_lues: usize,
}

impl Gate {
    pub fn new(peers: Peers, limits: Limits) -> Self {
        Self {
            peers,
            limits,
            seen: HashSet::new(),
            order: VecDeque::new(),
            journal: VecDeque::new(),
            charges_lues: 0,
        }
    }

    pub fn peers(&self) -> &Peers {
        &self.peers
    }

    /// Accès mutable (appairage, révocation : effet immédiat sur le prochain message).
    pub fn peers_mut(&mut self) -> &mut Peers {
        &mut self.peers
    }

    pub fn journal(&self) -> impl Iterator<Item = &JournalEntry> {
        self.journal.iter()
    }

    /// Nombre de fois où une charge a été décodée (pour vérifier qu'un message
    /// refusé ou ignoré n'a pas été lu).
    pub fn charges_lues(&self) -> usize {
        self.charges_lues
    }

    /// Ordre : taille → enveloppe → `de` = expéditeur → clé connue/révoquée → type
    /// connu → signature → rejeu. L'idempotence ne s'applique qu'à un message
    /// authentique : sinon un inconnu pourrait « brûler » un `id`.
    pub fn admettre(&mut self, expediteur: &DeviceId, octets: &[u8]) -> Verdict {
        // 1. Taille, avant tout décodage : plafond global, puis plafond du type.
        let ceiling = self.limits.ceiling();
        if octets.len() > ceiling {
            return Verdict::Refuse(Raison::TropGros {
                taille: octets.len(),
                max: ceiling,
            });
        }
        // 2. Enveloppe ; la charge reste du JSON brut, non décodé.
        let wire: Wire = match serde_json::from_slice(octets) {
            Ok(w) => w,
            Err(e) => return Verdict::Refuse(Raison::Illisible(e.to_string())),
        };
        if wire.v != MESH_VERSION {
            return Verdict::Refuse(Raison::VersionInconnue(wire.v));
        }
        // 3. L'identité annoncée est celle du canal.
        if &wire.de != expediteur {
            return Verdict::Refuse(Raison::DeNeCorrespondPas {
                de: wire.de.to_string(),
                expediteur: expediteur.to_string(),
            });
        }
        // 4. Refus par défaut : inconnue et révoquée sont distinguées.
        let Some(peer) = self.peers.get(&wire.de) else {
            return Verdict::Refuse(Raison::CleInconnue(wire.de.to_string()));
        };
        if peer.state != AnchorKeyState::Active {
            return Verdict::Refuse(Raison::CleRevoquee(wire.de.to_string()));
        }
        let key = peer.key;
        // 5. Type énuméré : sinon ignoré et journalisé, charge jamais lue.
        let Some(kind) = MessageType::from_wire(&wire.kind) else {
            if self.journal.len() >= self.limits.journal {
                self.journal.pop_front();
            }
            self.journal.push_back(JournalEntry {
                de: wire.de.clone(),
                type_recu: wire.kind.chars().take(64).collect(),
                taille: octets.len(),
            });
            return Verdict::Ignore(wire.kind);
        };
        let max = self.limits.max_for(kind);
        if octets.len() > max {
            return Verdict::Refuse(Raison::TropGros {
                taille: octets.len(),
                max,
            });
        }
        // 6. Signature : c'est ici seulement que la charge est décodée.
        self.charges_lues += 1;
        let charge: serde_json::Value = match serde_json::from_str(wire.charge.get()) {
            Ok(c) => c,
            Err(e) => return Verdict::Refuse(Raison::Illisible(e.to_string())),
        };
        let bytes = match signing_bytes(
            wire.v,
            &wire.id,
            &wire.de,
            &wire.a,
            &wire.kind,
            wire.horloge,
            &charge,
        ) {
            Ok(b) => b,
            Err(_) => return Verdict::Refuse(Raison::ChargeNonCanonique),
        };
        if !verify(&key, &bytes, &wire.signature) {
            return Verdict::Refuse(Raison::SignatureInvalide);
        }
        // 7. Rejeu : acquitté une seule fois.
        if self.seen.contains(&wire.id) {
            return Verdict::DejaVu(wire.id);
        }
        if self.order.len() >= self.limits.seen_ids
            && let Some(old) = self.order.pop_front()
        {
            self.seen.remove(&old);
        }
        self.seen.insert(wire.id);
        self.order.push_back(wire.id);
        Verdict::Admis(Message {
            v: wire.v,
            id: wire.id,
            de: wire.de,
            a: wire.a,
            kind,
            horloge: wire.horloge,
            charge,
            signature: wire.signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ed25519_dalek::SigningKey;
    use serde_json::json;
    use this_vfs::layout::AnchorKeyRole;

    fn dev(s: &str) -> DeviceId {
        DeviceId::parse(s).unwrap()
    }
    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }
    fn setup() -> (Gate, SigningKey, DeviceId, DeviceId) {
        let (alice, bob) = (dev("device-aaaaaaaa"), dev("device-bbbbbbbb"));
        let k = key(1);
        let peers = Peers::new();
        peers.admit(&alice, k.verifying_key(), AnchorKeyRole::Shared, Utc::now());
        (Gate::new(peers, Limits::default()), k, alice, bob)
    }
    fn msg(k: &SigningKey, de: &DeviceId, a: &DeviceId, id: u128, kind: MessageType) -> Message {
        Message::signed(
            k,
            Uuid::from_u128(id),
            de.clone(),
            a.clone(),
            kind,
            1,
            json!({"liste":"courses","op":"add","n":3}),
        )
        .unwrap()
    }

    #[test]
    fn a_signed_by_active_key_is_admitted() {
        let (mut g, k, alice, bob) = setup();
        let m = msg(&k, &alice, &bob, 1, MessageType::EspaceOp);
        assert_eq!(g.admettre(&alice, &m.to_bytes()), Verdict::Admis(m));
    }

    #[test]
    fn b_resigned_under_another_de_is_refused() {
        let (mut g, k, alice, bob) = setup();
        // bob (clé connue, mais l'attaquant n'a que la signature d'alice) : on change `de`.
        let peers = g.peers().clone();
        peers.admit(
            &bob,
            key(2).verifying_key(),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        *g.peers_mut() = peers;
        let m = msg(&k, &alice, &bob, 1, MessageType::EspaceOp);
        let mut forged = m.clone();
        forged.de = bob.clone();
        // Annoncé par bob : la signature d'alice ne couvre pas `de = bob`.
        assert_eq!(
            g.admettre(&bob, &forged.to_bytes()),
            Verdict::Refuse(Raison::SignatureInvalide)
        );
        // Annoncé par alice sur le canal d'alice : `de` ne correspond pas.
        assert!(matches!(
            g.admettre(&alice, &forged.to_bytes()),
            Verdict::Refuse(Raison::DeNeCorrespondPas { .. })
        ));
        // Changer `a` casse aussi la signature.
        let mut redirected = m;
        redirected.a = dev("device-cccccccc");
        assert_eq!(
            g.admettre(&alice, &redirected.to_bytes()),
            Verdict::Refuse(Raison::SignatureInvalide)
        );
    }

    #[test]
    fn c_revoked_differs_from_unknown() {
        let (mut g, k, alice, bob) = setup();
        let m = msg(&k, &alice, &bob, 1, MessageType::EspaceOp);
        assert!(g.peers_mut().revoke(&alice));
        assert_eq!(
            g.admettre(&alice, &m.to_bytes()),
            Verdict::Refuse(Raison::CleRevoquee("device-aaaaaaaa".into()))
        );
        let stranger = dev("device-dddddddd");
        let m2 = msg(&key(9), &stranger, &bob, 2, MessageType::EspaceOp);
        assert_eq!(
            g.admettre(&stranger, &m2.to_bytes()),
            Verdict::Refuse(Raison::CleInconnue("device-dddddddd".into()))
        );
    }

    #[test]
    fn d_unknown_type_is_ignored_and_payload_unread() {
        let (mut g, k, alice, bob) = setup();
        let m = msg(&k, &alice, &bob, 1, MessageType::EspaceOp);
        let mut v: serde_json::Value = serde_json::from_slice(&m.to_bytes()).unwrap();
        v["type"] = json!("shell.exec");
        v["charge"] = json!({"cmd": 1.5}); // un flottant : la lecture échouerait si on la tentait
        let verdict = g.admettre(&alice, &serde_json::to_vec(&v).unwrap());
        assert_eq!(verdict, Verdict::Ignore("shell.exec".into()));
        assert_eq!(g.charges_lues(), 0);
        let j: Vec<_> = g.journal().collect();
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].type_recu, "shell.exec");
    }

    #[test]
    fn e_oversized_is_refused_before_reading_payload() {
        let (mut g, k, alice, bob) = setup();
        // Plus grand que le plafond global : même pas du JSON, jamais décodé.
        let junk = vec![b'x'; Limits::default().ceiling() + 1];
        assert!(matches!(
            g.admettre(&alice, &junk),
            Verdict::Refuse(Raison::TropGros { .. })
        ));
        // Plus grand que le plafond de SON type (appairage.sas = 2 KiB), signature valide.
        let big = Message::signed(
            &k,
            Uuid::from_u128(5),
            alice.clone(),
            bob,
            MessageType::AppairageSas,
            1,
            json!({"x": "y".repeat(3000)}),
        )
        .unwrap();
        assert!(matches!(
            g.admettre(&alice, &big.to_bytes()),
            Verdict::Refuse(Raison::TropGros { max: 2048, .. })
        ));
        assert_eq!(g.charges_lues(), 0);
    }

    #[test]
    fn f_replay_is_acknowledged_once() {
        let (mut g, k, alice, bob) = setup();
        let m = msg(&k, &alice, &bob, 7, MessageType::AgentPropose);
        let bytes = m.to_bytes();
        assert!(matches!(g.admettre(&alice, &bytes), Verdict::Admis(_)));
        assert_eq!(
            g.admettre(&alice, &bytes),
            Verdict::DejaVu(Uuid::from_u128(7))
        );
        assert_eq!(
            g.admettre(&alice, &bytes),
            Verdict::DejaVu(Uuid::from_u128(7))
        );
    }

    #[test]
    fn signature_ignores_field_order_and_formatting() {
        let (mut g, k, alice, bob) = setup();
        let m = msg(&k, &alice, &bob, 3, MessageType::EspaceOp);
        // Même message, champs dans un autre ordre, charge indentée à l'envers.
        let reordered = format!(
            "{{ \"signature\": \"{}\", \"charge\": {{ \"op\": \"add\", \"n\": 3, \"liste\": \"courses\" }}, \"type\": \"espace.op\", \"horloge\": 1, \"a\": \"{}\", \"de\": \"{}\", \"id\": \"{}\", \"v\": 1 }}",
            m.signature, bob, alice, m.id
        );
        assert!(matches!(
            g.admettre(&alice, reordered.as_bytes()),
            Verdict::Admis(_)
        ));
    }

    #[test]
    fn tampered_payload_is_refused() {
        let (mut g, k, alice, bob) = setup();
        let mut m = msg(&k, &alice, &bob, 4, MessageType::EspaceOp);
        m.charge = json!({"liste":"courses","op":"del","n":3});
        assert_eq!(
            g.admettre(&alice, &m.to_bytes()),
            Verdict::Refuse(Raison::SignatureInvalide)
        );
    }

    #[test]
    fn registry_roundtrips_and_keeps_revocation() {
        let (mut g, _k, alice, _bob) = setup();
        g.peers_mut().admit(
            &dev("device-eeeeeeee"),
            key(5).verifying_key(),
            AnchorKeyRole::Ownership,
            Utc::now(),
        );
        g.peers_mut().revoke(&alice);
        let back = Peers::from_json(&g.peers().to_json()).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back.state(&alice), Some(AnchorKeyState::Revoked));
        assert_eq!(
            back.role(&dev("device-eeeeeeee")),
            Some(AnchorKeyRole::Ownership)
        );
    }

    #[test]
    fn service_appel_a_son_plafond_d_un_mio() {
        let l = Limits::default();
        assert_eq!(l.max_for(MessageType::ServiceAppel), 1024 * 1024);
        assert_eq!(l.max_for(MessageType::ServiceReponse), 1024 * 1024);
        assert_eq!(l.ceiling(), 1024 * 1024);
        let (mut g, k, alice, bob) = setup();
        let charge = json!({"x": "y".repeat(512 * 1024)});
        let gros = |kind, id| {
            Message::signed(
                &k,
                Uuid::from_u128(id),
                alice.clone(),
                bob.clone(),
                kind,
                1,
                charge.clone(),
            )
            .unwrap()
        };
        // 512 Kio : admis en `service.appel`, refusé en `espace.op` (64 Kio).
        let appel = gros(MessageType::ServiceAppel, 1);
        assert_eq!(g.admettre(&alice, &appel.to_bytes()), Verdict::Admis(appel));
        assert!(matches!(
            g.admettre(&alice, &gros(MessageType::EspaceOp, 2).to_bytes()),
            Verdict::Refuse(Raison::TropGros { .. })
        ));
    }

    #[test]
    fn device_id_shape() {
        assert!(DeviceId::parse("device-0123abcd").is_some());
        for bad in [
            "device-0123ABCD",
            "device-123",
            "dev-0123abcd",
            "device-0123abcdef",
        ] {
            assert!(DeviceId::parse(bad).is_none(), "{bad}");
        }
    }
}
