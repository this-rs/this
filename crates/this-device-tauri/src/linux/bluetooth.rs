//! Bluetooth sur Linux : BlueZ (`org.bluez`) par son ObjectManager, sur le bus **système**.
//!
//! Ce que fait ce backend : dire l'état de la radio (`bluetoothState`), l'état de la
//! permission, et faire **un balayage borné** (`bluetoothScan`). Il ne connecte pas : les
//! méthodes `bluetoothConnect`, `bluetoothRead`, etc. sont refusées `not_supported` par
//! [`super::call`], parce qu'un appareil se désigne ici par son adresse MAC (BlueZ n'a pas
//! l'UUID de périphérique de CoreBluetooth) et que la garde de [`crate::families::bluetooth`]
//! exige cette forme UUID. Mieux vaut le dire que laisser croire à une connexion.
//!
//! ## Jamais de panique, jamais de blocage
//!
//! - Pas de bus système (conteneur, session sans D-Bus) : `Err(NotSupported)` tout de suite,
//!   et `unsupported` côté permission. Rien n'est tenté ensuite.
//! - Chaque étape (ouverture du bus, `GetManagedObjects`, `StartDiscovery`, `StopDiscovery`)
//!   est bornée par [`BUS_STEP_TIMEOUT`] ; l'attente du balayage est la durée demandée, déjà
//!   clamée par [`crate::wire::clamp_duration`]. Le total est donc fini et connu d'avance :
//!   `4 × 3 s + durée` au pire.
//! - La découverte démarrée est toujours arrêtée, même si la lecture finale échoue.
//!
//! Un appareil est « vu » si BlueZ lui donne un RSSI : un appareil en cache, hors de portée,
//! n'en a pas, et n'entre pas dans le résultat.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use serde_json::Value;
use this_platform::device_bridge::{BtDevice, DeviceError, PermissionState, Reply};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as Variant};
use zbus::{Connection, Proxy};

use super::{BUS_STEP_TIMEOUT, bounded, error_of, system_bus};
use crate::families::bluetooth::{BluetoothReply, RadioState};

const SERVICE: &str = "org.bluez";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";

/// L'adaptateur BlueZ, tel que l'ObjectManager le montre.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawAdapter {
    pub path: String,
    pub powered: bool,
}

/// Un appareil BlueZ, réduit à ce que le contrat en dit.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RawDevice {
    pub address: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    pub uuids: Vec<String>,
}

/// Ce que l'ObjectManager contient d'utile.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub adapters: Vec<RawAdapter>,
    pub devices: Vec<RawDevice>,
}

/// Un UUID de service sous sa forme 128 bits minuscule : `180d` et `0000180D` deviennent
/// `0000180d-0000-1000-8000-00805f9b34fb`. BlueZ les rend toujours en 128 bits.
pub fn expand_uuid(uuid: &str) -> String {
    let lower = uuid.to_ascii_lowercase();
    match lower.len() {
        4 => format!("0000{lower}-0000-1000-8000-00805f9b34fb"),
        8 => format!("{lower}-0000-1000-8000-00805f9b34fb"),
        _ => lower,
    }
}

/// Les appareils vus pendant le balayage, dans la forme du contrat.
///
/// - sans RSSI : pas vu (cache de BlueZ) ;
/// - avec un filtre de services : au moins un des UUID annoncés doit y être ;
/// - une adresse n'apparaît qu'une fois (le plus fort RSSI gagne) ;
/// - tri par RSSI décroissant, puis par id : une sortie déterministe.
///
/// L'`id` est l'adresse MAC en majuscules : c'est la seule identité que BlueZ donne.
pub fn seen_devices(devices: &[RawDevice], services: &[String]) -> Vec<BtDevice> {
    let wanted: Vec<String> = services.iter().map(|s| expand_uuid(s)).collect();
    let mut best: BTreeMap<String, BtDevice> = BTreeMap::new();
    for device in devices {
        let Some(rssi) = device.rssi else { continue };
        if !wanted.is_empty() {
            let announced: Vec<String> = device.uuids.iter().map(|u| expand_uuid(u)).collect();
            if !wanted.iter().any(|w| announced.contains(w)) {
                continue;
            }
        }
        let id = device.address.to_ascii_uppercase();
        let candidate = BtDevice {
            id: id.clone(),
            name: device.name.clone().filter(|n| !n.trim().is_empty()),
            rssi: i32::from(rssi),
        };
        match best.get(&id) {
            Some(known) if known.rssi >= candidate.rssi => {}
            _ => {
                best.insert(id, candidate);
            }
        }
    }
    let mut out: Vec<BtDevice> = best.into_values().collect();
    out.sort_by(|a, b| b.rssi.cmp(&a.rssi).then_with(|| a.id.cmp(&b.id)));
    out
}

/// L'état de la radio à partir de ce qu'on a pu lire.
///
/// `Ok(None)` : le bus répond, pas d'adaptateur : `unsupported`. Un refus d'accès est
/// `unauthorized` ; un bus muet ou une erreur passagère, `unknown` — on ne devine pas.
pub fn radio_state_of(outcome: &Result<Option<bool>, DeviceError>) -> RadioState {
    match outcome {
        Ok(Some(true)) => RadioState::PoweredOn,
        Ok(Some(false)) => RadioState::PoweredOff,
        Ok(None) => RadioState::Unsupported,
        Err(DeviceError::NotSupported) => RadioState::Unsupported,
        Err(DeviceError::NotAuthorized) => RadioState::Unauthorized,
        Err(_) => RadioState::Unknown,
    }
}

/// L'état de permission. Linux n'a pas de dialogue Bluetooth : si le bus répond et qu'un
/// adaptateur existe, l'accès est celui du système (`granted`) ; sans adaptateur ni bus,
/// `unsupported` ; un refus d'accès D-Bus, `denied`.
pub fn permission_of(outcome: &Result<Option<bool>, DeviceError>) -> PermissionState {
    match outcome {
        Ok(Some(_)) => PermissionState::Granted,
        Err(DeviceError::NotAuthorized) => PermissionState::Denied,
        _ => PermissionState::Unsupported,
    }
}

/// Le résultat d'un balayage, ou le refus à rendre, à partir de deux lectures.
pub fn scan_outcome(adapter: Option<&RawAdapter>, after: &Snapshot, services: &[String]) -> Reply {
    match adapter {
        None => Reply::Error {
            code: DeviceError::NotSupported,
        },
        Some(a) if !a.powered => Reply::Error {
            code: DeviceError::Unavailable,
        },
        Some(_) => Reply::Devices {
            devices: seen_devices(&after.devices, services),
        },
    }
}

type Interfaces = HashMap<String, HashMap<String, OwnedValue>>;

fn text(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|v| v.downcast_ref::<&str>().ok())
        .map(str::to_owned)
}

/// Les adaptateurs et appareils d'une réponse `GetManagedObjects`. Une propriété d'un type
/// inattendu est ignorée : la réponse d'un démon ne doit jamais faire paniquer le pont.
pub fn parse_objects(objects: HashMap<OwnedObjectPath, Interfaces>) -> Snapshot {
    let mut snapshot = Snapshot::default();
    for (path, interfaces) in objects {
        if let Some(props) = interfaces.get(ADAPTER_IFACE) {
            let powered = props
                .get("Powered")
                .and_then(|v| v.downcast_ref::<bool>().ok())
                .unwrap_or(false);
            snapshot.adapters.push(RawAdapter {
                path: path.to_string(),
                powered,
            });
        }
        if let Some(props) = interfaces.get(DEVICE_IFACE) {
            let Some(address) = text(props, "Address") else {
                continue;
            };
            let uuids = props
                .get("UUIDs")
                .and_then(|v| {
                    let list = v.downcast_ref::<zbus::zvariant::Array>().ok()?;
                    Some(
                        list.iter()
                            .filter_map(|item| item.downcast_ref::<&str>().ok().map(str::to_owned))
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default();
            snapshot.devices.push(RawDevice {
                address,
                name: text(props, "Name"),
                rssi: props.get("RSSI").and_then(|v| v.downcast_ref::<i16>().ok()),
                uuids,
            });
        }
    }
    // Un ordre stable : HashMap n'en a pas.
    snapshot.adapters.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot
}

/// Lit l'ObjectManager de BlueZ, borné.
async fn read_snapshot(conn: &Connection) -> Result<Snapshot, DeviceError> {
    bounded(BUS_STEP_TIMEOUT, async {
        let manager = Proxy::new(conn, SERVICE, "/", OBJECT_MANAGER_IFACE)
            .await
            .map_err(|e| error_of(&e))?;
        let reply = manager
            .call_method("GetManagedObjects", &())
            .await
            .map_err(|e| error_of(&e))?;
        let objects: HashMap<OwnedObjectPath, Interfaces> = reply
            .body()
            .deserialize()
            .map_err(|_| DeviceError::Internal)?;
        Ok(parse_objects(objects))
    })
    .await
}

/// L'état brut de la radio : `Some(allumée)` s'il y a un adaptateur, `None` sinon.
async fn radio_outcome() -> Result<Option<bool>, DeviceError> {
    let conn = system_bus().await?;
    let snapshot = read_snapshot(&conn).await?;
    Ok(snapshot.adapters.first().map(|a| a.powered))
}

/// L'état de permission du Bluetooth, sans rien démarrer.
pub async fn permission_state() -> PermissionState {
    permission_of(&radio_outcome().await)
}

/// `bluetoothState`, dans la forme du fil.
pub async fn state_json() -> Value {
    let state = radio_state_of(&radio_outcome().await);
    serde_json::to_value(BluetoothReply::State { state })
        .unwrap_or_else(|_| super::error_json(DeviceError::Internal))
}

/// Un appel D-Bus sans valeur de retour, borné.
async fn call_unit(
    proxy: &Proxy<'_>,
    method: &'static str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> Result<(), DeviceError> {
    bounded(BUS_STEP_TIMEOUT, async {
        proxy
            .call_method(method, body)
            .await
            .map(|_| ())
            .map_err(|e| error_of(&e))
    })
    .await
}

/// Un balayage borné : `duration_ms` (déjà clampé) d'écoute, les appareils vus une fois chacun.
///
/// Sans bus : `Reply::Error{not_supported}`. Radio éteinte : `unavailable`. Sans adaptateur :
/// `not_supported`. Le temps total est fini : voir la documentation du module.
pub async fn scan(duration_ms: u32, services: &[String]) -> Reply {
    let conn = match system_bus().await {
        Ok(conn) => conn,
        Err(code) => return Reply::Error { code },
    };
    let before = match read_snapshot(&conn).await {
        Ok(snapshot) => snapshot,
        Err(code) => return Reply::Error { code },
    };
    let Some(adapter) = before.adapters.first().cloned() else {
        return scan_outcome(None, &before, services);
    };
    if !adapter.powered {
        return scan_outcome(Some(&adapter), &before, services);
    }
    let Ok(path) = OwnedObjectPath::try_from(adapter.path.as_str()) else {
        return Reply::Error {
            code: DeviceError::Internal,
        };
    };
    let proxy = match Proxy::new(&conn, SERVICE, path, ADAPTER_IFACE).await {
        Ok(proxy) => proxy,
        Err(e) => {
            return Reply::Error { code: error_of(&e) };
        }
    };
    // Le filtre est une aide, pas une condition : s'il échoue, le tri final l'applique quand même.
    if !services.is_empty() {
        let mut filter: HashMap<&str, Variant<'_>> = HashMap::new();
        let uuids: Vec<String> = services.iter().map(|s| expand_uuid(s)).collect();
        filter.insert("UUIDs", Variant::from(uuids));
        let _ = call_unit(&proxy, "SetDiscoveryFilter", &(filter,)).await;
    }
    // « Déjà en cours » (une autre application balaie) n'est pas un échec : on écoute, et on
    // n'arrête pas ce qu'on n'a pas démarré.
    let started = match call_unit(&proxy, "StartDiscovery", &()).await {
        Ok(()) => true,
        Err(DeviceError::Unavailable) => false,
        Err(code) => return Reply::Error { code },
    };
    tokio::time::sleep(Duration::from_millis(u64::from(duration_ms))).await;
    let after = read_snapshot(&conn).await;
    if started {
        let _ = call_unit(&proxy, "StopDiscovery", &()).await;
    }
    match after {
        Ok(after) => scan_outcome(Some(&adapter), &after, services),
        Err(code) => Reply::Error { code },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(address: &str, name: Option<&str>, rssi: Option<i16>, uuids: &[&str]) -> RawDevice {
        RawDevice {
            address: address.into(),
            name: name.map(str::to_owned),
            rssi,
            uuids: uuids.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn short_service_ids_expand_to_the_128_bit_form_bluez_reports() {
        assert_eq!(expand_uuid("180D"), "0000180d-0000-1000-8000-00805f9b34fb");
        assert_eq!(
            expand_uuid("0000180d"),
            "0000180d-0000-1000-8000-00805f9b34fb"
        );
        assert_eq!(
            expand_uuid("0000180D-0000-1000-8000-00805F9B34FB"),
            "0000180d-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn a_cached_device_without_rssi_was_not_seen() {
        let seen = seen_devices(&[dev("AA:BB:CC:DD:EE:01", Some("Cache"), None, &[])], &[]);
        assert!(seen.is_empty());
    }

    #[test]
    fn devices_are_sorted_by_signal_then_id_and_named_or_none() {
        let seen = seen_devices(
            &[
                dev("AA:BB:CC:DD:EE:02", Some("Faible"), Some(-90), &[]),
                dev("aa:bb:cc:dd:ee:01", Some("  "), Some(-40), &[]),
                dev("AA:BB:CC:DD:EE:03", Some("Fort"), Some(-40), &[]),
            ],
            &[],
        );
        let ids: Vec<&str> = seen.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "AA:BB:CC:DD:EE:01",
                "AA:BB:CC:DD:EE:03",
                "AA:BB:CC:DD:EE:02"
            ]
        );
        // Un nom vide est « inconnu », comme CoreBluetooth rend `null`.
        assert_eq!(seen[0].name, None);
        assert_eq!(seen[1].name.as_deref(), Some("Fort"));
    }

    #[test]
    fn an_address_is_listed_once_with_its_strongest_signal() {
        let seen = seen_devices(
            &[
                dev("AA:BB:CC:DD:EE:01", None, Some(-80), &[]),
                dev("aa:bb:cc:dd:ee:01", None, Some(-50), &[]),
            ],
            &[],
        );
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].rssi, -50);
    }

    #[test]
    fn a_service_filter_keeps_only_announcing_devices_in_any_form() {
        let heart = "0000180d-0000-1000-8000-00805f9b34fb";
        let seen = seen_devices(
            &[
                dev("AA:BB:CC:DD:EE:01", None, Some(-50), &[heart]),
                dev(
                    "AA:BB:CC:DD:EE:02",
                    None,
                    Some(-50),
                    &["0000180f-0000-1000-8000-00805f9b34fb"],
                ),
                dev("AA:BB:CC:DD:EE:03", None, Some(-50), &[]),
            ],
            &["180D".to_string()],
        );
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].id, "AA:BB:CC:DD:EE:01");
    }

    #[test]
    fn radio_state_never_guesses() {
        assert_eq!(radio_state_of(&Ok(Some(true))), RadioState::PoweredOn);
        assert_eq!(radio_state_of(&Ok(Some(false))), RadioState::PoweredOff);
        assert_eq!(radio_state_of(&Ok(None)), RadioState::Unsupported);
        assert_eq!(
            radio_state_of(&Err(DeviceError::NotSupported)),
            RadioState::Unsupported
        );
        assert_eq!(
            radio_state_of(&Err(DeviceError::NotAuthorized)),
            RadioState::Unauthorized
        );
        // Un bus muet : on ne sait pas, on ne dit pas « éteint ».
        assert_eq!(
            radio_state_of(&Err(DeviceError::Timeout)),
            RadioState::Unknown
        );
    }

    #[test]
    fn no_bus_or_no_adapter_is_unsupported_never_granted() {
        assert_eq!(
            permission_of(&Err(DeviceError::NotSupported)),
            PermissionState::Unsupported
        );
        assert_eq!(permission_of(&Ok(None)), PermissionState::Unsupported);
        assert_eq!(
            permission_of(&Err(DeviceError::Timeout)),
            PermissionState::Unsupported
        );
        assert_eq!(permission_of(&Ok(Some(false))), PermissionState::Granted);
        assert_eq!(
            permission_of(&Err(DeviceError::NotAuthorized)),
            PermissionState::Denied
        );
    }

    #[test]
    fn a_scan_without_adapter_or_with_the_radio_off_is_a_typed_refusal() {
        let empty = Snapshot::default();
        assert_eq!(
            scan_outcome(None, &empty, &[]),
            Reply::Error {
                code: DeviceError::NotSupported
            }
        );
        let off = RawAdapter {
            path: "/org/bluez/hci0".into(),
            powered: false,
        };
        assert_eq!(
            scan_outcome(Some(&off), &empty, &[]),
            Reply::Error {
                code: DeviceError::Unavailable
            }
        );
        let on = RawAdapter {
            powered: true,
            ..off
        };
        assert_eq!(
            scan_outcome(Some(&on), &empty, &[]),
            Reply::Devices { devices: vec![] }
        );
    }

    fn props(pairs: Vec<(&str, Variant<'_>)>) -> HashMap<String, OwnedValue> {
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), OwnedValue::try_from(v).unwrap()))
            .collect()
    }

    #[test]
    fn a_managed_objects_reply_is_parsed_and_a_bad_property_type_is_ignored() {
        let mut objects: HashMap<OwnedObjectPath, Interfaces> = HashMap::new();
        objects.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(),
            HashMap::from([(
                ADAPTER_IFACE.to_string(),
                props(vec![("Powered", Variant::from(true))]),
            )]),
        );
        objects.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA").unwrap(),
            HashMap::from([(
                DEVICE_IFACE.to_string(),
                props(vec![
                    ("Address", Variant::from("AA:BB:CC:DD:EE:01")),
                    ("Name", Variant::from("Capteur")),
                    ("RSSI", Variant::from(-61_i16)),
                    (
                        "UUIDs",
                        Variant::from(vec!["0000180d-0000-1000-8000-00805f9b34fb"]),
                    ),
                ]),
            )]),
        );
        // Un appareil dont l'adresse n'est pas du texte : ignoré, sans panique.
        objects.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0/dev_BB").unwrap(),
            HashMap::from([(
                DEVICE_IFACE.to_string(),
                props(vec![("Address", Variant::from(7_u32))]),
            )]),
        );
        let snapshot = parse_objects(objects);
        assert_eq!(
            snapshot.adapters,
            vec![RawAdapter {
                path: "/org/bluez/hci0".into(),
                powered: true
            }]
        );
        assert_eq!(
            snapshot.devices,
            vec![dev(
                "AA:BB:CC:DD:EE:01",
                Some("Capteur"),
                Some(-61),
                &["0000180d-0000-1000-8000-00805f9b34fb"]
            )]
        );
    }
}
