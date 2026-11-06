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
      return "denied"
    }
    switch status {
    case .authorized: return "granted"
    case .denied, .restricted: return "denied"
    case .notDetermined: return "prompt"
    default: return "prompt"
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
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    guard (ContactsBridge.limitMin...ContactsBridge.limitMax).contains(args.limit) else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    guard CNContactStore.authorizationStatus(for: .contacts) == .authorized else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    DispatchQueue.global(qos: .userInitiated).async {
      do {
        let contacts = try ContactsBridge.fetch(limit: args.limit)
        invoke.resolve(ContactsAnswer(contacts: contacts))
      } catch {
        // The error text is not forwarded: it may quote the book.
        invoke.resolve(Reply.error(code: "internal"))
      }
    }
  }
}
