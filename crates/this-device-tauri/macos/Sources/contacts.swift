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
        case .authorized:    return Etat.granted.fil
        case .denied:        return Etat.denied.fil
        case .restricted:    return Etat.denied.fil
        case .notDetermined: return Etat.prompt.fil
        default:             return Etat.prompt.fil
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
        guard currentState() == Etat.granted.fil else {
            return errorReply(code: ReplyCode.notAuthorized)
        }

        do {
            let entries = try fetch(limit: limit)
            return encodeJson(ContactsAnswer(contacts: entries))
        } catch {
            return errorReply(code: ReplyCode.internalError)
        }
    }

    // MARK: - Get / search / create (same shapes as iOS)

    /// One contact as the wire entry. Names are joined as in `fetch`.
    private static func entry(_ contact: CNContact) -> ContactEntry {
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

    /// `contactsGet` — one contact by the identifier a list or a search returned.
    /// An identifier the book no longer knows is `not_supported`, not a failure.
    static func get(_ args: [String: Any]) -> String {
        guard let id = args["id"] as? String,
              !id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard currentState() == Etat.granted.fil else {
            return errorReply(code: ReplyCode.notAuthorized)
        }
        do {
            let contact = try store.unifiedContact(withIdentifier: id, keysToFetch: keys)
            return encodeJson(ContactAnswer(contact: entry(contact)))
        } catch {
            return errorReply(code: ReplyCode.notSupported)
        }
    }

    /// `contactsSearch` — contacts whose name matches `query`, at most `limit` of them.
    static func search(_ args: [String: Any]) -> String {
        let limit = (args["limit"] as? Int) ?? 50
        guard (limitMin...limitMax).contains(limit) else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard let raw = args["query"] as? String,
              !raw.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard currentState() == Etat.granted.fil else {
            return errorReply(code: ReplyCode.notAuthorized)
        }
        let query = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            let predicate = CNContact.predicateForContacts(matchingName: query)
            let found = try store.unifiedContacts(matching: predicate, keysToFetch: keys)
            return encodeJson(ContactsAnswer(contacts: found.prefix(limit).map { entry($0) }))
        } catch {
            return errorReply(code: ReplyCode.internalError)
        }
    }

    /// `contactsCreate` — adds one contact to the default container. Choosing an account
    /// (iCloud, Google...) is the person's business, not this bridge's.
    static func create(_ args: [String: Any]) -> String {
        guard let rawName = args["name"] as? String else {
            return errorReply(code: ReplyCode.internalError)
        }
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard currentState() == Etat.granted.fil else {
            return errorReply(code: ReplyCode.notAuthorized)
        }
        let emails = (args["emails"] as? [String]) ?? []
        let phones = (args["phones"] as? [String]) ?? []

        let contact = CNMutableContact()
        let parts = name.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true)
        contact.givenName = parts.first.map(String.init) ?? ""
        contact.familyName = parts.count > 1 ? String(parts[1]) : ""
        if !emails.isEmpty {
            contact.emailAddresses = emails.map {
                CNLabeledValue(label: CNLabelHome, value: $0 as NSString)
            }
        }
        if !phones.isEmpty {
            contact.phoneNumbers = phones.map {
                CNLabeledValue(label: CNLabelPhoneNumberMobile, value: CNPhoneNumber(stringValue: $0))
            }
        }
        let save = CNSaveRequest()
        save.add(contact, toContainerWithIdentifier: nil)
        do {
            try store.execute(save)
            return encodeJson(ContactAnswer(contact: entry(contact)))
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

/// `{"kind":"contact","contact":{...}}` — the same card as in a list, alone.
private struct ContactAnswer: Encodable {
    let contact: ContactEntry

    private enum Key: String, CodingKey {
        case kind, contact
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        try c.encode("contact", forKey: .kind)
        try c.encode(contact, forKey: .contact)
    }
}
