//! Mouvement sur Linux : un échantillon de gyroscope lu dans l'IIO du noyau.
//!
//! Le module n'a pas de `cfg` interne : `lib.rs` le déclare sous `#[cfg(target_os = "linux")]`.
//! Il ne dépend que de `std` et de `tokio` (le délai).
//!
//! Source : `/sys/bus/iio/devices/iio:deviceN/`. Un gyroscope y expose `in_anglvel_{x,y,z}_raw`,
//! et son échelle (`in_anglvel_{x,y,z}_scale`, ou `in_anglvel_scale` partagée) en rad/s par
//! unité brute. La valeur d'un axe suit la convention IIO : `(raw + offset) * scale`.
//!
//! Deux parties, séparées pour être testées sans capteur :
//! - la logique pure (`parse_raw`, `parse_factor`, `axis_value`, `sample_from_attrs`,
//!   `select_device_names`) ne voit que des textes déjà lus ;
//! - l'I/O (`read_once`, `gyroscope_sample_in`) lit `sysfs` et attend un capteur borné.
//!
//! Un capteur absent est un refus typé `unavailable`, rendu après un délai borné : le capteur
//! peut apparaître tard (pilote chargé au démarrage), mais une absence ne bloque pas le tour.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use this_platform::device_bridge::{DeviceError, PermissionState, Reply};

/// La racine des périphériques IIO du noyau.
pub const IIO_ROOT: &str = "/sys/bus/iio/devices";

/// Le délai maximal d'attente d'un capteur avant le refus `unavailable`.
pub const SAMPLE_WAIT_MAX: Duration = Duration::from_secs(2);

/// L'intervalle entre deux tentatives de lecture pendant l'attente.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Les trois axes d'un gyroscope, en rad/s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GyroAxes {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Un entier brut d'IIO, lu depuis son fichier texte. Un texte qui n'est pas un entier est
/// refusé : une valeur approchée d'un capteur serait une mesure inventée.
pub fn parse_raw(text: &str) -> Option<i64> {
    text.trim().parse().ok()
}

/// Un facteur (échelle ou décalage), fini. `NaN`, l'infini et le texte sont refusés.
pub fn parse_factor(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// La valeur physique d'un axe, selon la convention IIO : `(raw + offset) * scale`.
pub fn axis_value(raw: i64, offset: f64, scale: f64) -> f64 {
    (raw as f64 + offset) * scale
}

/// Les noms d'attributs d'un axe : la donnée brute, puis l'échelle et le décalage, par axe
/// (`in_anglvel_x_scale`) ou partagés par le type (`in_anglvel_scale`).
fn raw_attr(axis: &str) -> String {
    format!("in_anglvel_{axis}_raw")
}

/// Lit une valeur de facteur : d'abord celle de l'axe, puis celle partagée par le type.
fn factor(read: &impl Fn(&str) -> Option<String>, axis: &str, kind: &str) -> Option<f64> {
    read(&format!("in_anglvel_{axis}_{kind}"))
        .or_else(|| read(&format!("in_anglvel_{kind}")))
        .as_deref()
        .and_then(parse_factor)
}

/// Un périphérique IIO est-il un gyroscope ? Il en est un s'il expose les trois axes bruts.
pub fn is_gyroscope(read: &impl Fn(&str) -> Option<String>) -> bool {
    ["x", "y", "z"]
        .iter()
        .all(|axis| read(&raw_attr(axis)).is_some())
}

/// Un échantillon à partir des attributs d'un périphérique, lus par `read` (un nom d'attribut
/// donne son contenu, ou `None` s'il n'existe pas). Une échelle absente ou un brut illisible
/// rend `unavailable` : sans échelle, un brut n'a pas d'unité.
pub fn sample_from_attrs(read: impl Fn(&str) -> Option<String>) -> Result<GyroAxes, DeviceError> {
    let mut values = [0.0_f64; 3];
    for (slot, axis) in values.iter_mut().zip(["x", "y", "z"]) {
        let raw = read(&raw_attr(axis))
            .as_deref()
            .and_then(parse_raw)
            .ok_or(DeviceError::Unavailable)?;
        let scale = factor(&read, axis, "scale").ok_or(DeviceError::Unavailable)?;
        // Un décalage absent vaut zéro : c'est la valeur par défaut du noyau.
        let offset = factor(&read, axis, "offset").unwrap_or(0.0);
        *slot = axis_value(raw, offset, scale);
    }
    Ok(GyroAxes {
        x: values[0],
        y: values[1],
        z: values[2],
    })
}

/// Les noms des périphériques IIO candidats, triés pour un choix déterministe. Seuls les
/// `iio:deviceN` comptent : un `trigger` ou un autre nom n'est pas un capteur.
pub fn select_device_names<'a>(names: &[&'a str]) -> Vec<&'a str> {
    let mut picked: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| {
            name.strip_prefix("iio:device").is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .collect();
    picked.sort_unstable();
    picked
}

/// Le contenu d'un fichier d'attribut, sans le retour à la ligne final, ou `None`.
fn attr_in(dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(dir.join(name))
        .ok()
        .map(|text| text.trim().to_string())
}

/// Une lecture unique : le premier périphérique gyroscope de `root` qui donne un échantillon.
/// `None` si aucun n'en donne un (dossier absent, aucun capteur, ou valeurs illisibles).
pub fn read_once(root: &Path) -> Option<GyroAxes> {
    let entries = fs::read_dir(root).ok()?;
    let names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    select_device_names(&refs).into_iter().find_map(|name| {
        let dir = root.join(name);
        let read = |attr: &str| attr_in(&dir, attr);
        if !is_gyroscope(&read) {
            return None;
        }
        sample_from_attrs(read).ok()
    })
}

/// Le temps courant en millisecondes Unix. Une horloge antérieure à l'époque donne zéro, sans
/// panique.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// La réponse du pont pour un échantillon ou un refus. `ts_ms` est l'instant de la lecture.
pub fn reply_of(sample: Option<GyroAxes>, ts_ms: u64) -> Reply {
    match sample {
        Some(axes) => Reply::Gyroscope {
            x: axes.x,
            y: axes.y,
            z: axes.z,
            ts_ms,
        },
        None => Reply::Error {
            code: DeviceError::Unavailable,
        },
    }
}

/// Un échantillon de gyroscope depuis `IIO_ROOT`, avec le délai d'attente borné par
/// [`SAMPLE_WAIT_MAX`].
pub async fn gyroscope_sample() -> Reply {
    gyroscope_sample_in(Path::new(IIO_ROOT), SAMPLE_WAIT_MAX).await
}

/// Comme [`gyroscope_sample`], sur une racine et un délai donnés. Relit `root` jusqu'à obtenir
/// un échantillon ou épuiser `wait`, puis rend `unavailable`. La lecture de `sysfs` est
/// bloquante : elle tourne sur un fil à part, jamais sur l'exécuteur.
pub async fn gyroscope_sample_in(root: &Path, wait: Duration) -> Reply {
    let deadline = Instant::now() + wait;
    loop {
        let dir = root.to_path_buf();
        let axes = tokio::task::spawn_blocking(move || read_once(&dir))
            .await
            .ok()
            .flatten();
        if let Some(axes) = axes {
            return reply_of(Some(axes), now_ms());
        }
        let now = Instant::now();
        if now >= deadline {
            return reply_of(None, now_ms());
        }
        tokio::time::sleep(POLL_INTERVAL.min(deadline - now)).await;
    }
}

/// Un gyroscope est-il présent sous `root` ? Une seule lecture, sans attente.
pub fn has_gyroscope(root: &Path) -> bool {
    read_once(root).is_some()
}

/// L'état du gyroscope : `granted` s'il y en a un de lisible (le noyau n'a pas de dialogue,
/// l'accès est un droit de fichier), `unsupported` sinon. Jamais `prompt` : personne ne
/// demanderait rien.
pub async fn gyroscope_status() -> PermissionState {
    let found = tokio::task::spawn_blocking(|| has_gyroscope(Path::new(IIO_ROOT)))
        .await
        .unwrap_or(false);
    if found {
        PermissionState::Granted
    } else {
        PermissionState::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn attrs(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn a_raw_value_is_an_integer_or_refused() {
        assert_eq!(parse_raw(" -123\n"), Some(-123));
        assert_eq!(parse_raw("1.5"), None);
        assert_eq!(parse_raw(""), None);
    }

    #[test]
    fn a_factor_must_be_finite() {
        assert_eq!(parse_factor("0.00106526\n"), Some(0.00106526));
        assert_eq!(parse_factor("NaN"), None);
        assert_eq!(parse_factor("inf"), None);
        assert_eq!(parse_factor("abc"), None);
    }

    #[test]
    fn the_axis_value_follows_the_iio_convention() {
        // (raw + offset) * scale, pas raw * scale + offset.
        assert_eq!(axis_value(100, 10.0, 0.5), 55.0);
        assert_eq!(axis_value(-4, 0.0, 0.25), -1.0);
    }

    #[test]
    fn a_per_axis_scale_wins_over_the_shared_one() {
        let read = attrs(&[
            ("in_anglvel_x_raw", "100"),
            ("in_anglvel_y_raw", "100"),
            ("in_anglvel_z_raw", "100"),
            ("in_anglvel_x_scale", "0.5"),
            ("in_anglvel_scale", "0.1"),
        ]);
        let axes = sample_from_attrs(read).unwrap();
        assert_eq!(axes.x, 50.0, "axe x : son échelle propre");
        assert_eq!(axes.y, 10.0, "axe y : l'échelle partagée");
        assert_eq!(axes.z, 10.0);
    }

    #[test]
    fn a_missing_offset_is_zero() {
        let read = attrs(&[
            ("in_anglvel_x_raw", "2"),
            ("in_anglvel_y_raw", "2"),
            ("in_anglvel_z_raw", "2"),
            ("in_anglvel_scale", "0.5"),
        ]);
        let axes = sample_from_attrs(read).unwrap();
        assert_eq!(
            axes,
            GyroAxes {
                x: 1.0,
                y: 1.0,
                z: 1.0
            }
        );
    }

    #[test]
    fn a_missing_scale_is_refused_not_guessed() {
        let read = attrs(&[
            ("in_anglvel_x_raw", "2"),
            ("in_anglvel_y_raw", "2"),
            ("in_anglvel_z_raw", "2"),
        ]);
        assert_eq!(sample_from_attrs(read), Err(DeviceError::Unavailable));
    }

    #[test]
    fn an_unreadable_raw_value_is_refused() {
        let read = attrs(&[
            ("in_anglvel_x_raw", "oops"),
            ("in_anglvel_y_raw", "2"),
            ("in_anglvel_z_raw", "2"),
            ("in_anglvel_scale", "1"),
        ]);
        assert_eq!(sample_from_attrs(read), Err(DeviceError::Unavailable));
    }

    #[test]
    fn a_device_needs_all_three_axes_to_be_a_gyroscope() {
        let two_axes = attrs(&[("in_anglvel_x_raw", "1"), ("in_anglvel_y_raw", "1")]);
        assert!(!is_gyroscope(&two_axes));
        let three = attrs(&[
            ("in_anglvel_x_raw", "1"),
            ("in_anglvel_y_raw", "1"),
            ("in_anglvel_z_raw", "1"),
        ]);
        assert!(is_gyroscope(&three));
    }

    #[test]
    fn only_iio_device_names_are_candidates_and_they_are_sorted() {
        let names = [
            "iio:device10",
            "trigger0",
            "iio:device2",
            "iio:devicex",
            "iio:device",
            "iio:device0",
        ];
        assert_eq!(
            select_device_names(&names),
            vec!["iio:device0", "iio:device10", "iio:device2"]
        );
    }

    #[test]
    fn a_reply_carries_the_axes_or_a_typed_refusal() {
        let axes = GyroAxes {
            x: 0.5,
            y: -0.25,
            z: 0.0,
        };
        assert_eq!(
            reply_of(Some(axes), 1_700_000_000_123),
            Reply::Gyroscope {
                x: 0.5,
                y: -0.25,
                z: 0.0,
                ts_ms: 1_700_000_000_123,
            }
        );
        assert_eq!(
            reply_of(None, 1),
            Reply::Error {
                code: DeviceError::Unavailable
            }
        );
    }

    #[test]
    fn read_once_finds_the_gyroscope_in_an_iio_layout() {
        let root = std::env::temp_dir().join(format!("iio-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // Un accéléromètre (sans anglvel) et un gyroscope, dans le désordre des noms.
        let accel = root.join("iio:device0");
        let gyro = root.join("iio:device1");
        fs::create_dir_all(&accel).unwrap();
        fs::create_dir_all(&gyro).unwrap();
        fs::write(accel.join("in_accel_x_raw"), "9\n").unwrap();
        fs::write(gyro.join("in_anglvel_x_raw"), "200\n").unwrap();
        fs::write(gyro.join("in_anglvel_y_raw"), "-200\n").unwrap();
        fs::write(gyro.join("in_anglvel_z_raw"), "0\n").unwrap();
        fs::write(gyro.join("in_anglvel_scale"), "0.01\n").unwrap();

        let found = read_once(&root);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            found,
            Some(GyroAxes {
                x: 2.0,
                y: -2.0,
                z: 0.0
            })
        );
    }

    #[test]
    fn read_once_on_a_missing_root_is_none() {
        let root = std::env::temp_dir().join("iio-test-absent-root-does-not-exist");
        assert_eq!(read_once(&root), None);
    }

    #[test]
    fn an_absent_sensor_is_unavailable_after_a_bounded_wait() {
        let root = std::env::temp_dir().join("iio-test-absent-root-does-not-exist-2");
        let started = Instant::now();
        let reply = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(gyroscope_sample_in(&root, Duration::from_millis(150)));
        assert_eq!(
            reply,
            Reply::Error {
                code: DeviceError::Unavailable
            }
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "attente non bornée"
        );
    }

    #[test]
    fn has_gyroscope_is_false_without_iio() {
        assert!(!has_gyroscope(Path::new("/definitivement/absent/iio")));
    }
}
