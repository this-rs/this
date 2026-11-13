//! Localisation sur Linux : GeoClue2 (`org.freedesktop.GeoClue2`) via le bus système D-Bus.
//!
//! Déroulé d'un échantillon : `Manager.GetClient` crée un client, on fixe son `DesktopId` et sa
//! précision, `Start` demande la position, on lit la propriété `Location` jusqu'à ce qu'elle
//! pointe un objet (GeoClue la met à jour dès qu'il a une position), puis `Stop` et
//! `DeleteClient` libèrent le client. Tout l'échange est borné par [`LOCATION_TIMEOUT`], plus
//! court que `NATIVE_TIMEOUT` (10 s) pour que la réponse soit la nôtre, typée, et non un
//! abandon du pont.
//!
//! Refus typés (vocabulaire de `DeviceError`, voir [`super::error_of`]) :
//! - GeoClue absent (pas de bus système, service inconnu) : `not_supported` ;
//! - accès refusé par GeoClue ou son agent : `not_authorized` ;
//! - délai dépassé : `timeout` ; autre erreur D-Bus : `unavailable`.
//!
//! La logique pure (`validate_fix`, `timestamp_ms`, `fix_answer`, `status_answer`) est testée
//! sans bus. L'I/O ne fait que parler à GeoClue.

use std::time::Duration;

use serde_json::{Value, json};
use this_platform::device_bridge::{DeviceError, PermissionState};
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, Proxy};

use super::{bounded, error_of, system_bus};
use crate::families::location::PositionFix;

/// Le délai maximal d'un échange complet (demande, attente d'une position, nettoyage).
pub const LOCATION_TIMEOUT: Duration = Duration::from_secs(8);

/// L'intervalle entre deux lectures de la propriété `Location` pendant l'attente.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

const SERVICE: &str = "org.freedesktop.GeoClue2";
const MANAGER_PATH: &str = "/org/freedesktop/GeoClue2/Manager";
const MANAGER_IFACE: &str = "org.freedesktop.GeoClue2.Manager";
const CLIENT_IFACE: &str = "org.freedesktop.GeoClue2.Client";
const LOCATION_IFACE: &str = "org.freedesktop.GeoClue2.Location";

/// La précision demandée à GeoClue : 8 = « exacte » (`GCLUE_ACCURACY_LEVEL_EXACT`).
pub const ACCURACY_EXACT: u32 = 8;

/// L'identifiant d'application présenté à GeoClue. GeoClue l'autorise (ou non) par un fichier
/// `.desktop` ou son agent : il doit correspondre à l'application installée — à vérifier sur
/// une machine réelle.
pub const DESKTOP_ID: &str = "this-device";

/// Une position venue de GeoClue, vérifiée avant d'entrer dans le contrat. Une coordonnée hors
/// de sa plage, ou une précision négative, est une réponse fausse : refusée `internal`.
pub fn validate_fix(
    latitude: f64,
    longitude: f64,
    accuracy_m: f64,
    ts_ms: u64,
) -> Result<PositionFix, DeviceError> {
    let ok = latitude.is_finite()
        && longitude.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
        && accuracy_m.is_finite()
        && accuracy_m >= 0.0;
    if !ok {
        return Err(DeviceError::Internal);
    }
    Ok(PositionFix {
        latitude,
        longitude,
        accuracy_m,
        ts_ms,
    })
}

/// L'horodatage GeoClue (secondes et microsecondes Unix) en millisecondes, sans débordement.
pub fn timestamp_ms(seconds: u64, microseconds: u64) -> u64 {
    seconds
        .saturating_mul(1000)
        .saturating_add(microseconds / 1000)
}

/// La réponse du pont à `locationSample` : la position avec son `kind`, ou un refus.
pub fn fix_answer(result: Result<PositionFix, DeviceError>) -> Value {
    match result {
        Ok(fix) => {
            let mut value = serde_json::to_value(fix).unwrap_or_else(|_| json!({}));
            if let Value::Object(map) = &mut value {
                map.insert("kind".to_string(), json!("location"));
            }
            value
        }
        Err(code) => json!({ "kind": "error", "code": code }),
    }
}

/// La réponse du pont à `locationRequest` : l'état de la permission, ou un refus.
pub fn status_answer(result: Result<PermissionState, DeviceError>) -> Value {
    match result {
        Ok(state) => json!({ "kind": "status", "state": state }),
        Err(code) => json!({ "kind": "error", "code": code }),
    }
}

/// Un client GeoClue créé, avec son chemin, pour le libérer ensuite.
struct Client {
    proxy: Proxy<'static>,
    path: OwnedObjectPath,
}

/// Crée un client GeoClue, fixe son identifiant et sa précision, et le démarre.
async fn create_and_start(conn: &Connection) -> Result<Client, zbus::Error> {
    let manager = Proxy::new(conn, SERVICE, MANAGER_PATH, MANAGER_IFACE).await?;
    let path: OwnedObjectPath = manager
        .call_method("GetClient", &())
        .await?
        .body()
        .deserialize()?;
    let proxy = Proxy::new(conn, SERVICE, path.clone(), CLIENT_IFACE).await?;
    proxy.set_property("DesktopId", DESKTOP_ID).await?;
    proxy
        .set_property("RequestedAccuracyLevel", ACCURACY_EXACT)
        .await?;
    proxy.call_method("Start", &()).await?;
    Ok(Client { proxy, path })
}

/// Arrête le client et le retire du service. Best effort et borné : un nettoyage raté ne
/// change pas la réponse déjà obtenue, et GeoClue retire de toute façon un client orphelin.
async fn stop_and_delete(conn: &Connection, client: &Client) {
    let cleanup = async {
        let _ = client.proxy.call_method("Stop", &()).await;
        if let Ok(manager) = Proxy::new(conn, SERVICE, MANAGER_PATH, MANAGER_IFACE).await {
            let _ = manager.call_method("DeleteClient", &client.path).await;
        }
        Ok::<(), DeviceError>(())
    };
    let _ = bounded(Duration::from_secs(2), cleanup).await;
}

/// Lit la position du client : attend que `Location` pointe un objet, puis lit ses propriétés.
async fn read_fix(conn: &Connection, client: &Client) -> Result<PositionFix, DeviceError> {
    let location_path = loop {
        let current: OwnedObjectPath = client
            .proxy
            .get_property("Location")
            .await
            .map_err(|e| error_of(&e))?;
        if current.as_str() != "/" {
            break current;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    let location = Proxy::new(conn, SERVICE, location_path, LOCATION_IFACE)
        .await
        .map_err(|e| error_of(&e))?;
    let latitude: f64 = location
        .get_property("Latitude")
        .await
        .map_err(|e| error_of(&e))?;
    let longitude: f64 = location
        .get_property("Longitude")
        .await
        .map_err(|e| error_of(&e))?;
    let accuracy_m: f64 = location
        .get_property("Accuracy")
        .await
        .map_err(|e| error_of(&e))?;
    let ts_ms = read_timestamp_ms(&location).await;
    validate_fix(latitude, longitude, accuracy_m, ts_ms)
}

/// L'horodatage de la position. GeoClue l'expose en paire secondes-microsecondes `(tt)` ; un
/// repli sur un `t` seul est lu aussi. Absent des deux, l'instant de la lecture sert de repli,
/// sans inventer une date passée.
async fn read_timestamp_ms(location: &Proxy<'_>) -> u64 {
    if let Ok((seconds, micros)) = location.get_property::<(u64, u64)>("Timestamp").await {
        return timestamp_ms(seconds, micros);
    }
    if let Ok(seconds) = location.get_property::<u64>("Timestamp").await {
        return timestamp_ms(seconds, 0);
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Une position, bornée par [`LOCATION_TIMEOUT`]. GeoClue absent : `not_supported`.
pub async fn location_sample() -> Result<PositionFix, DeviceError> {
    bounded(LOCATION_TIMEOUT, sample_inner()).await
}

async fn sample_inner() -> Result<PositionFix, DeviceError> {
    let conn = system_bus().await?;
    let client = create_and_start(&conn).await.map_err(|e| error_of(&e))?;
    let result = read_fix(&conn, &client).await;
    stop_and_delete(&conn, &client).await;
    result
}

/// L'état de la permission de localisation. GeoClue n'a pas de lecture passive : on démarre
/// un client, et son accord ou son refus donne l'état. Ce n'est pas un dialogue ouvert par ce
/// module : si GeoClue a un agent, c'est lui qui répond.
pub async fn location_request() -> Result<PermissionState, DeviceError> {
    bounded(LOCATION_TIMEOUT, request_inner()).await
}

async fn request_inner() -> Result<PermissionState, DeviceError> {
    let conn = system_bus().await?;
    match create_and_start(&conn).await {
        Ok(client) => {
            stop_and_delete(&conn, &client).await;
            Ok(PermissionState::Granted)
        }
        Err(err) => match error_of(&err) {
            DeviceError::NotAuthorized => Ok(PermissionState::Denied),
            other => Err(other),
        },
    }
}

/// `locationSample`, dans la forme du fil.
pub async fn sample_json() -> Value {
    fix_answer(location_sample().await)
}

/// `locationRequest`, dans la forme du fil.
pub async fn request_json() -> Value {
    status_answer(location_request().await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::error_for_dbus_name;

    #[test]
    fn a_missing_geoclue_service_is_a_typed_refusal() {
        assert_eq!(
            error_for_dbus_name("org.freedesktop.DBus.Error.ServiceUnknown"),
            DeviceError::NotSupported
        );
        assert_eq!(
            error_for_dbus_name("org.freedesktop.DBus.Error.NameHasNoOwner"),
            DeviceError::NotSupported
        );
    }

    #[test]
    fn a_refusal_from_geoclue_or_its_agent_is_not_authorized() {
        assert_eq!(
            error_for_dbus_name("org.freedesktop.GeoClue2.Error.AccessDenied"),
            DeviceError::NotAuthorized
        );
        assert_eq!(
            error_for_dbus_name("org.freedesktop.GeoClue2.Error.NotAuthorized"),
            DeviceError::NotAuthorized
        );
    }

    #[test]
    fn a_silent_bus_is_a_timeout_and_an_unknown_error_is_unavailable() {
        assert_eq!(
            error_for_dbus_name("org.freedesktop.DBus.Error.NoReply"),
            DeviceError::Timeout
        );
        assert_eq!(
            error_for_dbus_name("org.freedesktop.GeoClue2.Error.Whatever"),
            DeviceError::Unavailable
        );
    }

    #[test]
    fn a_plausible_fix_is_accepted_as_it_is() {
        let fix = validate_fix(48.8566, 2.3522, 12.5, 1_700_000_000_123).unwrap();
        assert_eq!(fix.latitude, 48.8566);
        assert_eq!(fix.longitude, 2.3522);
        assert_eq!(fix.accuracy_m, 12.5);
        assert_eq!(fix.ts_ms, 1_700_000_000_123);
    }

    #[test]
    fn an_out_of_range_or_non_finite_fix_is_refused_as_internal() {
        assert_eq!(validate_fix(91.0, 0.0, 1.0, 1), Err(DeviceError::Internal));
        assert_eq!(
            validate_fix(0.0, -181.0, 1.0, 1),
            Err(DeviceError::Internal)
        );
        assert_eq!(
            validate_fix(f64::NAN, 0.0, 1.0, 1),
            Err(DeviceError::Internal)
        );
        assert_eq!(validate_fix(0.0, 0.0, -1.0, 1), Err(DeviceError::Internal));
        assert_eq!(
            validate_fix(0.0, 0.0, f64::INFINITY, 1),
            Err(DeviceError::Internal)
        );
    }

    #[test]
    fn the_geoclue_timestamp_is_milliseconds_without_overflow() {
        assert_eq!(timestamp_ms(1_700_000_000, 123_456), 1_700_000_000_123);
        assert_eq!(timestamp_ms(u64::MAX, 0), u64::MAX);
    }

    #[test]
    fn a_fix_answer_carries_its_kind_and_its_fields() {
        let fix = PositionFix {
            latitude: 1.5,
            longitude: -2.0,
            accuracy_m: 3.0,
            ts_ms: 7,
        };
        let value = fix_answer(Ok(fix));
        assert_eq!(value["kind"], "location");
        assert_eq!(value["latitude"], 1.5);
        assert_eq!(value["ts_ms"], 7);
        assert_eq!(
            fix_answer(Err(DeviceError::Timeout)),
            json!({ "kind": "error", "code": "timeout" })
        );
    }

    #[test]
    fn a_status_answer_uses_the_contract_states() {
        assert_eq!(
            status_answer(Ok(PermissionState::Denied)),
            json!({ "kind": "status", "state": "denied" })
        );
        assert_eq!(
            status_answer(Err(DeviceError::NotSupported)),
            json!({ "kind": "error", "code": "not_supported" })
        );
    }
}
