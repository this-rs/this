# Le maillage — des appareils qui se reconnaissent par leur clé

Le maillage existe pour que deux appareils d'une même maison (mon Mac, mon
iPhone, le téléphone d'un proche) se parlent **sans serveur** : chacun se
présente par sa clé, chacun décide seul de ce qu'il accepte. Pas de compte, pas
d'autorité, pas de nuage au milieu.

Deux crates, et une règle : **le refus est la règle**, tout ce qui n'est pas
explicitement admis ne passe pas.

| Couche | Où | Rôle |
|---|---|---|
| **Le protocole** | `crates/this-mesh/src/` (sans la feature `transport`) | l'enveloppe signée, la forme canonique, le registre des pairs (`Peers`), la porte (`Gate`), le code court et la machine d'états de l'appairage. Aucune I/O, compilable pour wasm. |
| **Le transport** | `crates/this-mesh/src/transport/` (feature `transport`, éteinte par défaut) | QUIC (quinn 0.11) en TLS 1.3 mutuel, certificat auto-signé épinglé, appairage par le réseau, appel de service. |
| **Les droits** | `crates/this-descriptor/src/perm.rs` | l'évaluation d'une `PolicyExpr` pour un appelant du maillage (`Sujet`, `autorise`, `autorise_op`, `roles_de`). |

Le **modèle** des droits (personnes, services offerts, rôles accordés) vit dans
l'application : `atelier/src-tauri/src/droits.rs` (dépôt `atelier`). Le framework
fournit l'évaluation, pas le modèle.

## Ce que le maillage ne fait pas

Dit d'emblée, pour ne pas le découvrir en production :

- **Pas de traversée de NAT, pas de relais.** Les deux appareils doivent se
  joindre directement (même réseau local, en pratique). Le repli sur quinn est
  écrit dans `crates/this-mesh/Cargo.toml` : iroh, qui apporte relais et
  traversée, ne compile pas pour iOS.
- **IPv4 seulement en v1.** `adresses_d_ecoute` écarte l'IPv6 (adresses
  éphémères, portée `%en0` des lien-local), et le client d'`envoyer`/`appeler`
  lie son endpoint sur `0.0.0.0:0` : il ne joint pas une adresse IPv6. Seul
  `appairer_vers` sait ouvrir un endpoint `[::]:0`.
- **Pas de découverte.** Le maillage reçoit une adresse (`SocketAddr`) ; la
  trouver (Bonjour, saisie, QR) est l'affaire de l'appelant.
- **Pas de streaming des réponses** : voir § Limites connues.

## Identité

### La clé d'appareil

Chaque appareil a **une** clé Ed25519 (`ed25519_dalek::SigningKey`). Elle sert à
tout : signer les messages, et porter le certificat TLS. Aucun autre secret
n'est généré.

### `DeviceId` : une étiquette

`DeviceId` a la forme `device-` suivi de 8 hexadécimaux minuscules
(`DeviceId::parse`). `DeviceId::de_cle` le dérive d'une clé : `device-` suivi
des 4 premiers octets de la clé publique.

Ce n'est **qu'une étiquette** (`envelope.rs`) : la confiance vient de la clé
enregistrée sous cet identifiant dans le registre, jamais de la forme du nom.
Quatre octets ne sont pas une preuve (voir § Points ouverts).

### Le certificat dont la clé publique EST la clé d'appareil

`Identite::de_cle` (`transport/tls.rs`) fabrique un certificat **auto-signé** :
sa SPKI est la clé Ed25519 de l'appareil, son `CommonName` est l'identifiant.
Pas de PKI, pas de magasin de racines.

La poignée de main TLS 1.3 est signée par cette clé, des **deux** côtés (TLS
mutuel : le serveur exige un certificat client). Présenter un certificat qui
porte la clé attendue ne suffit donc pas : il faut détenir la clé privée, et
rustls vérifie cette signature après que le vérificateur a regardé la clé.
TLS 1.2 est refusé.

### L'épinglage

Le client épingle le serveur sur sa **clé publique complète** (`Epingle::cle`) :
`VerifierServeur` n'accepte que le certificat dont la clé Ed25519 est
exactement celle attendue. On n'épingle ni un nom ni une autorité.

Côté serveur, le vérificateur TLS n'accepte que des clés Ed25519 bien formées ;
la décision « clé connue / révoquée » revient à la porte (`Gate`), source
unique, et est aussi faite à l'acceptation de chaque connexion.

### Pourquoi une empreinte n'est qu'un affichage

`Identite::empreinte` rend le SHA-256 de la clé publique. Sa documentation le
dit : « pour les journaux, jamais comparée pour décider — la décision se fait
sur la clé complète ». L'épinglage, le registre (`Peers::id_de_cle`) et la porte
comparent tous les 32 octets de la clé, jamais un résumé.

## Appairage

L'appairage mène la clé d'un inconnu jusqu'au registre, **sans rien écrire
avant les deux confirmations**. Il imite le « Numeric Comparison » du Bluetooth
SSP : les deux écrans affichent un code, l'humain compare, chacun dit oui.

### Un ALPN dédié, sur le même port

Une connexion d'appairage négocie l'ALPN `this-mesh-pair/1` (`ALPN_APPAIRAGE`),
distinct de `this-mesh/1` (`ALPN`) qui porte les messages. Le serveur aiguille
sur l'ALPN **négocié**, jamais sur le contenu d'un message : une connexion
d'appairage ne voit ni la porte ni le flux.

C'est la seule connexion où le client n'épingle rien (la clé du serveur n'est
pas encore connue) : `VerifierAppairage` accepte tout certificat Ed25519 bien
formé, mais vérifie toujours la signature de la poignée de main. Ce
vérificateur est privé et la configuration qui l'utilise n'offre **que**
l'ALPN d'appairage : rustls refuse un ALPN non offert, une telle connexion ne
peut donc pas aboutir en `this-mesh/1`.

### La séquence sur le fil

Un seul flux bidirectionnel, des messages cadrés (longueur sur 4 octets
big-endian, puis le JSON), chacun plafonné à 1024 octets (`PLAFOND`, dans
`transport/appairage.rs`) et refusé avant allocation au-delà. Chaque demi-nonce
fait 32 octets (`TAILLE_NONCE`), en hexadécimal sur le fil.

```text
I (initiateur, client)                          R (répondeur, serveur)
  tire nI
  {"t":"engage","h":"<hex SHA256(D ‖ nI)>"}  ──▶
                                             ◀──  tire nR (après l'engagement)
                                                  {"t":"nonce","n":"<hex nR>"}
  {"t":"revele","n":"<hex nI>"}              ──▶  vérifie SHA256(D ‖ nI) = h,
                                                  sinon abandon
  code = sas(moi, pair, nI ‖ nR)                  code = sas(moi, pair, nI ‖ nR)
           ── les deux écrans affichent le code, l'humain compare ──
  {"t":"confirme","lien":"moi"|"autre"}      ◀─▶  {"t":"confirme","lien":"moi"|"autre"}
  ou {"t":"refuse"}                               ou {"t":"refuse"}
```

`D` est `ENGAGEMENT_DOMAIN` (`b"this-mesh/pair/commit/v1"`). Les deux clés qui
entrent dans le code sont **celles du canal** : la mienne, et celle du
certificat TLS du pair, dont la poignée de main a prouvé la détention. Aucune
clé n'est jamais lue dans un message.

### Le code court (`SAS_DIGITS` chiffres)

`sas(a, b, nonce)` (`appairage.rs`) : HMAC-SHA256 avec pour clé `SAS_DOMAIN`
(`b"this-mesh/sas/v1"`), sur `min(clés) ‖ max(clés) ‖ nonce`. Les 8 premiers
octets du résultat, lus en entier big-endian, sont réduits modulo
10^`SAS_DIGITS` et complétés de zéros : `SAS_DIGITS` vaut 6. Les clés sont
triées, donc les deux côtés calculent le même code quel que soit l'ordre ; le
nonce `nI ‖ nR` rend deux appairages simultanés distincts.

### Pourquoi l'engagement

Un attaquant placé au milieu tient deux canaux : I↔attaquant et
attaquant↔R, donc deux paires de clés, donc en principe deux codes différents
que l'humain verra. Mais si les nonces étaient échangés **en clair et dans
n'importe quel ordre**, l'attaquant connaîtrait tout ce qui entre dans le code
d'un côté avant de choisir ce qu'il envoie de l'autre : il pourrait essayer
des clés (ou des nonces) hors ligne jusqu'à ce que les deux codes coïncident, ce
qui ne demande qu'environ 10^`SAS_DIGITS` essais. L'engagement l'en empêche :
I fige nI (`engage`) avant que R ne choisisse nR, et R ne révèle rien qui
dépende de nI. Quand nI est révélé, tout est déjà fixé ; il ne reste à
l'attaquant qu'**une** chance sur 10^`SAS_DIGITS` que les deux codes tombent
pareil.

### Deux confirmations, et le délai

La machine d'états pure (`Appairage` : `Rencontre` → `SasAttendu` → `Appaire`)
ne produit une `Admission` que si la confirmation locale **et** celle du pair
sont acquises (`SasAttendu::appairer` rend `None` sinon). Côté transport,
`AppairageEnCours::decider_avec_lien` envoie la décision locale, attend celle du
pair, et ne rend `Ok(Admission)` que si les deux disent oui avant l'échéance.

- Le transport **n'écrit jamais** au registre : l'appelant appelle
  `Admission::inscrire`.
- Un `AppairageEnCours` lâché sans décision ferme sa connexion : le pair voit un
  abandon, rien n'est écrit d'aucun côté.
- Le délai par défaut est de 120 s (`DELAI_APPAIRAGE`), de la connexion à la
  décision du pair ; `appairer_vers` et `ecouter_avec_appairage` prennent le
  délai en paramètre.
- Le serveur garde au plus 4 demandes en attente de décision
  (`CAPACITE_DEMANDES`) ; au-delà, la demande est fermée, jamais mise en file.
- Une clé **révoquée** qui demande l'appairage est fermée d'office ; la
  révocation est revérifiée au moment de la décision. Une clé **active** peut
  se ré-appairer sous son identifiant.
- On ne s'appaire pas avec soi-même (clé du pair = ma clé : refus).

### Le lien `moi` / `autre`

Chaque « oui » porte le lien choisi **de ce côté** (`Lien`) :

| Lien | Sens | Rôle inscrit au registre |
|---|---|---|
| `moi` | deux appareils de la même personne | `Ownership` |
| `autre` | l'appareil de quelqu'un d'autre | `Shared` |

Le lien final vaut `moi` **seulement si les deux côtés ont dit `moi`**
(`Lien::combiner`) : un côté seul ne peut jamais s'octroyer la propriété. Un
`confirme` sans champ `lien` (pair plus ancien) vaut `autre`.
`decider(accepte)` équivaut à `decider_avec_lien(accepte, Lien::Autre)`. Le rôle
passé à l'ouverture de l'appairage n'est plus qu'indicatif
(`role_indicatif`) : c'est le lien final qui fixe le rôle.

## Messages

### L'enveloppe signée

Tout message a la même forme (`Message`, `envelope.rs`) :

```json
{"v":1,"id":"<uuid>","de":"device-1a2b3c4d","a":"device-5e6f7a8b",
 "type":"service.appel","horloge":42,"charge":{…},"signature":"<128 hex>"}
```

- `v` vaut `MESH_VERSION` (1) ; une autre version est refusée.
- `horloge` est un compteur logique de l'émetteur, **pas** un horodatage. La
  porte ne le vérifie pas.
- `signature` : Ed25519 (64 octets en hexadécimal) sur les **octets signés** :
  `SIGNING_DOMAIN` (`b"this-mesh/v1\n"`) suivi du JSON canonique de
  `{a, charge, de, horloge, id, type, v}`. Tout est signé sauf la signature,
  `de` et `a` compris. Le préfixe de domaine fait qu'une signature du maillage
  ne vaut rien ailleurs (manifestes, etc.).

Sur QUIC, chaque message est cadré (longueur sur 4 octets big-endian puis les
octets, `transport/cadrage.rs`) ; une longueur au-delà du plafond est refusée
**avant** toute allocation du corps.

### La forme canonique, et pourquoi pas de nombres à virgule

Les octets signés doivent être les mêmes chez l'émetteur et chez le récepteur,
quels que soient l'ordre des champs ou la mise en forme sur le fil. La forme
canonique (`canonical`) : JSON compact, clés triées par ordre d'octets à tous
les niveaux, sans dépendre de la feature `preserve_order` de serde_json.

Un nombre à virgule n'a pas de forme écrite unique (`0.5`, `5e-1`,
`0.50000000000000001` désignent le même `f64`, et deux bibliothèques ne
l'impriment pas pareil) : la signature cesserait d'être reproductible. Il est
donc **refusé** : `Message::signed` rend `SignError::NonCanonical`, et la porte
rend `Raison::ChargeNonCanonique`. Seuls les entiers passent.

Conséquence pratique : une charge qui contient des nombres à virgule voyage
**en chaîne JSON**. L'application sérialise la valeur (`.to_string()`) et met la
chaîne dans la charge ; le récepteur la relit. C'est ce que fait l'Atelier pour
une requête de moteur, dont la liste d'outils porte des `"minimum": 0.5`
(test dans `atelier/src-tauri/src/maillage.rs`).

### Les types de message

Énumérés (`MessageType`), aucun type libre. Le plafond est celui **du message
entier** tel qu'il arrive sur le fil ; il est fixé par `Limits::default()`
(`limits.rs`), un champ par type :

| Type sur le fil | Usage | Plafond (champ de `Limits`) |
|---|---|---|
| `appairage.sas` | code court, clé publique | 2 Kio (`appairage_sas`) |
| `grant.demande` | un module, des droits demandés | 4 Kio (`grant_demande`) |
| `grant.reponse` | accord ou refus | 4 Kio (`grant_reponse`) |
| `espace.op` | une opération sur une liste partagée | 64 Kio (`espace_op`) |
| `espace.sync` | un lot d'opérations de rattrapage | 256 Kio (`espace_sync`) |
| `agent.propose` | une proposition d'agent | 32 Kio (`agent_propose`) |
| `agent.repond` | la réponse à une proposition | 64 Kio (`agent_repond`) |
| `service.appel` | un appel de service | 1 Mio (`service_appel`) |
| `service.reponse` | une réponse de service envoyée comme message | 1 Mio (`service_reponse`) |

Le plus grand plafond (`Limits::ceiling`) est appliqué avant même de connaître
le type, puis le plafond du type. Ces messages portent des opérations et des
propositions, pas des fichiers.

### La porte et ses verdicts

`Gate::admettre(expediteur, octets)` est le seul chemin d'entrée. `expediteur`
est l'identité que **le canal** a authentifiée (la clé du certificat client,
retrouvée au registre). L'ordre est fixe, et la charge n'est jamais décodée
avant que la taille, l'identité et le type soient bons :

1. **Taille** : plafond global (`Limits::ceiling`), avant tout décodage.
2. **Enveloppe** lisible, charge gardée en JSON brut ; `v` = `MESH_VERSION`.
3. **`de` = expéditeur** du canal.
4. **Clé** connue et active (inconnue et révoquée sont distinguées).
5. **Type** énuméré ; sinon ignoré et journalisé (au plus 256 entrées,
   `Limits::journal`), charge jamais lue. Puis plafond du type.
6. **Signature** : c'est seulement ici que la charge est décodée, remise en
   forme canonique et vérifiée.
7. **Rejeu** : un `id` déjà vu est acquitté sans être rejoué.

| Verdict | Sens | Sur le fil (accusé) |
|---|---|---|
| `Admis(Message)` | authentique et nouveau : à traiter | `{"v":"admis"}` (+ `reponse`, § Appel de service) |
| `DejaVu(id)` | même `id` déjà admis : acquitté, **pas rejoué** | `{"v":"deja-vu","id":"…"}` |
| `Ignore(type)` | type hors énumération : journalisé, charge non lue | `{"v":"ignore","type":"…"}` |
| `Refuse(Raison)` | une raison par cause, jamais un « non » générique | `{"v":"refuse","raison":"…"}` |

Les raisons de refus (`Raison`) : `TropGros`, `Illisible`, `VersionInconnue`,
`DeNeCorrespondPas`, `CleInconnue`, `CleRevoquee`, `SignatureInvalide`,
`ChargeNonCanonique`. Un refus ferme la connexion après l'envoi de l'accusé.

### L'anti-rejeu

La porte retient les `id` des messages **admis** : au plus 4096
(`Limits::seen_ids`), les plus anciens sortent. L'idempotence ne s'applique
qu'à un message authentique, sinon un inconnu pourrait « brûler » un `id`. La
mémoire est en RAM : voir § Limites connues.

### Le registre partagé et la révocation

`Peers` est un **handle** : `clone()` partage le registre, il ne le copie pas.
Le listener, la porte et l'application voient le même : un ajout ou une
révocation (`peers.revoke(id)`) prend effet sur le prochain message, sans
rouvrir le listener. Une révocation coupe **les sessions de ce pair seulement**
(`Peers::surveiller_revocation`), y compris pendant l'attente d'un répondeur.

## Appel de service

Un pair peut **répondre**. Le client envoie un message `service.appel` avec
`appeler` ; le serveur, ouvert par `ecouter_complet` (ou `ecouter_complet_sur`),
passe chaque message admis au `Repondeur` ; la réponse revient **dans
l'accusé**.

```text
client                                   serveur
  appeler(adresse, identite, epingle, message, delai)
    ── message signé (cadré) ──────────▶ Gate::admettre
                                         verdict → Flux (d'abord)
                                         si Admis : Repondeur(de, message)
    ◀── {"v":"admis","reponse":{…}} ──── accusé
  (Verdict, Option<Value>)
```

- **`Repondeur`** : `Arc<dyn Fn(DeviceId, Message) -> Future<Output =
  Option<Value>>>`. Il reçoit l'expéditeur authentifié et le message admis.
- **Seul un `Admis` l'appelle** : jamais un refusé, un ignoré ni un déjà-vu.
  Un rejeu ne rappelle pas le répondeur (une opération n'est pas exécutée deux
  fois) et son accusé ne porte jamais de réponse.
- Le verdict part dans le `Flux` **avant** l'appel au répondeur, et aucun verrou
  de la porte n'est tenu pendant l'attente.
- Le répondeur a 300 s (`DELAI_REPONSE`) ; passé ce délai, l'accusé part
  **sans** réponse.
- Une réponse de plus de 4 Mio de JSON sérialisé (`PLAFOND_REPONSE`) est
  remplacée par `{"erreur":"reponse_trop_grosse"}`. Le client lit l'accusé
  jusqu'à `PLAFOND_REPONSE` plus une marge de 4 Kio (`MARGE_ACCUSE`).
- `appeler` prend un `delai` qui borne toute l'opération (connexion, envoi,
  attente) ; au-delà, `TransportError::Connexion("délai dépassé")`. `None` en
  réponse veut dire : rien répondu, pas de répondeur, ou répondeur en retard.
- `envoyer` (sans réponse) lit un accusé d'au plus 4 Kio (`ACK_CEILING`) : une
  réponse de service n'y tient pas, il faut `appeler`.
- Les messages d'une même connexion sont traités un à un : un appel lent
  retarde les suivants **de cette connexion**, pas ceux des autres pairs.
  `envoyer` et `appeler` ouvrent une connexion par message.

La **réponse n'est pas signée** : elle voyage dans le canal TLS, épinglé sur la
clé du serveur. Elle n'est donc pas soumise à la forme canonique (elle peut
contenir des nombres à virgule), mais elle ne peut pas être relayée à un tiers
comme preuve.

## Écoute sur plusieurs adresses

`ecouter_complet_sur(&[adresses], …)` ouvre **un endpoint QUIC par adresse
locale**, chacun lié exactement à elle, tous derrière **une seule porte**.
`adresses_d_ecoute(locales, port)` choisit les adresses : l'IPv4 qui n'est ni
loopback, ni lien-local, ni non spécifiée, ni multicast, ni broadcast, plus
**toujours** `127.0.0.1`, sans doublon.

### Pourquoi : mesuré

Mesuré sur de vrais appareils (commit `cc0fcbb`) : un Mac branché deux fois sur
le même /24 (Wi-Fi 192.168.1.54, Ethernet 192.168.1.8) écoutait sur `0.0.0.0` ;
un iPhone qui appelait `.54` recevait la réponse depuis `.8` et la jetait (un
client QUIC ignore une autre source), d'où « délai dépassé ».

La cause, lue dans quinn-udp 0.5.16 (`src/unix.rs`), telle que le message du
commit la rapporte : à la réception, l'adresse de destination est bien connue
(`IP_RECVDSTADDR`, l. 164-172, 754-757) ; à l'envoi, sans
`fast-apple-datapath`, la source est posée dans un cmsg `IP_RECVDSTADDR`
(l. 654-661) que macOS n'utilise pas pour choisir la source (« no
IP_SENDSRCADDR on macOS », l. 166) : le routage décide. Un endpoint lié à
l'adresse appelée, lui, répond depuis elle.

### Une seule porte

- Tous les endpoints partagent la même `Gate` : même mémoire anti-rejeu (un
  message rejoué vers l'autre adresse est `DejaVu`, le répondeur n'est pas
  rappelé), même registre, même `Flux`, même file de demandes d'appairage.
- Une adresse qui ne se lie pas ne fait pas échouer les autres : elle est
  rapportée par `Flux::echecs` ; `Flux::adresses` donne celles qui écoutent.
  Erreur seulement si **aucune** ne se lie.
- Un port `0` est résolu par endpoint (chaque adresse peut recevoir un port
  différent) : en production, passer un port fixe.
- Lâcher le `Flux` ferme tous les endpoints.
- `ecouter`, `ecouter_avec_appairage` et `ecouter_complet` passent par le même
  chemin avec une seule adresse.

Non testé dans le dépôt : deux vraies interfaces (`127.0.0.2` ne se lie pas
sous macOS). Les tests utilisent deux ports sur `127.0.0.1`, ce qui prouve le
partage de la porte et de l'anti-rejeu, pas le choix de la source.

## Droits

### Trois niveaux (dans l'application)

Le modèle est celui de l'Atelier (`atelier/src-tauri/src/droits.rs`) ; il est
résumé ici pour situer ce que fournit le framework.

- **Personne** : `moi`, ou un contact. Chaque appareil appairé appartient à
  une personne : lien `moi` → `moi`, lien `autre` → un contact.
- **Service** : ce que CET appareil offre ; chaque opération a une
  `PolicyExpr`.
- **Rôle** : accordé par CET appareil à une **personne**, sur un service.
  Retirer un rôle à une personne coupe tous ses appareils.

L'évaluation se fait chez celui qui offre le service.

### Ce que fournit le framework : `Sujet` et `autorise`

`perm.rs` évalue une `PolicyExpr` pour un appelant du maillage, décrit par un
`Sujet` : `proprietaire` (un appareil du lien `moi`), `authentifie` (un pair
appairé l'est), `roles` (accordés sur CE service), `perms` (permissions
matérialisées, comparées au nom exact).

`autorise(expr, sujet)` rend une décision à deux valeurs : seul un `true`
**évaluable** accorde. Elle s'appuie sur le moteur commun
(`engine::policy::evaluate`), avec un principal sans service ni admin et un
environnement sans résolveur. La table de `perm.rs` :

| Variante | Résultat |
|---|---|
| `Public` | vrai |
| `Authenticated` | `sujet.authentifie` |
| `Owner` | `sujet.proprietaire` (n'implique AUCUN rôle) |
| `HasRole(rs)` | un des `rs` est dans `sujet.roles` ; liste vide : faux |
| `Perm(p)` | `p` est dans `sujet.perms` (nom exact) |
| `Deny` | faux |
| `ServiceOnly` | faux : le maillage n'a pas de service interne |
| `AdminOnly` | faux : le maillage n'a pas d'admin |
| `Resolver(_)` | faux (inévaluable) : le maillage n'a pas de résolveur |
| `And(v)` | tous vrais ; liste vide : FAUX (un `And` vide n'accorde rien) |
| `Or(v)` | au moins un vrai ; liste vide : faux |
| `Not(e)` | négation de `e` ; si `e` est inévaluable, faux |

Attention, `Not` est une vraie négation : `not:deny`, `not:admin_only` et
`not:service_only` accordent TOUT, à tout appelant. Seul `not:resolver:x` reste
refusé. `PolicyExpr::may_admit_anonymous` repère ces politiques avant
publication.

`autorise_op(perm, op, sujet)` applique `autorise` à la politique **déclarée**
de `op` ; une opération absente est refusée (on n'applique pas
`PermDesc::default_policy`, qui donnerait `authenticated` aux opérations
standard). `roles_de(perm)` donne les rôles qu'un service peut distribuer, lus
dans ses `HasRole` : on ne les déclare pas une seconde fois.

Une chaîne de politique ne peut pas s'imbriquer au-delà de 32 niveaux
(`MAX_POLICY_DEPTH`).

### `role:a|b`, jamais `role:a|role:b`

Plusieurs rôles s'écrivent `role:lecteur|membre` (« l'un ou l'autre »).
`role:lecteur|role:membre` se lit naturellement pareil, mais donnait les rôles
`lecteur` et `role:membre` (le `:` est permis dans un nom) : un membre perdait
l'accès sans erreur. Cette forme est maintenant **refusée** au parsing, avec le
message « repeated `role:` prefix … write `role:a|b` ».

### Être appairé ne donne rien

L'appairage inscrit une clé au registre : il permet à la porte d'**authentifier**
les messages, il n'accorde aucun service. Ce qu'un pair peut faire se décide
opération par opération, par `autorise_op`, avec les rôles accordés à sa
personne.

Une nuance à connaître : un pair appairé est `authentifie`. Une opération dont
la politique déclarée est `authenticated` (ou `public`) est donc ouverte à tout
pair appairé, lien `autre` compris. Être appairé ne donne rien **tant que les
services ne déclarent pas `authenticated`** ; l'Atelier a un test qui fixe ce
comportement (`un_pair_autre_sans_role_n_a_que_les_services_authentifies`).

## Limites connues

- **Pas de streaming des réponses.** Une réponse de service tient dans un seul
  accusé : au plus 4 Mio (`PLAFOND_REPONSE`), et rien n'arrive avant la fin du
  répondeur (300 s au plus, `DELAI_REPONSE`). Un moteur qui produit des jetons
  au fil de l'eau rend tout d'un bloc.
- **Pas de traversée de NAT, pas de relais** : les appareils doivent se
  joindre directement.
- **IPv4 seulement** : `adresses_d_ecoute` écarte l'IPv6, et le client
  d'`envoyer`/`appeler` lie `0.0.0.0:0`.
- **quinn-udp sous macOS ne choisit pas l'adresse source** d'une réponse (pas
  d'`IP_SENDSRCADDR`) : un endpoint lié à `0.0.0.0` peut répondre depuis la
  mauvaise interface. Contournement : `ecouter_complet_sur`, un endpoint par
  adresse. Une adresse qui apparaît après l'ouverture (changement de réseau)
  n'écoute pas tant qu'on ne rouvre pas.
- **L'anti-rejeu est en mémoire et borné** : 4096 `id` (`Limits::seen_ids`),
  perdus au redémarrage. Un message capturé peut être réadmis après un
  redémarrage du récepteur, ou après 4096 messages plus récents. `horloge`
  n'est pas vérifiée.
- **Un message par connexion** côté client (`envoyer`, `appeler`) : une
  poignée de main TLS par message.

## Points ouverts

Lus dans le code, pas encore tranchés :

- `DeviceId::de_cle` ne garde que 4 octets de la clé, et `Peers::admit`
  **remplace** une fiche existante sous le même identifiant. Un appairage dont
  la clé a le même préfixe qu'un pair déjà inscrit écraserait sa fiche à
  l'inscription (`Admission::inscrire`). Il faut une confirmation humaine, mais
  le préfixe se trouve par force brute.
- La porte vérifie la signature de `a` (le destinataire), mais ne compare pas
  `a` à l'identifiant local.

## Vérifier

```sh
cargo test -p this-mesh                       # protocole pur
cargo test -p this-mesh --features transport  # transport QUIC, appairage, appel
cargo test -p this-descriptor perm            # autorise, role:a|b
```
