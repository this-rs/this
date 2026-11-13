// Contacts: the permission state, the request, and a bounded read of the contacts book.
//
// PERSONAL DATA OF THIRD PARTIES. A contact is a person who never agreed to this app. Nothing
// here logs a contact, and the fetch asks the system only for the keys listed in
// `ContactsBridge.keys`. Adding a key (notes, birthday, photo, postal address...) is a decision
// to take with the supervisor, not a detail.
//
// Wire format: see `families/contacts.rs` on the Rust side (`ContactsReply`). Errors are answers
// too: every method resolves, none rejects.

import Contacts
import Foundation
import Tauri

/// The arguments of `contactsList`. The Rust side sends `limit` already defaulted.
private struct ContactsListArgs: Decodable {
  let limit: Int
}

/// One contact, as the Rust side reads it: identifier, full name, emails, phone numbers.
private struct ContactEntry: Encodable {
  let id: String
  let name: String
  let emails: [String]
  let phones: [String]
}

/// `{"kind":"contacts","contacts":[...]}`. Hand-encoded like `Reply`, so the key set is explicit.
private struct ContactsAnswer: Encodable {
  let contacts: [ContactEntry]

  private enum Key: String, CodingKey {
    case kind, contacts
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode("contacts", forKey: .kind)
    try c.encode(contacts, forKey: .contacts)
  }
}

fileprivate enum ContactsBridge {
  /// The bounds of a list. The Rust side refuses a limit outside them; this side refuses it too.
  static let limitMin = 1
  static let limitMax = 200

  /// The only fields read from the book.
  static let keys: [CNKeyDescriptor] = [
    CNContactIdentifierKey as CNKeyDescriptor,
    CNContactGivenNameKey as CNKeyDescriptor,
    CNContactFamilyNameKey as CNKeyDescriptor,
    CNContactEmailAddressesKey as CNKeyDescriptor,
    CNContactPhoneNumbersKey as CNKeyDescriptor,
  ]

  static let store = CNContactStore()

  /// The permission state in the contract's words (`PermissionState`). Partial access (iOS 18
  /// "selected contacts") is not granted: the contract has no partial state, and a list would
  /// silently be a subset, so it counts as denied.
  static func state(_ status: CNAuthorizationStatus) -> String {
    if #available(iOS 18.0, *), status == .limited {
      return Etat.denied.fil
    }
    switch status {
    case .authorized: return Etat.granted.fil
    case .denied, .restricted: return Etat.denied.fil
    case .notDetermined: return Etat.prompt.fil
    default: return Etat.prompt.fil
    }
  }

  /// Reads at most `limit` contacts, in the order the system returns them. Blocking: call it
  /// off the main thread.
  static func fetch(limit: Int) throws -> [ContactEntry] {
    let request = CNContactFetchRequest(keysToFetch: keys)
    var entries: [ContactEntry] = []
    try store.enumerateContacts(with: request) { contact, stop in
      let name = [contact.givenName, contact.familyName]
        .filter { !$0.isEmpty }
        .joined(separator: " ")
      entries.append(
        ContactEntry(
          id: contact.identifier,
          name: name,
          emails: contact.emailAddresses.map { $0.value as String },
          phones: contact.phoneNumbers.map { $0.value.stringValue }
        ))
      if entries.count >= limit {
        stop.pointee = true
      }
    }
    return entries
  }
}

/// Écrit une ligne de diagnostic dans le journal de l'app (`$HOME/Documents/atelier.log`),
/// le même fichier que la coquille trace. Ne sert qu'au diagnostic : rien de ce qui touche
/// les données de la personne n'y transite, seulement des statuts et des messages génériques.
enum ContactsDiagnostic {
  static func log(_ message: String) {
    guard let home = ProcessInfo.processInfo.environment["HOME"] else { return }
    let path = home + "/Documents/atelier.log"
    let ligne = "contacts \(message)\n"
    guard let data = ligne.data(using: .utf8) else { return }
    if let handle = try? FileHandle(forWritingTo: URL(fileURLWithPath: path)) {
      handle.seekToEndOfFile()
      handle.write(data)
      try? handle.close()
    } else {
      try? data.write(to: URL(fileURLWithPath: path), options: .atomic)
    }
  }
}

extension ThisDevicePlugin {
  /// The contacts permission state, read without prompting the person.
  @objc public func contactsStatus(_ invoke: Invoke) {
    let status = CNContactStore.authorizationStatus(for: .contacts)
    invoke.resolve(Reply.status(state: ContactsBridge.state(status)))
  }

  /// Asks the person for access to the contacts book. The system prompt shows only once; after
  /// that the answer is the stored one. Resolves with the state after the answer.
  @objc public func contactsRequest(_ invoke: Invoke) {
    ContactsBridge.store.requestAccess(for: .contacts) { _, _ in
      let status = CNContactStore.authorizationStatus(for: .contacts)
      invoke.resolve(Reply.status(state: ContactsBridge.state(status)))
    }
  }

  /// Up to `limit` contacts, with the explicit keys only. Only with full access; otherwise
  /// `not_authorized`. A bad argument is `internal` (no dedicated code in the contract).
  @objc public func contactsList(_ invoke: Invoke) {
    let args: ContactsListArgs
    do {
      args = try invoke.parseArgs(ContactsListArgs.self)
    } catch {
      ContactsDiagnostic.log("parseArgs: \(error)")
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    guard (ContactsBridge.limitMin...ContactsBridge.limitMax).contains(args.limit) else {
      ContactsDiagnostic.log("limit hors borne: \(args.limit)")
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    let statut = CNContactStore.authorizationStatus(for: .contacts)
    ContactsDiagnostic.log("statut systeme: \(statut.rawValue)")
    guard statut == .authorized else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    DispatchQueue.global(qos: .userInitiated).async {
      do {
        let contacts = try ContactsBridge.fetch(limit: args.limit)
        invoke.resolve(ContactsAnswer(contacts: contacts))
      } catch {
        // Le TEXTE de l'erreur d'énumération est générique (code CNError), pas le contenu du
        // carnet : on peut le tracer pour diagnostiquer sans rien faire fuiter.
        ContactsDiagnostic.log("fetch: \(error)")
        invoke.resolve(Reply.error(code: "internal"))
      }
    }
  }
}

// ---- Un contact, une recherche, une création -------------------------------------------
//
// `contactsList` ne permettait ni de désigner quelqu'un précisément, ni d'en ajouter un. Ces
// trois méthodes comblent ce trou, avec les MÊMES clés (`ContactsBridge.keys`) : le pont ne lit
// jamais un champ que `families/contacts.rs` ne déclare pas.

private struct ContactIdArgs: Decodable {
  let id: String
}

private struct ContactsSearchArgs: Decodable {
  let query: String
  let limit: Int
}

private struct ContactsCreateArgs: Decodable {
  let name: String
  let emails: [String]
  let phones: [String]
}

/// `{"kind":"contact","contact":{…}}` — la même fiche que dans une liste, seule.
private struct ContactAnswer: Encodable {
  let kind = "contact"
  let contact: ContactEntry
}

extension ContactsBridge {
  /// One contact, as the Rust side reads it. Missing names are joined as in `fetch`.
  static func entry(_ contact: CNContact) -> ContactEntry {
    let name = [contact.givenName, contact.familyName]
      .filter { !$0.isEmpty }
      .joined(separator: " ")
    return ContactEntry(
      id: contact.identifier,
      name: name,
      emails: contact.emailAddresses.map { $0.value as String },
      phones: contact.phoneNumbers.map { $0.value.stringValue }
    )
  }
}

extension ThisDevicePlugin {
  /// One contact, by the identifier a list or a search returned.
  @objc public func contactsGet(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ContactIdArgs.self)
    guard CNContactStore.authorizationStatus(for: .contacts) == .authorized else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    DispatchQueue.global(qos: .userInitiated).async {
      do {
        let contact = try ContactsBridge.store.unifiedContact(
          withIdentifier: args.id, keysToFetch: ContactsBridge.keys)
        invoke.resolve(ContactAnswer(contact: ContactsBridge.entry(contact)))
      } catch {
        // Un identifiant que le carnet ne connaît plus : `not_supported`, pas une panne.
        ContactsDiagnostic.log("get: \(error)")
        invoke.resolve(Reply.error(code: "not_supported"))
      }
    }
  }

  /// Contacts whose name matches `query`, at most `limit` of them.
  @objc public func contactsSearch(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ContactsSearchArgs.self)
    guard (ContactsBridge.limitMin...ContactsBridge.limitMax).contains(args.limit) else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    guard CNContactStore.authorizationStatus(for: .contacts) == .authorized else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    let query = args.query.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !query.isEmpty else {
      invoke.resolve(Reply.error(code: "missing_argument"))
      return
    }
    DispatchQueue.global(qos: .userInitiated).async {
      let predicate = CNContact.predicateForContacts(matchingName: query)
      do {
        let found = try ContactsBridge.store.unifiedContacts(
          matching: predicate, keysToFetch: ContactsBridge.keys)
        let entries = found.prefix(args.limit).map { ContactsBridge.entry($0) }
        invoke.resolve(ContactsAnswer(contacts: Array(entries)))
      } catch {
        ContactsDiagnostic.log("search: \(error)")
        invoke.resolve(Reply.error(code: "internal"))
      }
    }
  }

  /// Adds one contact to the person's book. The container is the default one: choosing an
  /// account (iCloud, Google…) is the person's business, not this bridge's.
  @objc public func contactsCreate(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ContactsCreateArgs.self)
    guard CNContactStore.authorizationStatus(for: .contacts) == .authorized else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    let name = args.name.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !name.isEmpty else {
      invoke.resolve(Reply.error(code: "missing_argument"))
      return
    }
    let contact = CNMutableContact()
    let parts = name.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true)
    contact.givenName = parts.first.map(String.init) ?? ""
    contact.familyName = parts.count > 1 ? String(parts[1]) : ""
    if !args.emails.isEmpty {
      contact.emailAddresses = args.emails.map {
        CNLabeledValue(label: CNLabelHome, value: $0 as NSString)
      }
    }
    if !args.phones.isEmpty {
      contact.phoneNumbers = args.phones.map {
        CNLabeledValue(label: CNLabelPhoneNumberMobile, value: CNPhoneNumber(stringValue: $0))
      }
    }
    let save = CNSaveRequest()
    save.add(contact, toContainerWithIdentifier: nil)
    DispatchQueue.global(qos: .userInitiated).async {
      do {
        try ContactsBridge.store.execute(save)
        invoke.resolve(ContactAnswer(contact: ContactsBridge.entry(contact)))
      } catch {
        ContactsDiagnostic.log("create: \(error)")
        invoke.resolve(Reply.error(code: "internal"))
      }
    }
  }
}

/// Ancre de l'éditeur de liens — voir `ThisDevicePlugin.famillesAncrees`.
///
/// Un `#selector(Foo.bar)` ne produit pas une référence au SYMBOLE de `bar` : il produit une
/// chaîne. Le dead-strip retire donc l'objet entier, et le pont répond « No command bar found
/// for plugin this-device ». Mesuré le 13/11/2026 sur un iPhone 16 Pro : dix-huit méthodes de
/// familles compilaient, étaient listées dans `famillesLiees`, et étaient **absentes du binaire**.
/// Référencer une CLASSE de ce fichier garde son objet — donc ses méthodes.
final class FamilleContacts: NSObject {}
