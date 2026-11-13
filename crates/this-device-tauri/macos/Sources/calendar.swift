// Calendar events on macOS through EventKit: permission, read, and create.
//
// Four methods (same names as iOS):
//   calendarStatus       the permission state, read without prompting
//   calendarRequest      asks for full access once, then answers the state
//   calendarEvents       the events of a bounded window (max 31 days, 200 events)
//   calendarCreateEvent  writes one event in the default calendar
//
// macOS 14+ uses `requestFullAccessToEvents` (same as iOS 17+). Older macOS uses
// `requestAccess(to: .event)` and maps `.authorized` to "granted".
//
// Writing changes the person's calendar: the device broker must have consent before the
// Rust side forwards a create.

import EventKit
import Foundation

enum MacCalendar {
    // MARK: - Bounds (same as iOS)

    /// The longest window a read may cover, in milliseconds (31 days).
    private static let windowMsMax: Int64 = 31 * 24 * 60 * 60 * 1000
    /// The most events one read answers.
    private static let eventsMax = 200

    private static let store = EKEventStore()

    // MARK: - Permission state

    /// The permission state in the contract's words.
    static func currentState() -> String {
        let status = EKEventStore.authorizationStatus(for: .event)
        if #available(macOS 14.0, *) {
            switch status {
            case .fullAccess:                   return Etat.granted.fil
            case .authorized:                   return Etat.granted.fil  // legacy pre-macOS 14
            case .writeOnly, .denied, .restricted: return Etat.denied.fil
            case .notDetermined:                return Etat.prompt.fil
            @unknown default:                   return Etat.prompt.fil
            }
        } else {
            switch status {
            case .authorized:    return Etat.granted.fil
            case .denied:        return Etat.denied.fil
            case .restricted:    return Etat.denied.fil
            case .notDetermined: return Etat.prompt.fil
            @unknown default:    return Etat.prompt.fil
            }
        }
    }

    // MARK: - Dispatch targets

    /// `calendarStatus`
    static func status() -> String {
        return statusReply(state: currentState())
    }

    /// `calendarRequest` — asks for full access to events.
    static func request() -> String {
        return blockingCall(timeout: 30) { done in
            if #available(macOS 14.0, *) {
                store.requestFullAccessToEvents { _, _ in
                    done(statusReply(state: currentState()))
                }
            } else {
                store.requestAccess(to: .event) { _, _ in
                    done(statusReply(state: currentState()))
                }
            }
        }
    }

    /// `calendarEvents` — the events of a bounded window.
    static func events(_ args: [String: Any]) -> String {
        guard let startMs = intArg(args, "start_ms"),
              let endMs = intArg(args, "end_ms") else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard isValidCalendarWindow(startMs: startMs, endMs: endMs) else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard currentState() == Etat.granted.fil else {
            return errorReply(code: ReplyCode.notAuthorized)
        }

        let start = calDate(ms: startMs)
        let end = calDate(ms: endMs)
        let predicate = store.predicateForEvents(withStart: start, end: end, calendars: nil)
        let events = store.events(matching: predicate)
            .sorted { $0.startDate < $1.startDate }
            .prefix(eventsMax)
            .map { event in
                CalendarEventOut(
                    title: event.title ?? "",
                    startMs: calMs(event.startDate),
                    endMs: calMs(event.endDate),
                    location: trimmedCalText(event.location))
            }
        return encodeJson(CalendarEventsReply(events: Array(events)))
    }

    /// `calendarCreateEvent` — writes one event in the default calendar.
    static func createEvent(_ args: [String: Any]) -> String {
        guard let rawTitle = args["title"] as? String,
              let title = trimmedCalText(rawTitle),
              let startMs = intArg(args, "start_ms"),
              let endMs = intArg(args, "end_ms") else {
            return errorReply(code: ReplyCode.internalError)
        }
        // A create has no 31-day bound: only the order matters.
        let (span, overflow) = endMs.subtractingReportingOverflow(startMs)
        guard !overflow, span > 0 else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard currentState() == Etat.granted.fil else {
            return errorReply(code: ReplyCode.notAuthorized)
        }
        guard let calendar = store.defaultCalendarForNewEvents else {
            return errorReply(code: ReplyCode.unavailable)
        }

        let event = EKEvent(eventStore: store)
        event.title = title
        event.startDate = calDate(ms: startMs)
        event.endDate = calDate(ms: endMs)
        event.location = trimmedCalText(args["location"] as? String)
        event.calendar = calendar

        do {
            try store.save(event, span: .thisEvent)
            return encodeJson(CalendarCreatedReply(eventId: event.eventIdentifier))
        } catch {
            return errorReply(code: ReplyCode.internalError)
        }
    }

    /// `calendarCalendars` — every event calendar, with its write permission.
    static func calendars() -> String {
        guard currentState() == Etat.granted.fil else {
            return errorReply(code: ReplyCode.notAuthorized)
        }
        let list = store.calendars(for: .event).map {
            CalendarSummaryOut(
                id: $0.calendarIdentifier,
                title: $0.title,
                allows_modifications: $0.allowsContentModifications)
        }
        return encodeJson(CalendarSummariesReply(calendars: list))
    }

    /// `calendarUpdateEvent` — patches an existing event. An absent field changes nothing; at
    /// least one must change. Unknown id: `not_supported`.
    static func updateEvent(_ args: [String: Any]) -> String {
        guard let id = args["id"] as? String, !id.isEmpty else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard currentState() == Etat.granted.fil else {
            return errorReply(code: ReplyCode.notAuthorized)
        }
        guard let event = store.event(withIdentifier: id) else {
            return errorReply(code: ReplyCode.notSupported)
        }
        var changed = false
        if let title = trimmedCalText(args["title"] as? String) {
            event.title = title
            changed = true
        }
        if args["start_ms"] != nil {
            guard let ms = intArg(args, "start_ms") else {
                return errorReply(code: ReplyCode.internalError)
            }
            event.startDate = calDate(ms: ms)
            changed = true
        }
        if args["end_ms"] != nil {
            guard let ms = intArg(args, "end_ms") else {
                return errorReply(code: ReplyCode.internalError)
            }
            event.endDate = calDate(ms: ms)
            changed = true
        }
        if let location = args["location"] as? String {
            event.location = location
            changed = true
        }
        if let notes = args["notes"] as? String {
            event.notes = notes
            changed = true
        }
        // A patch that changes nothing is not sent.
        guard changed else {
            return errorReply(code: ReplyCode.internalError)
        }
        // Checked on the MODIFIED event: the Rust side does not know the stored dates.
        guard event.endDate > event.startDate else {
            return errorReply(code: ReplyCode.internalError)
        }
        do {
            try store.save(event, span: .thisEvent)
            return encodeJson(CalendarEventChangedReply(kind: "event_updated", event_id: id))
        } catch {
            return errorReply(code: ReplyCode.internalError)
        }
    }

    /// `calendarDeleteEvent` — removes one event (this occurrence only, not the series).
    static func deleteEvent(_ args: [String: Any]) -> String {
        guard let id = args["id"] as? String, !id.isEmpty else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard currentState() == Etat.granted.fil else {
            return errorReply(code: ReplyCode.notAuthorized)
        }
        guard let event = store.event(withIdentifier: id) else {
            return errorReply(code: ReplyCode.notSupported)
        }
        do {
            try store.remove(event, span: .thisEvent)
            return encodeJson(CalendarEventChangedReply(kind: "event_deleted", event_id: id))
        } catch {
            return errorReply(code: ReplyCode.internalError)
        }
    }

    // MARK: - Helpers

    private static func isValidCalendarWindow(startMs: Int64, endMs: Int64) -> Bool {
        let (span, overflow) = endMs.subtractingReportingOverflow(startMs)
        return !overflow && span > 0 && span <= windowMsMax
    }

    private static func calDate(ms: Int64) -> Date {
        Date(timeIntervalSince1970: Double(ms) / 1000)
    }

    private static func calMs(_ date: Date) -> Int64 {
        Int64((date.timeIntervalSince1970 * 1000).rounded())
    }
}

/// Extract an integer from the args dictionary. Handles both Int and Double (JSON numbers).
private func intArg(_ args: [String: Any], _ key: String) -> Int64? {
    if let v = args[key] as? Int64 { return v }
    if let v = args[key] as? Int { return Int64(v) }
    if let v = args[key] as? Double { return Int64(v) }
    return nil
}

/// Trim whitespace; nil when nothing is left.
private func trimmedCalText(_ raw: String?) -> String? {
    guard let text = raw?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else {
        return nil
    }
    return text
}

// MARK: - Wire types (match iOS exactly)

private struct CalendarEventOut: Encodable {
    let title: String
    let startMs: Int64
    let endMs: Int64
    let location: String?

    private enum Key: String, CodingKey {
        case title, start_ms, end_ms, location
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        try c.encode(title, forKey: .title)
        try c.encode(startMs, forKey: .start_ms)
        try c.encode(endMs, forKey: .end_ms)
        try c.encode(location, forKey: .location)
    }
}

private struct CalendarEventsReply: Encodable {
    let events: [CalendarEventOut]

    private enum Key: String, CodingKey {
        case kind, events
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        try c.encode("calendar_events", forKey: .kind)
        try c.encode(events, forKey: .events)
    }
}

private struct CalendarCreatedReply: Encodable {
    let eventId: String?

    private enum Key: String, CodingKey {
        case kind, event_id
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        try c.encode("calendar_created", forKey: .kind)
        try c.encode(eventId, forKey: .event_id)
    }
}

private struct CalendarSummaryOut: Encodable {
    let id: String
    let title: String
    let allows_modifications: Bool
}

/// `{"kind":"calendars","calendars":[...]}`
private struct CalendarSummariesReply: Encodable {
    let kind = "calendars"
    let calendars: [CalendarSummaryOut]
}

/// `{"kind":"event_updated"|"event_deleted","event_id":"..."}`
private struct CalendarEventChangedReply: Encodable {
    let kind: String
    let event_id: String
}
