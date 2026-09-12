//! Journal **ordonné et durable** sur la zone journal d'obrain.
//!
//! # Pourquoi ce module existe
//!
//! Le trait [`crate::events::log::EventLog`] décrit exactement ce qu'il faut à
//! une réplication — `append` qui rend un numéro de séquence **monotone**,
//! `subscribe(from)` qui reprend à une position, `ack`/`seek` par consommateur —
//! mais sa **seule** implémentation était `InMemoryEventLog` : un journal qui
//! disparaît au redémarrage ne réplique rien.
//!
//! Obrain fournit déjà le support : une **zone journal** par canal
//! (`substrate.journal.<canal>`), en ajout seul, où chaque ligne porte un `seq`
//! explicite, avec récupération intégrée après corruption (le moteur
//! resynchronise sur la prochaine trame valide au lieu d'amputer la queue).
//! Ce module est l'adaptateur — **aucune modification d'obrain**.
//!
//! # La leçon qu'il encode une fois pour toutes
//!
//! Le moteur rend les lignes dans l'**ordre physique** de la zone, qui peut
//! dévier de l'ordre logique. Le harness l'a appris à ses dépens : une session
//! de 515 k événements dont la tête était `[0,1,2,4,5,6,14,7,8,…]`, et un resume
//! qui échouait (« seq 7 attendu à l'index 15 »). Tout consommateur devait donc
//! re-trier et dédupliquer **avant** de réparer les trous.
//!
//! [`ObrainJournalLog::read_from`](crate::events::log_obrain::ObrainJournalLog::read_from) fait ce travail **ici**, et le fait toujours :
//! tri stable par `seq`, déduplication par `seq` (le premier gagne), et
//! indication de troncature quand la borne de lecture mord. Aucun consommateur
//! n'a plus à réapprendre la leçon.
//!
//! # Ce que ce module n'est pas
//!
//! Ce n'est pas le transport : il ne sait pas *expédier* le journal vers un
//! autre nœud. C'est la moitié locale d'une réplication — la partie ordonnée,
//! idempotente et reprenable. Le transport reste à écrire (P3-suite).

use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::core::events::EventEnvelope;
use crate::events::log::EventLog;
use crate::events::types::{SeekPosition, SeqNo};
use crate::storage::ObrainStore;

/// Schéma d'une entrée de journal.
pub const JOURNAL_SCHEMA: &str = "this-rs/journal/1";

/// Suffixe du canal où sont consignées les positions de consommateurs.
pub const ACK_CHANNEL_SUFFIX: &str = ".acks";

/// Borne par défaut d'une lecture — au-delà, la lecture est dite tronquée.
pub const DEFAULT_READ_LIMIT: usize = 1 << 20;

/// Une entrée de journal : une trame autour d'une charge utile.
///
/// Le `seq` est **au premier niveau** du JSON : c'est ce que le moteur lit pour
/// filtrer par plage (`event_seq`), et c'est ce qui rend la position adressable
/// sans décoder la charge utile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Numéro de séquence, **strictement croissant** dans un canal.
    pub seq: u64,
    /// Nature de l'entrée (`event`, `ack`…).
    pub kind: String,
    /// Horodatage d'écriture (ms depuis l'époque Unix).
    pub at_ms: i64,
    /// La charge utile.
    pub payload: serde_json::Value,
}

impl JournalEntry {
    /// Sérialise l'entrée en une ligne JSON.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Relit une ligne, ou `None` si elle n'est pas une entrée exploitable.
    ///
    /// **Sans `seq` numérique, la ligne est rejetée** : accepter une entrée non
    /// ordonnançable reviendrait à répliquer du désordre.
    pub fn from_line(line: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let seq = v.get("seq")?.as_u64()?;
        Some(Self {
            seq,
            kind: v
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or("event")
                .to_string(),
            at_ms: v.get("at_ms").and_then(|a| a.as_i64()).unwrap_or(0),
            payload: v.get("payload").cloned().unwrap_or(serde_json::Value::Null),
        })
    }

    /// Empaquette une enveloppe d'événement.
    ///
    /// L'enveloppe est sérialisée **avec** son `seq_no` : une ligne répliquée
    /// porte donc sa propre position, et un rejeu ne peut pas la renégocier.
    pub fn from_envelope(envelope: &EventEnvelope) -> Self {
        Self {
            seq: envelope.seq_no.unwrap_or(0),
            kind: "event".to_string(),
            at_ms: envelope.timestamp.timestamp_millis(),
            payload: serde_json::to_value(envelope).unwrap_or(serde_json::Value::Null),
        }
    }

    /// Dépaquette une enveloppe d'événement, si c'en est une.
    pub fn to_envelope(&self) -> Option<EventEnvelope> {
        if self.kind != "event" {
            return None;
        }
        serde_json::from_value(self.payload.clone()).ok()
    }

    /// L'entrée consigne-t-elle la position d'un consommateur ?
    pub fn ack_of(&self) -> Option<(String, u64)> {
        if self.kind != "ack" {
            return None;
        }
        let consumer = self.payload.get("consumer")?.as_str()?.to_string();
        let seq = self.payload.get("seq")?.as_u64()?;
        Some((consumer, seq))
    }
}

/// Une page de journal.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalPage {
    /// Les entrées, **re-triées par `seq` et dédupliquées**.
    pub entries: Vec<JournalEntry>,
    /// La borne de lecture a-t-elle mordu ? (publié : une page tronquée n'est
    /// jamais présentée comme complète)
    pub truncated: bool,
}

struct JournalInner {
    store: Arc<ObrainStore>,
    channel: String,
    /// Sérialise les appends : deux écritures concurrentes se marcheraient
    /// dessus sur l'attribution des `seq`.
    append_lock: std::sync::Mutex<()>,
}

/// Le journal d'un canal, adossé à une zone journal d'obrain.
#[derive(Clone)]
pub struct ObrainJournalLog {
    inner: Arc<JournalInner>,
}

impl ObrainJournalLog {
    /// Ouvre (ou crée) le journal d'un canal.
    ///
    /// Le nom du canal devient un nom de zone : il est **validé** (alphanumérique,
    /// tiret, souligné, point) plutôt que laissé passer — un nom invalide
    /// produirait une zone au nom imprévisible.
    pub fn open(store: Arc<ObrainStore>, channel: impl Into<String>) -> Result<Self> {
        let channel = channel.into();
        if channel.is_empty() {
            return Err(anyhow!("canal de journal vide"));
        }
        if !channel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(anyhow!(
                "canal de journal invalide : {channel:?} (alphanumérique, `-`, `_`, `.` attendus)"
            ));
        }
        Ok(Self {
            inner: Arc::new(JournalInner {
                store,
                channel,
                append_lock: std::sync::Mutex::new(()),
            }),
        })
    }

    /// Le canal.
    pub fn channel(&self) -> &str {
        &self.inner.channel
    }

    fn substrate(&self) -> Result<Arc<obrain_substrate::SubstrateStore>> {
        self.inner
            .store
            .db()
            .substrate_handle()
            .ok_or_else(|| anyhow!("journal : store non adossé au substrat"))
    }

    /// Écrit un lot d'entrées, en leur **attribuant** leurs `seq`.
    ///
    /// # Attribution et idempotence — deux cas, une règle
    ///
    /// - `seq == 0` : **attribue-moi une place**. L'entrée reçoit `tête + 1`.
    ///   C'est le cas d'un événement neuf.
    /// - `seq > 0` : **position imposée**. Si elle est déjà présente (ou
    ///   périmée), l'entrée est **ignorée**. C'est le cas d'une **réplication** :
    ///   la position fait autorité depuis la source, et un lot rejoué ne
    ///   duplique rien.
    ///
    /// La distinction compte : sans elle, on ne saurait pas si `seq = 0` veut
    /// dire « en tête » ou « jamais écrit », et un rejeu écraserait le journal
    /// au lieu de l'étendre.
    ///
    /// Rend la tête du journal après écriture.
    pub fn append_entries(&self, entries: &mut [JournalEntry]) -> Result<u64> {
        let _guard = self
            .inner
            .append_lock
            .lock()
            .map_err(|_| anyhow!("journal : verrou d'append empoisonné"))?;

        let head = self.last_seq()?.unwrap_or(0);
        let mut next = head;
        let mut lines: Vec<String> = Vec::new();
        let mut written: Vec<u64> = Vec::new();

        for entry in entries.iter_mut() {
            if entry.seq != 0 && entry.seq <= head {
                // Position imposée et déjà prise (ou périmée) : on n'écrit pas.
                continue;
            }
            next += 1;
            entry.seq = next;
            if entry.at_ms == 0 {
                entry.at_ms = crate::storage::writer_lease::now_ms();
            }
            lines.push(entry.to_line());
            written.push(next);
        }

        if lines.is_empty() {
            return Ok(head);
        }

        let sub = self.substrate()?;
        // `write_session_journal` remplace la zone ; `append_session_journal`
        // ajoute. On ajoute — un journal ne se réécrit pas.
        if head == 0 {
            obrain_engine::journal_zone::write_session_journal(&sub, &self.inner.channel, &lines)
                .map_err(|e| anyhow!("journal : écriture du canal {} : {e}", self.inner.channel))?;
        } else {
            obrain_engine::journal_zone::append_session_journal(&sub, &self.inner.channel, &lines)
                .map_err(|e| anyhow!("journal : ajout au canal {} : {e}", self.inner.channel))?;
        }
        Ok(next)
    }

    /// Écrit **une** entrée et rend son `seq`.
    pub fn append_entry(&self, mut entry: JournalEntry) -> Result<u64> {
        let mut v = vec![entry.clone()];
        let head = self.append_entries(&mut v)?;
        entry.seq = v[0].seq;
        let _ = entry;
        Ok(head)
    }

    /// Lit les entrées **après** `after_seq`, re-triées et dédupliquées.
    ///
    /// C'est ici que la leçon du harness est appliquée : le moteur rend l'ordre
    /// **physique**, ce module rend l'ordre **logique**. Un consommateur qui
    /// reconstruit un flux contigu n'a plus à s'en soucier.
    pub fn read_from(&self, after_seq: u64, max_entries: usize) -> Result<JournalPage> {
        let sub = self.substrate()?;
        let limit = if max_entries == 0 {
            DEFAULT_READ_LIMIT
        } else {
            max_entries
        };
        let raw = obrain_engine::journal_zone::read_session_journal(
            &sub,
            &self.inner.channel,
            after_seq as i64,
            i64::MAX,
            limit,
        );

        let mut entries: Vec<JournalEntry> = raw
            .iter()
            .filter_map(|l| JournalEntry::from_line(l))
            .collect();
        let read = entries.len();

        // Tri **stable** par seq : à seq égal, l'ordre physique d'origine décide,
        // ce qui rend le résultat reproductible.
        entries.sort_by_key(|e| e.seq);
        entries.retain(|e| e.seq > after_seq);
        // Déduplication par seq : le premier gagne (règle explicite, pas un
        // « au hasard » dépendant du tri).
        entries.dedup_by_key(|e| e.seq);

        Ok(JournalPage {
            truncated: read >= limit,
            entries,
        })
    }

    /// Toutes les entrées du canal.
    pub fn read_all(&self) -> Result<Vec<JournalEntry>> {
        Ok(self.read_from(0, DEFAULT_READ_LIMIT)?.entries)
    }

    /// La tête du journal, ou `None` s'il est vide.
    pub fn last_seq(&self) -> Result<Option<u64>> {
        let page = self.read_from(0, DEFAULT_READ_LIMIT)?;
        Ok(page.entries.last().map(|e| e.seq))
    }

    /// Nombre d'entrées.
    pub fn len(&self) -> Result<usize> {
        Ok(self.read_all()?.len())
    }

    /// Le journal est-il vide ?
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// La position consignée d'un consommateur.
    pub fn consumer_position(&self, consumer: &str) -> Result<Option<u64>> {
        let acks = ObrainJournalLog::open(
            Arc::clone(&self.inner.store),
            format!("{}{}", self.inner.channel, ACK_CHANNEL_SUFFIX),
        )?;
        let mut position = None;
        for entry in acks.read_all()? {
            if let Some((name, seq)) = entry.ack_of()
                && name == consumer
            {
                position = Some(seq); // la dernière consigne gagne
            }
        }
        Ok(position)
    }

    /// Consigne la position d'un consommateur.
    ///
    /// Les accusés vivent dans **le même mécanisme** que le journal (un canal
    /// dédié) : pas de fichier parallèle, pas de format à maintenir en plus.
    pub fn ack_consumer(&self, consumer: &str, seq: u64) -> Result<u64> {
        let acks = ObrainJournalLog::open(
            Arc::clone(&self.inner.store),
            format!("{}{}", self.inner.channel, ACK_CHANNEL_SUFFIX),
        )?;
        let mut batch = vec![JournalEntry {
            seq: 0,
            kind: "ack".to_string(),
            at_ms: crate::storage::writer_lease::now_ms(),
            payload: serde_json::json!({ "consumer": consumer, "seq": seq }),
        }];
        acks.append_entries(&mut batch)
    }

    /// Position de départ pour un consommateur, selon la demande.
    fn start_position(&self, consumer: &str, position: &SeekPosition) -> Result<u64> {
        Ok(match position {
            SeekPosition::Beginning => 0,
            SeekPosition::Latest => self.last_seq()?.unwrap_or(0),
            SeekPosition::Sequence(seq) => *seq,
            SeekPosition::LastAcknowledged => self.consumer_position(consumer)?.unwrap_or(0),
        })
    }
}

#[async_trait]
impl EventLog for ObrainJournalLog {
    async fn append(&self, mut envelope: EventEnvelope) -> Result<SeqNo> {
        let head = self.last_seq()?.unwrap_or(0);
        let seq = head + 1;
        envelope.seq_no = Some(seq);
        let mut entry = JournalEntry::from_envelope(&envelope);
        entry.seq = seq;
        let mut v = vec![entry];
        self.append_entries(&mut v)?;
        Ok(v[0].seq)
    }

    /// Flux infini : d'abord le stocké depuis la position, puis les nouveautés.
    ///
    /// Le suivi est un **sondage** (intervalle court). C'est honnête et suffisant
    /// pour un canal local ; la version poussée s'appuierait sur le bus
    /// post-commit d'obrain (`MutationBus`), qui sait notifier.
    async fn subscribe(
        &self,
        consumer: &str,
        position: SeekPosition,
    ) -> Result<std::pin::Pin<Box<dyn tokio_stream::Stream<Item = EventEnvelope> + Send>>> {
        let log = self.clone();
        let consumer = consumer.to_string();
        let cursor = log.start_position(&consumer, &position)?;
        let buffer: std::collections::VecDeque<EventEnvelope> = std::collections::VecDeque::new();

        let stream = futures::stream::unfold(
            (log, consumer, cursor, buffer),
            |(log, consumer, mut cursor, mut buffer)| async move {
                loop {
                    if let Some(envelope) = buffer.pop_front() {
                        return Some((envelope, (log, consumer, cursor, buffer)));
                    }
                    match log.read_from(cursor, 1024) {
                        Ok(page) if !page.entries.is_empty() => {
                            for entry in page.entries {
                                if let Some(envelope) = entry.to_envelope() {
                                    cursor = entry.seq;
                                    buffer.push_back(envelope);
                                }
                            }
                        }
                        Ok(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "journal : lecture échouée, on réessaie");
                            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        }
                    }
                }
            },
        );

        Ok(Box::pin(stream))
    }

    async fn ack(&self, consumer: &str, seq_no: SeqNo) -> Result<()> {
        self.ack_consumer(consumer, seq_no)?;
        Ok(())
    }

    async fn seek(&self, consumer: &str, position: SeekPosition) -> Result<()> {
        // Un `seek` remplace la consigne : on consigne la nouvelle position.
        let target = match position {
            SeekPosition::Beginning => 0,
            SeekPosition::Latest => self.last_seq()?.unwrap_or(0),
            SeekPosition::Sequence(seq) => seq,
            SeekPosition::LastAcknowledged => return Ok(()),
        };
        self.ack_consumer(consumer, target)?;
        Ok(())
    }

    async fn last_seq_no(&self) -> Option<SeqNo> {
        self.last_seq().ok().flatten()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn log(channel: &str) -> ObrainJournalLog {
        ObrainJournalLog::open(ObrainStore::new_in_memory(), channel).expect("ouverture")
    }

    fn entry(kind: &str, payload: &str) -> JournalEntry {
        JournalEntry {
            seq: 0,
            kind: kind.to_string(),
            at_ms: 1_700_000_000_000,
            payload: serde_json::json!({ "v": payload }),
        }
    }

    #[test]
    fn appends_get_monotone_sequences() {
        let j = log("chan-a");
        assert_eq!(j.last_seq().unwrap(), None);
        let mut batch = vec![entry("event", "a"), entry("event", "b")];
        let head = j.append_entries(&mut batch).unwrap();
        assert_eq!(head, 2);
        assert_eq!(batch[0].seq, 1);
        assert_eq!(batch[1].seq, 2);

        let mut more = vec![entry("event", "c")];
        assert_eq!(j.append_entries(&mut more).unwrap(), 3);
        assert_eq!(more[0].seq, 3, "la séquence reprend la tête");
    }

    #[test]
    fn replaying_a_batch_duplicates_nothing() {
        let j = log("chan-idem");
        let mut batch = vec![entry("event", "x"), entry("event", "y")];
        j.append_entries(&mut batch).unwrap();
        assert_eq!(j.len().unwrap(), 2);
        assert_eq!(batch[0].seq, 1);

        // Le lot est rejoué AVEC SES POSITIONS (le cas d'une réplication : la
        // source fait autorité) : rien ne doit être dupliqué.
        let mut again = batch.clone();
        let head = j.append_entries(&mut again).unwrap();
        assert_eq!(head, 2, "aucune nouvelle position consommée");
        assert_eq!(j.len().unwrap(), 2, "un rejeu ne duplique jamais");

        // Un lot NEUF (seq 0) reçoit au contraire de nouvelles positions.
        let mut fresh = vec![entry("event", "z")];
        assert_eq!(j.append_entries(&mut fresh).unwrap(), 3);
        assert_eq!(fresh[0].seq, 3);
        assert_eq!(j.len().unwrap(), 3);
    }

    /// La réplication : ce qu'on lit d'une source se réécrit tel quel ailleurs.
    #[test]
    fn entries_replicate_with_their_source_positions() {
        let source = log("chan-src");
        let mut batch: Vec<JournalEntry> = (0..4).map(|i| entry("event", &i.to_string())).collect();
        source.append_entries(&mut batch).unwrap();

        let replica = log("chan-dst");
        let mut carried = source.read_all().unwrap();
        replica.append_entries(&mut carried).unwrap();

        assert_eq!(
            replica.read_all().unwrap(),
            source.read_all().unwrap(),
            "le réplica porte les mêmes positions que la source"
        );

        // Réplication rejouée depuis le même curseur : aucun doublon.
        let mut again = source.read_all().unwrap();
        replica.append_entries(&mut again).unwrap();
        assert_eq!(replica.len().unwrap(), 4);
    }

    /// La leçon du harness, encodée : l'ordre physique peut dévier, l'ordre
    /// rendu est **logique**.
    #[test]
    fn read_from_returns_logical_order_even_when_physical_order_deviates() {
        let j = log("chan-order");
        // Écriture normale.
        let mut batch: Vec<JournalEntry> = (0..6).map(|i| entry("event", &i.to_string())).collect();
        j.append_entries(&mut batch).unwrap();

        // On réécrit la zone à l'envers, comme le moteur peut la rendre après
        // des appends concurrents ou une resynchronisation.
        let sub = j.substrate().unwrap();
        let all = j.read_all().unwrap();
        let mut reversed: Vec<String> = all.iter().map(|e| e.to_line()).collect();
        reversed.reverse();
        obrain_engine::journal_zone::write_session_journal(&sub, "chan-order", &reversed).unwrap();

        let page = j.read_from(0, 1024).unwrap();
        let seqs: Vec<u64> = page.entries.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6], "re-trié par seq");
        assert!(!page.truncated);

        // Et une lecture partielle part bien de la position demandée.
        let after = j.read_from(3, 1024).unwrap();
        assert_eq!(
            after.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
    }

    #[test]
    fn duplicates_in_the_zone_are_collapsed() {
        let j = log("chan-dup");
        let mut batch = vec![entry("event", "a"), entry("event", "b")];
        j.append_entries(&mut batch).unwrap();

        let sub = j.substrate().unwrap();
        let all = j.read_all().unwrap();
        let mut lines: Vec<String> = all.iter().map(|e| e.to_line()).collect();
        lines.push(lines[0].clone()); // doublon physique du seq 1
        obrain_engine::journal_zone::write_session_journal(&sub, "chan-dup", &lines).unwrap();

        let page = j.read_from(0, 1024).unwrap();
        assert_eq!(
            page.entries.len(),
            2,
            "un seq dupliqué n'apparaît qu'une fois"
        );
        assert_eq!(page.entries[0].seq, 1);
    }

    #[test]
    fn the_log_survives_a_store_reopen() {
        let dir = std::env::temp_dir().join(format!("this_journal_{}", uuid::Uuid::new_v4()));
        {
            let store = ObrainStore::open(&dir).unwrap();
            let j = ObrainJournalLog::open(store, "chan-durable").unwrap();
            let mut batch = vec![entry("event", "1"), entry("event", "2")];
            j.append_entries(&mut batch).unwrap();
        }
        // Réouverture : le journal est toujours là, et la tête n'a pas bougé.
        let store = ObrainStore::open(&dir).unwrap();
        let j = ObrainJournalLog::open(store, "chan-durable").unwrap();
        assert_eq!(j.last_seq().unwrap(), Some(2), "un journal durable survit");
        let mut next = vec![entry("event", "3")];
        assert_eq!(j.append_entries(&mut next).unwrap(), 3);
        assert_eq!(next[0].seq, 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn consumer_positions_are_persisted_in_their_own_channel() {
        let j = log("chan-ack");
        assert_eq!(j.consumer_position("replica-1").unwrap(), None);
        j.ack_consumer("replica-1", 5).unwrap();
        j.ack_consumer("replica-2", 2).unwrap();
        j.ack_consumer("replica-1", 9).unwrap();

        assert_eq!(
            j.consumer_position("replica-1").unwrap(),
            Some(9),
            "le dernier gagne"
        );
        assert_eq!(j.consumer_position("replica-2").unwrap(), Some(2));
        assert_eq!(j.consumer_position("inconnu").unwrap(), None);
        // Les accusés ne polluent PAS le journal d'événements.
        assert_eq!(j.len().unwrap(), 0);
    }

    #[test]
    fn opaque_lines_without_a_sequence_are_rejected() {
        // Accepter une ligne non ordonnançable reviendrait à répliquer du désordre.
        assert!(JournalEntry::from_line(r#"{"kind":"event"}"#).is_none());
        assert!(JournalEntry::from_line("pas du json").is_none());
        assert!(JournalEntry::from_line(r#"{"seq":"douze"}"#).is_none());
        let ok = JournalEntry::from_line(r#"{"seq":4,"kind":"event","at_ms":1,"payload":{}}"#)
            .expect("entrée valide");
        assert_eq!(ok.seq, 4);
    }

    #[test]
    fn an_invalid_channel_name_is_refused() {
        let store = ObrainStore::new_in_memory();
        assert!(ObrainJournalLog::open(Arc::clone(&store), "").is_err());
        assert!(ObrainJournalLog::open(Arc::clone(&store), "a/b").is_err());
        assert!(ObrainJournalLog::open(store, "chan-ok.1").is_ok());
    }

    fn event(n: usize) -> EventEnvelope {
        EventEnvelope::new(crate::core::events::FrameworkEvent::GdprErasure {
            tenant_id: uuid::Uuid::from_u128(n as u128),
            entities_deleted: n,
            links_deleted: 0,
            notifications_deleted: 0,
            erased_at: chrono::Utc::now(),
        })
    }

    #[tokio::test]
    async fn the_event_log_replays_then_tails() {
        use tokio_stream::StreamExt;

        let j = log("chan-log");
        // Deux événements AVANT l'abonnement.
        for i in 1..3 {
            j.append(event(i)).await.unwrap();
        }
        assert_eq!(j.last_seq_no().await, Some(2));

        // Rejeu depuis le début : les deux premiers arrivent.
        let mut stream = j
            .subscribe("replica", SeekPosition::Beginning)
            .await
            .unwrap();
        let first = stream.next().await.expect("premier");
        assert_eq!(first.seq_no, Some(1));
        let second = stream.next().await.expect("deuxième");
        assert_eq!(second.seq_no, Some(2));

        // Un nouvel événement arrive ensuite : le flux le rend.
        j.append(event(3)).await.unwrap();
        let third = stream.next().await.expect("troisième");
        assert_eq!(third.seq_no, Some(3));

        // L'accusé de réception est consigné et reprend la position.
        j.ack("replica", 2).await.unwrap();
        assert_eq!(j.consumer_position("replica").unwrap(), Some(2));
        let resumed = j
            .start_position("replica", &SeekPosition::LastAcknowledged)
            .unwrap();
        assert_eq!(resumed, 2);
    }
}
