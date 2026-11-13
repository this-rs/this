//! Caméra et micro sur Linux : l'état de la permission, lu sans rien démarrer.
//!
//! Deux parties, séparées pour être testées sans matériel :
//! - la logique pure (`camera_state`, `microphone_state`, `select_video_nodes`,
//!   `audio_socket_candidates`, `is_sandboxed`) prend des faits déjà observés et rend un état ;
//! - l'I/O (`probe_*`, [`state_of`]) lit `/dev`, les sockets audio et le portail, et ne fait que cela.
//!
//! Sur Linux, il n'y a pas de dialogue système à ouvrir depuis ce processus. Deux mondes :
//! - **hors sandbox**, l'accès est dicté par les droits du système de fichiers : appartenance au
//!   groupe `video` pour `/dev/videoN`, socket PipeWire/Pulse de `$XDG_RUNTIME_DIR` pour le micro.
//!   L'état rendu est celui que le système affiche (`granted` si ouvrable), pas une décision de la
//!   personne ;
//! - **en sandbox** (Flatpak, Snap), l'accès passe par le portail XDG
//!   (`org.freedesktop.portal.Desktop`, bus session), qui interroge la personne à l'usage. Ce
//!   module ne peut pas lire cette décision : il rend `prompt` si le portail est là, `unsupported`
//!   sinon. Jamais `granted` : ce serait mentir.
//!
//! Une demande (`captureRequest`) rend donc l'état réel, comme la lecture : aucun dialogue n'est
//! ouvert, et rien n'est affirmé qui ne soit vérifié.

use std::fs;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use this_platform::device_bridge::{DeviceError, PermissionState};

use super::{BUS_STEP_TIMEOUT, bounded, error_of};
use crate::families::camera_mic::CaptureCapability;

/// Ce que l'environnement dit du monde où tourne le processus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    /// Flatpak ou Snap : le système de fichiers ne dit plus la vérité, le portail décide.
    pub sandboxed: bool,
    /// Le portail XDG possède son nom sur le bus session.
    pub portal: bool,
}

/// Le processus est-il en sandbox ? Flatpak pose `/.flatpak-info` ; Snap exporte `SNAP`.
pub fn is_sandboxed(flatpak_info_exists: bool, snap_env: Option<&str>) -> bool {
    flatpak_info_exists || snap_env.is_some_and(|v| !v.is_empty())
}

/// Ce qu'une ouverture de test a donné pour un noeud V4L2 (`/dev/videoN`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeAccess {
    /// Ouvert en lecture : le processus a le droit d'accéder au périphérique.
    Openable,
    /// Refusé par le système (EACCES, EPERM) : le groupe `video` manque, par exemple.
    PermissionDenied,
    /// Autre erreur (occupé, disparu entre-temps) : la réponse n'est pas nette.
    Other,
}

/// Ce qu'une lecture de métadonnées a donné pour un chemin de socket audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketAccess {
    /// Absent, ou présent mais qui n'est pas une socket.
    Absent,
    /// Une socket Unix est là : le serveur audio tourne pour cette session.
    Present,
    /// Le chemin existe mais le système refuse de le lire (EACCES).
    PermissionDenied,
}

/// Un nom de noeud V4L2 : `video` suivi d'au moins un chiffre, et rien d'autre.
/// `video-dec0` ou `videox` ne sont pas des caméras.
pub fn is_video_node_name(name: &str) -> bool {
    name.strip_prefix("video")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Les chemins des noeuds vidéo, triés par nom pour un résultat déterministe quel que soit
/// l'ordre que rend le système de fichiers.
pub fn select_video_nodes(dir: &Path, names: &[String]) -> Vec<PathBuf> {
    let mut kept: Vec<&String> = names.iter().filter(|n| is_video_node_name(n)).collect();
    kept.sort();
    kept.into_iter().map(|n| dir.join(n)).collect()
}

/// Les chemins de socket audio à sonder, dans l'ordre de préférence.
///
/// - `PIPEWIRE_REMOTE` : chemin absolu explicite, s'il est donné.
/// - `PULSE_SERVER` : sa forme `unix:/chemin` (les espaces séparent des alternatives).
/// - `$XDG_RUNTIME_DIR/pipewire-0` puis `$XDG_RUNTIME_DIR/pulse/native`, les emplacements par
///   défaut de la session de l'utilisateur.
///
/// Sans `XDG_RUNTIME_DIR` ni variable explicite, la liste est vide : pas de session, pas de son.
pub fn audio_socket_candidates(
    xdg_runtime_dir: Option<&Path>,
    pulse_server: Option<&str>,
    pipewire_remote: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(remote) = pipewire_remote.filter(|p| p.is_absolute()) {
        out.push(remote.to_path_buf());
    }
    if let Some(server) = pulse_server {
        out.extend(
            server
                .split_whitespace()
                .filter_map(|alt| alt.strip_prefix("unix:"))
                .map(PathBuf::from),
        );
    }
    if let Some(runtime) = xdg_runtime_dir {
        out.push(runtime.join("pipewire-0"));
        out.push(runtime.join("pulse").join("native"));
    }
    out
}

/// L'état de la caméra à partir des noeuds sondés et du contexte.
///
/// - en sandbox : `prompt` si le portail est là (c'est lui qui demandera), sinon `unsupported` ;
/// - aucun noeud : `unsupported` ;
/// - un noeud ouvrable : `granted` ;
/// - tous refusés par le système : `denied` ;
/// - sinon (occupé, erreur inconnue) : `prompt`, c'est-à-dire « pas de réponse nette ».
pub fn camera_state(nodes: &[NodeAccess], ctx: Context) -> PermissionState {
    if ctx.sandboxed {
        return sandbox_state(ctx);
    }
    if nodes.is_empty() {
        PermissionState::Unsupported
    } else if nodes.contains(&NodeAccess::Openable) {
        PermissionState::Granted
    } else if nodes.iter().all(|n| *n == NodeAccess::PermissionDenied) {
        PermissionState::Denied
    } else {
        PermissionState::Prompt
    }
}

/// L'état du micro à partir des chemins de socket sondés et du contexte.
///
/// Hors sandbox, un serveur audio qui répond à la session donne `granted`, une socket
/// illisible `denied`, aucune socket `unsupported`. En sandbox : comme la caméra.
pub fn microphone_state(sockets: &[SocketAccess], ctx: Context) -> PermissionState {
    if ctx.sandboxed {
        return sandbox_state(ctx);
    }
    if sockets.contains(&SocketAccess::Present) {
        PermissionState::Granted
    } else if sockets.contains(&SocketAccess::PermissionDenied) {
        PermissionState::Denied
    } else {
        PermissionState::Unsupported
    }
}

/// En sandbox, seul le portail peut dire oui : sa présence est un `prompt`, son absence un mur.
fn sandbox_state(ctx: Context) -> PermissionState {
    if ctx.portal {
        PermissionState::Prompt
    } else {
        PermissionState::Unsupported
    }
}

/// Sonde un noeud V4L2 en l'ouvrant en lecture seule, puis en le refermant aussitôt.
///
/// Une ouverture en lecture seule ne démarre pas de flux (pas de VIDIOC_STREAMON). Certains
/// pilotes peuvent allumer le capteur à l'ouverture : à vérifier sur matériel réel.
fn probe_node(path: &Path) -> NodeAccess {
    match fs::OpenOptions::new().read(true).open(path) {
        Ok(_) => NodeAccess::Openable,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => NodeAccess::PermissionDenied,
        Err(_) => NodeAccess::Other,
    }
}

/// Sonde un chemin de socket par ses métadonnées seules : rien n'est connecté.
fn probe_socket(path: &Path) -> SocketAccess {
    match fs::metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => SocketAccess::Present,
        Ok(_) => SocketAccess::Absent,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => SocketAccess::PermissionDenied,
        Err(_) => SocketAccess::Absent,
    }
}

/// Liste `dir` et sonde chaque noeud vidéo. Un dossier illisible donne zéro noeud, donc
/// `unsupported` : on ne devine pas une caméra qu'on ne voit pas.
fn probe_cameras(dir: &Path) -> Vec<NodeAccess> {
    let names: Vec<String> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect(),
        Err(_) => Vec::new(),
    };
    select_video_nodes(dir, &names)
        .iter()
        .map(|path| probe_node(path))
        .collect()
}

/// Lit les variables de session et sonde les sockets audio candidates.
fn probe_audio() -> Vec<SocketAccess> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let pulse = std::env::var("PULSE_SERVER").ok();
    let pipewire = std::env::var_os("PIPEWIRE_REMOTE").map(PathBuf::from);
    audio_socket_candidates(runtime.as_deref(), pulse.as_deref(), pipewire.as_deref())
        .iter()
        .map(|path| probe_socket(path))
        .collect()
}

/// Le portail XDG est-il sur le bus session ? Borné : un bus muet est « non ».
async fn portal_present() -> bool {
    let probe = async {
        let conn = zbus::Connection::session()
            .await
            .map_err(|e| error_of(&e))?;
        let bus = zbus::Proxy::new(
            &conn,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await
        .map_err(|e| error_of(&e))?;
        let reply = bus
            .call_method("NameHasOwner", &("org.freedesktop.portal.Desktop",))
            .await
            .map_err(|e| error_of(&e))?;
        reply
            .body()
            .deserialize::<bool>()
            .map_err(|_| DeviceError::Internal)
    };
    bounded(BUS_STEP_TIMEOUT, probe).await.unwrap_or(false)
}

/// L'état d'une capacité de la famille, lu sans prompt ni capture. Les lectures de `/dev` sont
/// bloquantes : elles tournent sur un fil à part.
pub async fn state_of(capability: CaptureCapability) -> PermissionState {
    let sandboxed = is_sandboxed(
        Path::new("/.flatpak-info").exists(),
        std::env::var("SNAP").ok().as_deref(),
    );
    let ctx = Context {
        sandboxed,
        // Hors sandbox le portail ne change rien à la réponse : inutile de l'interroger.
        portal: sandboxed && portal_present().await,
    };
    let probed = tokio::task::spawn_blocking(move || match capability {
        CaptureCapability::Camera => camera_state(&probe_cameras(Path::new("/dev")), ctx),
        CaptureCapability::Microphone => microphone_state(&probe_audio(), ctx),
    })
    .await;
    probed.unwrap_or(PermissionState::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: Context = Context {
        sandboxed: false,
        portal: false,
    };

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_video_followed_by_digits_is_a_camera_node() {
        for ok in ["video0", "video1", "video12"] {
            assert!(is_video_node_name(ok), "{ok}");
        }
        for bad in [
            "video",
            "videox",
            "video-dec0",
            "videoo0",
            "vbi0",
            "v4l-subdev0",
            "",
        ] {
            assert!(!is_video_node_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn video_nodes_are_selected_and_sorted() {
        let dir = Path::new("/dev");
        let found = select_video_nodes(
            dir,
            &names(&["video10", "tty0", "video2", "video-dec0", "video0"]),
        );
        assert_eq!(
            found,
            vec![
                PathBuf::from("/dev/video0"),
                PathBuf::from("/dev/video10"),
                PathBuf::from("/dev/video2"),
            ]
        );
    }

    #[test]
    fn no_camera_node_is_unsupported() {
        assert_eq!(camera_state(&[], OPEN), PermissionState::Unsupported);
        let found = select_video_nodes(Path::new("/dev"), &names(&["sda", "null"]));
        assert!(found.is_empty());
    }

    #[test]
    fn one_openable_camera_is_granted() {
        assert_eq!(
            camera_state(&[NodeAccess::Openable], OPEN),
            PermissionState::Granted
        );
        // Un seul noeud utilisable suffit, même à côté de noeuds refusés.
        assert_eq!(
            camera_state(&[NodeAccess::PermissionDenied, NodeAccess::Openable], OPEN),
            PermissionState::Granted
        );
    }

    #[test]
    fn cameras_all_refused_by_the_system_are_denied() {
        assert_eq!(
            camera_state(
                &[NodeAccess::PermissionDenied, NodeAccess::PermissionDenied],
                OPEN
            ),
            PermissionState::Denied
        );
    }

    #[test]
    fn a_busy_or_unknown_camera_is_prompt_not_denied() {
        assert_eq!(
            camera_state(&[NodeAccess::Other], OPEN),
            PermissionState::Prompt
        );
        assert_eq!(
            camera_state(&[NodeAccess::PermissionDenied, NodeAccess::Other], OPEN),
            PermissionState::Prompt
        );
    }

    #[test]
    fn a_live_audio_socket_is_granted() {
        assert_eq!(
            microphone_state(&[SocketAccess::Present], OPEN),
            PermissionState::Granted
        );
        assert_eq!(
            microphone_state(&[SocketAccess::Absent, SocketAccess::Present], OPEN),
            PermissionState::Granted
        );
    }

    #[test]
    fn an_unreadable_audio_socket_is_denied() {
        assert_eq!(
            microphone_state(
                &[SocketAccess::PermissionDenied, SocketAccess::Absent],
                OPEN
            ),
            PermissionState::Denied
        );
    }

    #[test]
    fn no_audio_server_is_unsupported() {
        assert_eq!(microphone_state(&[], OPEN), PermissionState::Unsupported);
        assert_eq!(
            microphone_state(&[SocketAccess::Absent, SocketAccess::Absent], OPEN),
            PermissionState::Unsupported
        );
    }

    #[test]
    fn the_audio_candidates_follow_the_session_order() {
        let runtime = Path::new("/run/user/1000");
        let got = audio_socket_candidates(
            Some(runtime),
            Some("unix:/run/user/1000/pulse/custom"),
            Some(Path::new("/run/user/1000/pw-explicit")),
        );
        assert_eq!(
            got,
            vec![
                PathBuf::from("/run/user/1000/pw-explicit"),
                PathBuf::from("/run/user/1000/pulse/custom"),
                PathBuf::from("/run/user/1000/pipewire-0"),
                PathBuf::from("/run/user/1000/pulse/native"),
            ]
        );
    }

    #[test]
    fn a_relative_or_tcp_pulse_server_is_not_probed_as_a_socket() {
        let got = audio_socket_candidates(None, Some("tcp:host:4713"), Some(Path::new("rel")));
        assert!(got.is_empty());
    }

    #[test]
    fn without_a_runtime_dir_there_is_no_audio_candidate() {
        assert!(audio_socket_candidates(None, None, None).is_empty());
    }

    #[test]
    fn a_missing_path_is_absent_and_a_regular_file_is_not_a_socket() {
        assert_eq!(
            probe_socket(Path::new("/definitivement/absent/pipewire-0")),
            SocketAccess::Absent
        );
        assert_eq!(probe_socket(Path::new("/dev/null")), SocketAccess::Absent);
    }

    #[test]
    fn a_real_unix_socket_is_present() {
        // Chemin court : la limite de longueur d'une socket Unix est de l'ordre de 104 octets
        // sur macOS.
        let path = PathBuf::from(format!("/tmp/tdl-cm-{}.sock", std::process::id()));
        let _ = fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert_eq!(probe_socket(&path), SocketAccess::Present);
        drop(listener);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_sandbox_never_grants_what_only_the_portal_can_decide() {
        let with_portal = Context {
            sandboxed: true,
            portal: true,
        };
        let without = Context {
            sandboxed: true,
            portal: false,
        };
        // Même avec un noeud ouvrable ou une socket présente : le portail tranche, pas nous.
        assert_eq!(
            camera_state(&[NodeAccess::Openable], with_portal),
            PermissionState::Prompt
        );
        assert_eq!(
            microphone_state(&[SocketAccess::Present], with_portal),
            PermissionState::Prompt
        );
        assert_eq!(
            camera_state(&[NodeAccess::Openable], without),
            PermissionState::Unsupported
        );
        assert_eq!(
            microphone_state(&[SocketAccess::Present], without),
            PermissionState::Unsupported
        );
    }

    #[test]
    fn flatpak_and_snap_are_sandboxes_and_an_empty_snap_is_not() {
        assert!(is_sandboxed(true, None));
        assert!(is_sandboxed(false, Some("/snap/app/1")));
        assert!(!is_sandboxed(false, Some("")));
        assert!(!is_sandboxed(false, None));
    }
}
