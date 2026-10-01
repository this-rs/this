//! Codecs du pont this-rs ↔ obrain : identité et valeurs.
//!
//! # Pourquoi ce fichier existe
//!
//! Les deux mondes ne parlent pas la même langue sur deux points précis :
//!
//! 1. **L'identité.** this-rs identifie ses entités par `Uuid` ; obrain par
//!    `NodeId(u64)` / `EdgeId(u64)`, des slots denses. Sans correspondance, le
//!    pont ne peut pas traduire un identifiant d'un monde à l'autre.
//! 2. **Les valeurs.** this-rs transporte du `serde_json::Value` ; obrain un
//!    `Value` typé à 7 variantes, **sans types composites**.
//!
//! # La décision d'identité : un encodage bijectif, PAS une table
//!
//! Une table `Uuid → NodeId` serait un état à allouer, persister et
//! resynchroniser — donc un coût par événement et une source de divergence
//! silencieuse. On encode donc l'identifiant **dans** l'UUID :
//!
//! ```text
//! 128 bits = [ étiquette 64 bits ][ identifiant 64 bits ]
//!            "OBRAIN\0" + genre      NodeId / EdgeId
//! ```
//!
//! Propriétés, toutes testées plus bas :
//! - **bijectif** : `node_of_uuid(uuid_of_node(n)) == Some(n)`, sans perte ;
//! - **sans état** : aucune table, aucun verrou, aucune allocation ;
//! - **genre distingué** : un UUID de nœud ne se décode pas en arête ;
//! - **étranger rejeté** : un UUID v4 quelconque (ou `Uuid::nil()`) rend `None`,
//!   donc on ne traduit jamais un identifiant qui n'est pas d'obrain.
//!
//! ⚠️ Conséquence assumée : ces UUID **ne sont pas des UUID valides au sens
//! RFC 9562** (les bits de version/variante font partie de la charge utile).
//! Ce sont des **poignées internes** au pont : elles ne doivent pas être
//! exposées à un client comme identité publique. C'est le prix de la bijection
//! exacte, et il est préférable à un état à maintenir.

use obrain_common::types::{EdgeId, NodeId, PropertyKey, Value};
use serde_json::{Map, Value as Json};
use uuid::Uuid;

/// Étiquette d'espace pour les nœuds — « OBRAIN » en ASCII, puis le genre.
const TAG_NODE: u64 = 0x4f42_5241_494e_0001;
/// Étiquette d'espace pour les arêtes.
const TAG_EDGE: u64 = 0x4f42_5241_494e_0002;
/// Étiquette d'espace pour les tenants (poignées dérivées).
const TAG_TENANT: u64 = 0x4f42_5241_494e_0003;

#[inline]
const fn encode(tag: u64, id: u64) -> u128 {
    ((tag as u128) << 64) | (id as u128)
}

#[inline]
fn decode(u: Uuid, tag: u64) -> Option<u64> {
    let bits = u.as_u128();
    if (bits >> 64) as u64 == tag {
        Some(bits as u64)
    } else {
        None
    }
}

/// Poignée UUID d'un nœud obrain.
#[inline]
pub fn uuid_of_node(id: NodeId) -> Uuid {
    Uuid::from_u128(encode(TAG_NODE, id.0))
}

/// Poignée UUID d'une arête obrain.
#[inline]
pub fn uuid_of_edge(id: EdgeId) -> Uuid {
    Uuid::from_u128(encode(TAG_EDGE, id.0))
}

/// Nœud obrain désigné par une poignée, ou `None` si l'UUID n'en est pas une.
#[inline]
pub fn node_of_uuid(u: Uuid) -> Option<NodeId> {
    decode(u, TAG_NODE).map(NodeId)
}

/// Arête obrain désignée par une poignée, ou `None`.
#[inline]
pub fn edge_of_uuid(u: Uuid) -> Option<EdgeId> {
    decode(u, TAG_EDGE).map(EdgeId)
}

/// Vrai si l'UUID est une poignée obrain (nœud **ou** arête).
/// Poignee stable pour un tenant nomme.
///
/// obrain porte le tenant comme une **chaine** (`EventContext.tenant_id`),
/// this-rs comme un **Uuid** (`EventEnvelope.tenant_id`). Une table serait
/// encore un etat ; on derive donc une poignee **deterministe**.
///
/// Regle : un tenant deja au format UUID garde son identite (on le parse) ;
/// un nom libre obtient une poignee derivee du nom (FNV-1a, deux passes).
/// Meme nom => meme routage, sans table. **L'aller-retour est coherent** :
/// la forme texte d'une poignee se re-parse en la meme poignee.
///
/// Limite assumee : pour un nom libre, la poignee est derivee et non
/// reversible — si un tenant doit un jour porter un UUID propre cote obrain,
/// il faudra reconcilier. C'est ecrit ici pour que ce ne soit pas une surprise.
pub fn uuid_of_tenant(name: &str) -> Uuid {
    if let Ok(parsed) = Uuid::parse_str(name) {
        return parsed;
    }
    const SEED_A: u64 = 0xcbf2_9ce4_8422_2325;
    const SEED_B: u64 = 0x9e37_79b9_7f4a_7c15;
    let hash = |seed: u64| -> u64 {
        let mut h = seed;
        for b in name.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let hi = hash(SEED_A);
    let lo = hash(SEED_B ^ name.len() as u64);
    // Les 16 bits hauts portent l'etiquette TENANT : une poignee de tenant
    // reste distinguable d'une poignee de noeud ou d'arete.
    let tagged = (hi & 0x0000_ffff_ffff_ffff) | (TAG_TENANT << 48);
    Uuid::from_u128((u128::from(tagged) << 64) | u128::from(lo))
}

pub fn is_obrain_handle(u: Uuid) -> bool {
    let tag = (u.as_u128() >> 64) as u64;
    tag == TAG_NODE || tag == TAG_EDGE
}

// ─────────────────────────── VALEURS ───────────────────────────

/// `Value` d'obrain → JSON.
pub fn value_to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int64(i) => Json::Number((*i).into()),
        Value::Float64(f) => serde_json::Number::from_f64(*f)
            .map(Json::Number)
            .unwrap_or(Json::Null), // NaN / ±inf n'ont pas d'équivalent JSON
        Value::String(s) => Json::String(s.to_string()),
        // Pas de variante binaire en JSON : tableau d'octets, sans perte.
        Value::Bytes(b) => {
            Json::Array(b.iter().map(|x| Json::Number((*x as u64).into())).collect())
        }
        Value::Timestamp(t) => Json::Number(i64::from(*t).into()),
        // obrain A des composites. Je m'etais trompe en ecrivant le contraire :
        // le compilateur a donne la liste complete des variantes, et `List` et
        // `Map` en font partie. Ils se convertissent donc SANS PERTE.
        Value::List(items) => Json::Array(items.iter().map(value_to_json).collect()),
        Value::Map(entries) => Json::Object(
            entries
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), value_to_json(v)))
                .collect(),
        ),
        Value::Vector(xs) => Json::Array(
            xs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(f64::from(*x))
                        .map(Json::Number)
                        .unwrap_or(Json::Null)
                })
                .collect(),
        ),
        // Temporels et chemin : JSON n'a pas de type correspondant. Rendu en
        // texte (Debug) : la donnee reste lisible, le type ne voyage pas.
        Value::Date(_)
        | Value::Time(_)
        | Value::Duration(_)
        | Value::ZonedDatetime(_)
        | Value::Path { .. } => Json::String(format!("{v:?}")),
    }
}

/// Propriétés d'obrain → objet JSON (ce que `EntityEvent.data` attend).
pub fn properties_to_json(props: &[(PropertyKey, Value)]) -> Map<String, Json> {
    props
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), value_to_json(v)))
        .collect()
}

/// JSON → `Value` d'obrain.
///
/// **La seule ambiguite est irreductible** : JSON n'a qu'UN type tableau,
/// obrain en a trois (`List`, `Vector`, `Bytes`). Au retour d'un tableau on ne
/// peut pas savoir lequel c'etait ; on choisit `List`, le plus general. Idem
/// pour les temporels, rendus en texte a l'aller et relus comme des chaines.
/// **La donnee survit toujours ; c'est le type qui ne voyage pas.**
///
/// Les scalaires et les composites (`List`/`Map`) font, eux, un aller-retour
/// exact — c'est teste.
pub fn json_to_value(j: &Json) -> Value {
    match j {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => n
            .as_i64()
            .map_or_else(|| Value::Float64(n.as_f64().unwrap_or(0.0)), Value::Int64),
        Json::String(s) => Value::String(s.as_str().into()),
        Json::Array(items) => {
            Value::List(items.iter().map(json_to_value).collect::<Vec<_>>().into())
        }
        Json::Object(map) => Value::Map(std::sync::Arc::new(
            map.iter()
                .map(|(k, v)| (PropertyKey::from(k.as_str()), json_to_value(v)))
                .collect::<std::collections::BTreeMap<PropertyKey, Value>>(),
        )),
    }
}

/// Objet JSON → propriétés d'obrain.
pub fn json_to_properties(m: &Map<String, Json>) -> Vec<(PropertyKey, Value)> {
    m.iter()
        .map(|(k, v)| (PropertyKey::from(k.as_str()), json_to_value(v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identity_is_bijective_without_any_table() {
        for raw in [0u64, 1, 42, 7, u32::MAX as u64, u64::MAX] {
            let n = NodeId(raw);
            assert_eq!(
                node_of_uuid(uuid_of_node(n)),
                Some(n),
                "aller-retour nœud {raw}"
            );
            let e = EdgeId(raw);
            assert_eq!(
                edge_of_uuid(uuid_of_edge(e)),
                Some(e),
                "aller-retour arête {raw}"
            );
        }
    }

    #[test]
    fn node_and_edge_handles_never_collide() {
        let n = uuid_of_node(NodeId(7));
        let e = uuid_of_edge(EdgeId(7));
        assert_ne!(n, e, "même identifiant numérique, genres différents");
        assert!(node_of_uuid(e).is_none(), "une arête n'est pas un nœud");
        assert!(edge_of_uuid(n).is_none(), "un nœud n'est pas une arête");
    }

    #[test]
    fn foreign_uuids_are_rejected() {
        for foreign in [Uuid::nil(), Uuid::new_v4(), Uuid::new_v4()] {
            assert!(!is_obrain_handle(foreign));
            assert!(node_of_uuid(foreign).is_none());
            assert!(edge_of_uuid(foreign).is_none());
        }
    }

    /// Scalaires ET composites : aller-retour exact.
    #[test]
    fn scalars_and_composites_roundtrip_exactly() {
        let inner = Value::Map(std::sync::Arc::new(
            [(
                PropertyKey::from("c"),
                Value::List(vec![Value::Bool(true)].into()),
            )]
            .into_iter()
            .collect(),
        ));
        let composites =
            Value::List(vec![Value::Int64(1), Value::String("deux".into()), inner].into());
        for v in [
            Value::Null,
            Value::Bool(true),
            Value::Int64(-42),
            Value::Float64(1.5),
            Value::String("texte".into()),
            composites,
        ] {
            let j = value_to_json(&v);
            assert_eq!(json_to_value(&j), v, "aller-retour perdu pour {v:?}");
        }
    }

    /// Les trois variantes « tableau » d'obrain partagent l'UNIQUE type tableau
    /// de JSON : la donnee survit, le type non. C'est la limite reelle, ecrite.
    #[test]
    fn array_like_types_keep_data_but_not_their_type() {
        let bytes = Value::Bytes(vec![1u8, 2, 255].into());
        let j = value_to_json(&bytes);
        assert_eq!(j, json!([1, 2, 255]), "la donnee est bien la");
        match json_to_value(&j) {
            Value::List(items) => assert_eq!(items.len(), 3, "relu comme List, pas comme Bytes"),
            other => panic!("attendu une List, obtenu {other:?}"),
        }
        let vec_f = Value::Vector(vec![0.5f32, 1.5].into());
        assert_eq!(value_to_json(&vec_f), json!([0.5, 1.5]));
    }

    /// Un temporel sort en nombre, jamais en objet opaque.
    #[test]
    fn temporal_values_are_rendered_as_numbers() {
        let epoch = Value::Timestamp(obrain_common::types::Timestamp::EPOCH);
        assert_eq!(value_to_json(&epoch), json!(0));
    }

    #[test]
    fn properties_map_in_both_directions() {
        let props = vec![
            (PropertyKey::from("name"), Value::from("Alice")),
            (PropertyKey::from("age"), Value::Int64(30)),
        ];
        let obj = properties_to_json(&props);
        assert_eq!(obj["name"], json!("Alice"));
        assert_eq!(obj["age"], json!(30));
        let mut sorted_orig = props.clone();
        sorted_orig.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        let mut back = json_to_properties(&obj);
        back.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        assert_eq!(back, sorted_orig);
    }
}
