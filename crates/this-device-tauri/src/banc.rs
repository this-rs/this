//! Le banc d'essai du pont : ce que l'appareil **devant soi** sait vraiment faire.
//!
//! Le contrat dit ce que le pont devrait savoir faire ; la carte du framework dit ce qu'il
//! annonce. Aucun des deux ne dit ce qui **répond** sur un iPhone donné, dans un build donné.
//! Mesuré le 13/11/2026 : `device_selftest` rendait un tableau vert des vingt et une capacités
//! parce qu'il ne lisait que des états de permission, et `faceIdConfirm` échouait en `internal`
//! sur une clé d'argument absente. Un voyant vert sur un chemin, un pont cassé sur l'autre.
//!
//! Ce module comble ce trou. Il ne décide rien (pas de consentement ici : ce n'est pas une
//! requête de module, c'est un diagnostic demandé par la personne qui tient l'appareil), il ne
//! touche à rien tout seul (la coquille l'appelle explicitement), et il range chaque méthode
//! selon ce qu'elle ferait si on l'appelait :
//!
//! - [`Nature::Lecture`] — lit un état ou une donnée. Le banc l'appelle.
//! - [`Nature::Permission`] — afficherait une demande système. Le banc ne l'appelle pas : la
//!   question doit être posée par un geste, pas par un diagnostic.
//! - [`Nature::Interactif`] — ouvrirait un sélecteur, l'appareil photo, une feuille de partage,
//!   une session NFC. **Mesuré le 13/11/2026** : appelé sans main devant l'écran, il ne rend
//!   jamais la main non plus — l'app entière reste suspendue sur l'attente.
//! - [`Nature::Ecriture`] — modifierait une donnée de la personne, ou en créerait une.
//!
//! Le banc est donc **complet dans son plan** (chaque méthode du contrat y figure) et
//! **prudent dans son exécution** (seules les lectures sont appelées). Ce qu'il n'exerce pas, il
//! le dit, au lieu de le compter comme réussi.
//!
//! Compilé seulement avec la feature `banc` : ce n'est pas une surface de production.

use serde_json::{Value, json};
use this_platform::capability::Capability;
use this_platform::device_bridge::{
    METHODS, PermissionState, Request, capability_of, complete_args, status_method,
};

/// Ce qu'une méthode ferait si on l'appelait, et donc si le banc peut l'appeler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nature {
    Lecture,
    Permission,
    Interactif,
    Ecriture,
}

impl Nature {
    /// Le nom de fil de la nature, pour le rapport.
    pub fn wire(self) -> &'static str {
        match self {
            Nature::Lecture => "lecture",
            Nature::Permission => "permission",
            Nature::Interactif => "interactif",
            Nature::Ecriture => "ecriture",
        }
    }

    /// Seules les lectures sont exercées sans personne devant l'écran.
    pub fn executable(self) -> bool {
        self == Nature::Lecture
    }
}

/// L'état d'une méthode après l'avoir appelée, ou la raison de ne pas l'avoir fait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Etat {
    /// L'appareil a répondu autre chose qu'un refus.
    Repondu,
    /// La personne n'a pas accordé cette capacité (au module, ou au système).
    RefuseAutorisation,
    /// Le pont a refusé avant le natif : un argument requis manque.
    RefuseArgument,
    /// Le matériel ou le système ne sait pas faire ça ici.
    Indisponible,
    /// La méthode n'existe pas sur cette plateforme.
    NonSupporte,
    /// Le pont a échoué : c'est un défaut, pas une réponse de l'appareil.
    Panne,
    /// Le natif n'a pas répondu dans la borne.
    Delai,
    /// La famille a refusé avec SON mot (`titre_absent`, `unknown_activity`, `too_large`…).
    /// C'est une réponse : la confondre avec une panne du pont faisait porter au pont des
    /// refus qui viennent du contrat de la famille.
    Refus,
    /// Le banc ne l'a pas appelée, par choix (voir [`Nature`]).
    NonExerce,
}

impl Etat {
    pub fn wire(self) -> &'static str {
        match self {
            Etat::Repondu => "repondu",
            Etat::RefuseAutorisation => "refuse_autorisation",
            Etat::RefuseArgument => "refuse_argument",
            Etat::Indisponible => "indisponible",
            Etat::NonSupporte => "non_supporte",
            Etat::Panne => "panne",
            Etat::Delai => "delai",
            Etat::Refus => "refus",
            Etat::NonExerce => "non_exerce",
        }
    }

    /// Un état qui demande une correction du pont, par opposition à une réponse honnête de
    /// l'appareil. C'est la seule colonne qui doit rester vide sur un appareil sain.
    ///
    /// `refuse_argument` n'en est PAS un — et c'est une correction de ce fichier, mesurée le
    /// 13/11/2026 : le banc appelle chaque méthode sans argument (c'est la situation d'un agent
    /// qui découvre l'outil), donc toute méthode dont un argument ne se remplace pas
    /// honnêtement (`contactsSearch` sans `query`, `filesRead` sans `bookmark`) répond
    /// légitimement `missing_argument`. Compter ça comme un défaut faisait afficher « 1 défaut »
    /// sur un pont sain, et noyait les vrais.
    pub fn is_defect(self) -> bool {
        matches!(self, Etat::Panne | Etat::Delai)
    }
}

/// La nature d'une méthode du contrat.
///
/// La règle est lisible : ce qui **montre** quelque chose à la personne (une question, un
/// sélecteur, une caméra) n'est pas une lecture ; ce qui **change** une donnée non plus. Tout le
/// reste est une lecture, et c'est ce que le banc exerce.
pub fn nature(method: &str) -> Nature {
    match method {
        // Demandes système : elles affichent une boîte de dialogue.
        "captureRequest"
        | "locationRequest"
        | "healthRequest"
        | "contactsRequest"
        | "photosRequest"
        | "calendarRequest"
        | "speechRequest"
        | "motionActivityRequest"
        | "notificationsRequest" => Nature::Permission,
        // Sélecteurs, caméra, micro, partage, NFC, Face ID : ils attendent un geste.
        "photosPick"
        | "filesPick"
        | "shareText"
        | "shareItem"
        | "nfcReadOnce"
        | "nfcWrite"
        | "faceIdConfirm"
        | "cameraCapturePhoto"
        | "cameraRecordVideo"
        | "microphoneStart"
        | "locationStreamStart"
        // `clipboardRead` LIT, mais depuis iOS 16 il peut afficher la demande de collage du
        // système — un geste, donc. Mesuré le 13/11/2026 : réponse en 6 ms sur iOS 18.6, et
        // **délai de 10 002 ms** sur les deux simulateurs iOS 26 (la boîte attendait une main).
        // Sur l'iPhone, 4 172 ms : la personne avait déjà répondu une fois. Le classer lecture
        // laissait une boîte de dialogue sur l'écran de quelqu'un à chaque balayage.
        | "clipboardRead" => Nature::Interactif,
        // `locationSample` n'est PAS listée : depuis que `locationStatus` existe
        // (`status_method(Location) = Some("locationStatus")`), la règle générale du balayage
        // s'applique — le banc lit l'état d'abord, et n'appelle la position que si la permission
        // est déjà accordée. C'est un cas où ajouter une méthode d'état rend une autre méthode
        // mesurable : sans elle, l'appel aurait posé la question sur l'écran de quelqu'un.
        // Écritures, ou appels qui n'ont de sens qu'avec un périphérique déjà choisi.
        "microphoneStop"
        | "locationStreamStop"
        | "clipboardWrite"
        | "filesWrite"
        | "contactsCreate"
        | "calendarCreateEvent"
        | "calendarUpdateEvent"
        | "calendarDeleteEvent"
        | "notificationsPost"
        | "notificationsCancel"
        | "notificationsSchedule"
        | "liveActivityStart"
        | "liveActivityUpdate"
        | "liveActivityEnd"
        | "bluetoothConnect"
        | "bluetoothDisconnect"
        | "bluetoothWrite"
        | "bluetoothSubscribe"
        | "bluetoothRead"
        | "bluetoothNotifications"
        | "bluetoothServices"
        | "localNetworkResolve"
        // Annoncer sa présence sur le réseau modifie ce que les autres voient : c'est une
        // écriture, pas une lecture. Le banc ne l'appelle donc pas.
        | "localNetworkAdvertise"
        | "localNetworkUnadvertise"
        | "speechTranscribeFile" => Nature::Ecriture,
        // `status`, `…Status`, `…List`, `…Sample`, `…Browse`, `…Recent`, `…Pending`,
        // `gyroscopeSample`, `bluetoothScan`, les six lectures d'état système : rien ne s'affiche,
        // rien ne change.
        _ => Nature::Lecture,
    }
}

/// Un cas du plan : une méthode, ses arguments complets, et ce qu'elle ferait.
#[derive(Clone, Debug)]
pub struct Cas {
    pub method: &'static str,
    pub args: Value,
    pub nature: Nature,
}

/// Le plan complet : **toutes** les méthodes du contrat, dans l'ordre de [`METHODS`].
///
/// Les arguments sont ceux que le pont remplirait lui-même pour un appelant qui n'en donne
/// aucun — c'est exactement la situation d'un agent qui découvre l'outil.
/// `status` n'est pas du plan des **familles** : c'est `Request::Status`, et il est balayé
/// capacité par capacité par [`capacites`]. Le mettre ici l'enverrait en `Request::Call`, où il
/// n'a pas de capacité dérivable, et le pont le refuserait `not_supported` — un faux défaut.
pub fn plan(now_ms: f64) -> Vec<Cas> {
    METHODS
        .iter()
        .filter(|method| **method != "status")
        .map(|method| Cas {
            method,
            args: complete_args(method, &json!({}), now_ms),
            nature: nature(method),
        })
        .collect()
}

/// Les vingt et une capacités du catalogue : leur état de permission, tel que le système le
/// donne sans rien demander à personne.
pub fn capacites() -> Vec<Capability> {
    Capability::ALL.to_vec()
}

/// Comment lire une réponse du pont.
///
/// Une erreur est une réponse : le pont ne rejette pas. Donc tout ce qui compte est dans
/// l'objet — et un `code` y est toujours la raison.
pub fn etat(reply: &Value) -> Etat {
    if let Some(code) = reply.get("code").and_then(Value::as_str) {
        return match code {
            "not_authorized" => Etat::RefuseAutorisation,
            "missing_argument" => Etat::RefuseArgument,
            "not_supported" => Etat::NonSupporte,
            "unavailable" | "unsupported" => Etat::Indisponible,
            "timeout" => Etat::Delai,
            // `internal` est le seul code qui veut dire « le pont a échoué ».
            "internal" => Etat::Panne,
            // Tout le reste est le mot de la famille : une réponse, avec sa raison.
            _ => Etat::Refus,
        };
    }
    // `Reply::Status` d'une capacité sans état : `unsupported` est un état, pas une panne.
    if reply.get("state").and_then(Value::as_str) == Some("unsupported") {
        return Etat::NonSupporte;
    }
    Etat::Repondu
}

/// La requête d'un cas, telle que la coquille l'enverra.
pub fn request(case: &Cas) -> Request {
    Request::Call {
        method: case.method.to_string(),
        args: case.args.clone(),
    }
}

/// Une écriture que le banc peut exercer : elle ne demande aucune main, et elle est nommée.
///
/// Le prix d'une écriture est réel — elle crée un objet sur l'appareil de quelqu'un — donc elle
/// est **opt-in** (`THIS_DEVICE_BANC_ECRITURES=1`), et chaque cas dit ce qu'il laisse derrière
/// lui. Là où le contrat offre de quoi effacer (une notification, une Live Activity, un
/// événement), le nettoyage est dans la liste et s'exécute après la vérification.
pub struct Ecriture {
    /// Le groupe : `sures` (un fichier, une notification, une activité — rien de la personne)
    /// ou `visibles` (un contact, un événement de calendrier — ils restent dans son carnet).
    pub groupe: Groupe,
    pub method: &'static str,
    pub args: Value,
    /// Ce qu'on appelle après, pour vérifier ou pour effacer.
    pub suite: Vec<(&'static str, Value)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Groupe {
    Sures,
    Visibles,
}

impl Groupe {
    pub fn wire(self) -> &'static str {
        match self {
            Groupe::Sures => "sures",
            Groupe::Visibles => "visibles",
        }
    }
}

/// Les écritures du banc, dans un ordre qui se suit : écrire, vérifier, effacer.
///
/// Ce qui n'y est **pas** et n'y sera pas : tout ce qui ouvrirait une interface — un sélecteur
/// (`photosPick`, `filesPick`), une feuille de partage (`shareText`, `shareItem`), une session
/// NFC, la caméra, le micro, une confirmation Face ID. Aucune écriture ne peut les remplacer :
/// c'est la personne qui les termine.
pub fn ecritures() -> Vec<Ecriture> {
    vec![
        Ecriture {
            groupe: Groupe::Sures,
            method: "filesWrite",
            // Un fichier d'une poignée d'octets, dans le conteneur de l'app : il ne touche à
            // rien de la personne et il prouve le chemin disque + base64 de bout en bout.
            args: json!({ "name": "banc-appareil.txt", "data_base64": "YmFuYy1hcHBhcmVpbA==" }),
            suite: Vec::new(),
        },
        Ecriture {
            groupe: Groupe::Sures,
            method: "notificationsSchedule",
            // Dans une heure, et retirée juste après : si le nettoyage échoue, personne ne la
            // verra jamais. Un identifiant fixe pour que le nettoyage soit sûr.
            args: json!({
                "title": "banc appareil",
                "body": "mesure du pont",
                "id": "banc-appareil",
                "in_ms": 3_600_000,
            }),
            suite: vec![
                ("notificationsPending", json!({})),
                ("notificationsCancel", json!({ "id": "banc-appareil" })),
            ],
        },
        Ecriture {
            groupe: Groupe::Sures,
            method: "liveActivityStart",
            args: json!({ "id": "banc-appareil", "titre": "banc appareil", "detail": "mesure", "progression": 0.0 }),
            suite: vec![
                (
                    "liveActivityUpdate",
                    json!({ "id": "banc-appareil", "titre": "banc appareil", "progression": 0.5 }),
                ),
                ("liveActivityEnd", json!({ "id": "banc-appareil" })),
            ],
        },
        Ecriture {
            groupe: Groupe::Visibles,
            method: "contactsCreate",
            // Un contact reconnaissable, et la recherche qui prouve qu'il est là. Le contrat
            // n'a pas de `contactsDelete` : il RESTE dans le carnet. D'où le groupe `visibles`.
            args: json!({ "name": "Banc Appareil", "emails": [], "phones": [] }),
            suite: vec![(
                "contactsSearch",
                json!({ "query": "Banc Appareil", "limit": 5 }),
            )],
        },
        Ecriture {
            groupe: Groupe::Visibles,
            method: "calendarCreateEvent",
            // Dans une semaine, une heure, et supprimé juste après : il n'apparaît nulle part
            // si le nettoyage passe.
            args: json!({
                "title": "Banc Appareil",
                "start_ms": 1_800_000_000_000_i64,
                "end_ms": 1_800_003_600_000_i64,
            }),
            suite: Vec::new(),
        },
    ]
}

/// La passe d'écriture, **opt-in** : `THIS_DEVICE_BANC_ECRITURES=sures` écrit un fichier, une
/// notification et une Live Activity ; `=visibles` ajoute un contact et un événement de
/// calendrier. Absente, la passe ne fait rien et le dit.
async fn balayer_ecritures<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<Value> {
    let demande = std::env::var("THIS_DEVICE_BANC_ECRITURES").unwrap_or_default();
    let visibles = demande == "visibles" || demande == "tout";
    let mut out = Vec::new();
    if demande.is_empty() {
        out.push(json!({ "groupe": "—", "etat": "non_exerce", "raison": "THIS_DEVICE_BANC_ECRITURES absent" }));
        return out;
    }
    for cas in ecritures() {
        if cas.groupe == Groupe::Visibles && !visibles {
            out.push(
                json!({ "methode": cas.method, "groupe": cas.groupe.wire(), "etat": "non_exerce" }),
            );
            continue;
        }
        for (method, args) in
            std::iter::once((cas.method, cas.args.clone())).chain(cas.suite.clone())
        {
            let requete = Request::Call {
                method: method.to_string(),
                args,
            };
            let reply = crate::answer(app, &requete).await;
            let mut entree = json!({
                "methode": method,
                "groupe": cas.groupe.wire(),
                "etat": etat(&reply).wire(),
                "forme": reply.get("kind").and_then(Value::as_str),
            });
            if let Some(code) = reply.get("code").and_then(Value::as_str) {
                entree["code"] = json!(code);
            }
            out.push(entree);
        }
    }
    out
}

/// Le rapport complet : l'état des capacités, ce que chaque méthode a répondu, et les défauts.
///
/// **Rien de ce que l'appareil rend n'est recopié ici** : ni un contact, ni une photo, ni un
/// texte du presse-papiers. Un diagnostic qui recopierait la donnée de la personne dans un
/// fichier serait exactement ce que la porte de consentement existe pour empêcher.
pub async fn balayer<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Value {
    let capacites = balayer_capacites(app).await;
    let methodes = balayer_methodes(app, &capacites).await;
    let systeme = balayer_systeme(app).await;
    let ecritures = balayer_ecritures(app).await;

    let mut compte: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for entree in &methodes {
        let etat = entree["etat"].as_str().unwrap_or("?").to_string();
        *compte.entry(etat).or_default() += 1;
    }
    let defauts: Vec<Value> = methodes
        .iter()
        .filter(|entree| {
            entree["etat"]
                .as_str()
                .and_then(est_defaut)
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    json!({
        "kind": "banc",
        "capacites": capacites,
        "methodes": methodes,
        "systeme": systeme,
        "ecritures": ecritures,
        "compte": compte,
        "defauts": defauts,
    })
}

/// Les états qui demandent une correction du pont, par leur nom de fil.
fn est_defaut(nom: &str) -> Option<bool> {
    [
        Etat::Repondu,
        Etat::RefuseAutorisation,
        Etat::RefuseArgument,
        Etat::Indisponible,
        Etat::NonSupporte,
        Etat::Panne,
        Etat::Delai,
        Etat::Refus,
        Etat::NonExerce,
    ]
    .into_iter()
    .find(|etat| etat.wire() == nom)
    .map(Etat::is_defect)
}

/// L'état de chaque capacité du catalogue. Aucune de ces lectures ne demande rien : c'est le
/// seul chemin qui ne peut pas laisser de question sur l'écran.
async fn balayer_capacites<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<Value> {
    let mut out = Vec::new();
    for capability in capacites() {
        // « Aucun état à lire » n'est pas « absente ». Un sélecteur, le presse-papiers et la
        // feuille de partage n'ont pas de permission système : `perform` répond `unsupported`
        // pour eux, et recopier ce mot ferait déclarer `device.photos_pick` indisponible sur
        // TOUS les appareils (c'est ce que `HostLimits::from_bench` en ferait).
        if status_method(capability).is_none() {
            out.push(json!({
                "capacite": serde_json::to_value(capability).unwrap_or(Value::Null),
                "etat": "sans_etat",
            }));
            continue;
        }
        let reply = crate::answer(app, &Request::Status { capability }).await;
        let etat = reply
            .get("state")
            .and_then(|state| serde_json::from_value::<PermissionState>(state.clone()).ok())
            .unwrap_or(PermissionState::Unsupported);
        out.push(json!({
            "capacite": serde_json::to_value(capability).unwrap_or(Value::Null),
            "etat": serde_json::to_value(etat).unwrap_or(Value::Null),
        }));
    }
    out
}

/// Chaque méthode du plan, et l'appel réel pour celles qui sont exécutables.
async fn balayer_methodes<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    capacites: &[Value],
) -> Vec<Value> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duree| duree.as_millis() as f64)
        .unwrap_or(0.0);
    let mut out = Vec::new();
    for case in plan(now_ms) {
        let mut entree = json!({
            "methode": case.method,
            "nature": case.nature.wire(),
            "etat": Etat::NonExerce.wire(),
        });
        if !case.nature.executable() {
            out.push(entree);
            continue;
        }
        // Une lecture de donnée personnelle poserait la question système si elle n'est pas
        // accordée. On lit d'abord l'état — qui ne demande rien — et on n'appelle que si la
        // réponse est déjà là. Un diagnostic ne doit pas se transformer en formulaire.
        if let Some(capability) = capability_of(case.method, &case.args)
            && status_method(capability).is_some_and(|method| method != case.method)
        {
            let wire = serde_json::to_value(capability).unwrap_or(Value::Null);
            let state = capacites
                .iter()
                .find(|entree| entree["capacite"] == wire)
                .and_then(|entree| entree["etat"].as_str())
                .unwrap_or("unsupported")
                .to_string();
            if !matches!(state.as_str(), "granted" | "limited") {
                let etat = match state.as_str() {
                    "denied" => Etat::RefuseAutorisation,
                    "prompt" => Etat::NonExerce,
                    _ => Etat::NonSupporte,
                };
                entree["etat"] = json!(etat.wire());
                entree["attente"] = json!(state);
                out.push(entree);
                continue;
            }
        }
        let debut = std::time::Instant::now();
        let reply = crate::answer(app, &request(&case)).await;
        entree["etat"] = json!(etat(&reply).wire());
        entree["ms"] = json!(debut.elapsed().as_millis() as u64);
        if let Some(code) = reply.get("code").and_then(Value::as_str) {
            entree["code"] = json!(code);
        }
        if let Some(kind) = reply.get("kind").and_then(Value::as_str) {
            entree["forme"] = json!(kind);
        }
        out.push(entree);
    }
    out
}

/// Les six lectures d'état système : sans consentement, sans question, et pourtant capables de
/// répondre `internal` sur un appareil où elles ne sont pas liées.
async fn balayer_systeme<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<Value> {
    let mut out = Vec::new();
    for method in crate::SYSTEM_METHODS {
        let reply = crate::system_state(app, method).await;
        out.push(json!({
            "methode": method,
            "etat": etat(&reply).wire(),
            "forme": reply.get("kind").and_then(Value::as_str),
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Le plan nomme CHAQUE méthode du contrat, une fois. Une méthode oubliée est une méthode
    /// dont personne ne saura jamais si elle répond.
    ///
    /// `status` est la seule exception, et elle est nommée : il n'est pas une famille mais
    /// `Request::Status`, balayé capacité par capacité (`balayer_capacites`), et l'appeler par
    /// `Request::Call` n'aurait pas de capacité dérivable.
    #[test]
    fn the_plan_names_every_method_of_the_contract_once() {
        let plan = plan(1_760_000_000_000.0);
        let named: BTreeSet<&str> = plan.iter().map(|case| case.method).collect();
        assert_eq!(named.len(), plan.len(), "une méthode est nommée deux fois");
        for method in METHODS.iter().filter(|method| **method != "status") {
            assert!(named.contains(method), "{method} absente du plan");
        }
        assert_eq!(named.len(), METHODS.len() - 1);
        assert!(
            !named.contains("status"),
            "status n'est pas une méthode de famille"
        );
    }

    /// Le banc n'exerce que des lectures : tout ce qui ouvre une question, un sélecteur ou une
    /// écriture est déclaré mais pas appelé.
    #[test]
    fn only_reads_are_exercised() {
        for case in plan(0.0) {
            if case.nature.executable() {
                assert_eq!(case.nature, Nature::Lecture, "{}", case.method);
            }
        }
        // Et les cas qui ont un effet ne sont pas classés lecture par distraction.
        for method in [
            "faceIdConfirm",
            "photosPick",
            "notificationsPost",
            "captureRequest",
            "cameraCapturePhoto",
        ] {
            assert!(
                !nature(method).executable(),
                "{method} ne doit pas être exercée"
            );
        }
    }

    /// La lecture de `notificationsStatus` ne saute pas sur la file principale : sa réponse ne
    /// doit dépendre de rien d'autre que du callback du système. Mesuré sur un appareil : un appel
    /// direct ne répondait jamais (garde-temps de 300 s) alors que la même lecture par l'autotest
    /// rendait `prompt`. Les autres lectures d'état répondent sans file principale.
    #[test]
    fn the_notification_status_read_does_not_hop_to_the_main_queue() {
        let source = include_str!("../ios/Sources/notifications.swift");
        let debut = source
            .find("static func read(")
            .expect("NotificationStatus.read existe");
        let corps = &source[debut..];
        let fin = corps.find("\n  }\n").expect("fin de read");
        assert!(
            !corps[..fin].contains("DispatchQueue.main"),
            "NotificationStatus.read ne doit pas dépendre de la file principale"
        );
    }

    /// Un état de permission est une lecture : la lire ne demande rien à personne, donc le banc
    /// peut le faire — et c'est ce qui distingue un appareil sans capteur d'un pont cassé.
    #[test]
    fn reading_a_permission_state_is_a_read() {
        for method in [
            "status",
            "captureStatus",
            "contactsStatus",
            "photosStatus",
            "calendarStatus",
            "healthStatus",
            "faceIdStatus",
            "speechStatus",
            "nfcStatus",
            "localNetworkStatus",
            "motionActivityStatus",
            "notificationsStatus",
            "liveActivityStatus",
        ] {
            assert_eq!(nature(method), Nature::Lecture, "{method}");
        }
    }

    /// Le classement d'une réponse : `internal` est un défaut, `not_authorized` et `unsupported`
    /// sont des réponses.
    #[test]
    fn an_internal_error_is_a_defect_and_a_refusal_is_an_answer() {
        assert_eq!(
            etat(&json!({"kind":"error","code":"internal"})),
            Etat::Panne
        );
        assert_eq!(etat(&json!({"kind":"error","code":"timeout"})), Etat::Delai);
        assert_eq!(
            etat(&json!({"kind":"error","code":"not_authorized"})),
            Etat::RefuseAutorisation
        );
        assert_eq!(
            etat(&json!({"kind":"error","code":"missing_argument"})),
            Etat::RefuseArgument
        );
        assert_eq!(
            etat(&json!({"kind":"error","code":"unsupported"})),
            Etat::Indisponible
        );
        assert_eq!(
            etat(&json!({"kind":"status","state":"unsupported"})),
            Etat::NonSupporte
        );
        assert_eq!(
            etat(&json!({"kind":"status","state":"granted"})),
            Etat::Repondu
        );
        assert_eq!(
            etat(&json!({"kind":"contacts","contacts":[]})),
            Etat::Repondu
        );
        assert!(Etat::Panne.is_defect());
        assert!(!Etat::RefuseAutorisation.is_defect());
        assert!(!Etat::NonSupporte.is_defect());
    }

    /// Un refus d'argument est le pont qui fait son travail : il a refusé AVANT le natif, en
    /// nommant la clé. Ce n'est pas un défaut — sinon un pont sain affiche « 1 défaut » pour
    /// `filesRead`, qui ne peut pas se lire sans le `bookmark` d'un sélecteur.
    #[test]
    fn a_missing_argument_is_an_answer_not_a_defect() {
        assert!(!Etat::RefuseArgument.is_defect());
        assert_eq!(
            etat(&json!({"kind":"error","code":"titre_absent"})),
            Etat::Refus
        );
        assert!(!Etat::Refus.is_defect());
        assert!(Etat::Panne.is_defect());
        assert_eq!(
            etat(&json!({"kind":"error","code":"missing_argument","missing":["reason"]})),
            Etat::RefuseArgument
        );
    }

    /// « Aucun état à lire » n'est pas « absente » : les capacités sans permission système ne
    /// doivent jamais sortir `unsupported`, sinon `HostLimits::from_bench` les retirerait de la
    /// carte de l'agent sur tous les appareils.
    #[test]
    fn a_capability_without_a_state_is_not_reported_absent() {
        for capability in capacites() {
            if status_method(capability).is_some() {
                continue;
            }
            assert!(
                matches!(
                    capability,
                    Capability::PhotosPick
                        | Capability::FilesPick
                        | Capability::Clipboard
                        | Capability::Share
                ),
                "{capability:?} n'a pas de méthode d'état et n'est pas un sélecteur : à vérifier"
            );
        }
    }

    /// `location` avait ce défaut : aucune méthode d'état, donc `unsupported` sur un iPhone qui
    /// a la localisation. Il a maintenant une méthode, et elle lit sa capacité dans ses
    /// arguments comme `captureStatus`.
    #[test]
    fn every_location_capability_has_a_state_to_read() {
        for capability in [Capability::Location, Capability::LocationAlways] {
            assert_eq!(
                status_method(capability),
                Some("locationStatus"),
                "{capability:?}"
            );
        }
        assert_eq!(
            capability_of("locationStatus", &json!({"capability": "location_always"})),
            Some(Capability::LocationAlways)
        );
    }
}
