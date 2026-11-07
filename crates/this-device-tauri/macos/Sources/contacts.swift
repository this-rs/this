// Contacts on macOS: permission state, request, and bounded read of the contacts book.
//
// PERSONAL DATA OF THIRD PARTIES. A contact is a person who never agreed to this app. Nothing
// here logs a contact, and the fetch asks the system only for the five keys listed below.
//
// Wire format: identical to the iOS side (`families/contacts.rs`). Errors are answers too:
// every function returns a JSON string, none throws.

import Contacts
import Foundation

enum MacContacts {
    // MARK: - Bounds (same as iOS)

    private static let limitMin = 1
    private static let limitMax = 200

    /// The only fields read from the book.
    private static let keys: [CNKeyDescriptor] = [
        CNContactIdentifierKey as CNKeyDescriptor,
        CNContactGivenNameKey as CNKeyDescriptor,
        CNContactFamilyNameKey as CNKeyDescriptor,
        CNContactEmailAddressesKey as CNKeyDescriptor,
        CNContactPhoneNumbersKey as CNKeyDescriptor,
    ]

    private static let store = CNContactStore()

    // MARK: - Permission state

    /// The permission state in the contract's words. On macOS there is no `.limited` status
    /// (that is iOS 18 only), so the mapping is simpler.
    static func currentState() -> String {
        let status = CNContactStore.authorizationStatus(for: .contacts)
        switch status {
        case .authorized:    return "granted"
        case .denied:        return "denied"
        case .restricted:    return "denied"
        case .notDetermined: return "prompt"
        default:             return "prompt"
        }
    }

    // MARK: - Dispatch targets

    /// `contactsStatus` — the permission state, read without prompting the person.
    static func status() -> String {
        return statusReply(state: currentState())
    }

    /// `contactsRequest` — asks the person for access. The system prompt shows only once.
    static func request() -> String {
        return blockingCall(timeout: 30) { done in
            store.requestAccess(for: .contacts) { _, _ in
                done(statusReply(state: currentState()))
            }
        }
    }

    /// `contactsList` — up to `limit` contacts with the explicit keys only.
    /// Only with full access; otherwise `not_authorized`.
    static func list(_ args: [String: Any]) -> String {
        let limit = (args["limit"] as? Int) ?? 50
        guard (limitMin...limitMax).contains(limit) else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard currentState() == "granted" else {
            return errorReply(code: ReplyCode.notAuthorized)
        }

        do {
            let entries = try fetch(limit: limit)
            return encodeJson(ContactsAnswer(contacts: entries))
        } catch {
            return errorReply(code: ReplyCode.internalError)
        }
    }

    // MARK: - Fetch

    /// Reads at most `limit` contacts, in the order the system returns them. Blocking.
    private static func fetch(limit: Int) throws -> [ContactEntry] {
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

// MARK: - Wire types (match iOS ContactsAnswer / ContactEntry exactly)

private struct ContactEntry: Encodable {
    let id: String
    let name: String
    let emails: [String]
    let phones: [String]
}

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
