// Calendar events through EventKit, with full access (iOS 17 and later).
//
// Four methods, answering with `Reply` (Reply.swift) for states and errors, and with the
// calendar answers below for data:
//   calendarStatus       the permission state, read without prompting the person
//   calendarRequest      asks for full access once, then answers the state the person chose
//   calendarEvents       the events of a bounded window: title, start, end, location only
//   calendarCreateEvent  writes one event in the default calendar
//
// Requires NSCalendarsFullAccessUsageDescription in Info.plist: without it the system
// terminates the app on the first access. Below iOS 17 every method answers `not_supported`.
// Writing changes the person's calendar: the device broker must have consent before the Rust
// side forwards a create (see lib.rs and this-platform's `gate`), this file does not ask.

import EventKit
import Foundation
import Tauri

/// The bounds of the calendar bridge. The Rust side (`src/families/calendar.rs`) checks the
/// same bounds before calling in; these are the native guard.
enum CalendarLimits {
  /// The longest window a read may cover, in milliseconds (31 days).
  static let windowMsMax: Int64 = 31 * 24 * 60 * 60 * 1000
  /// The most events one read answers. The rest of the window is not sent.
  static let eventsMax = 200
}

/// Error codes this file sends, beyond `ReplyCode` (Reply.swift).
enum CalendarCode {
  static let notSupported = "not_supported"
  /// A refused argument (empty title, bad window) or a failed save: the caller's bug or ours.
  static let internalError = "internal"
}

struct CalendarWindowArgs: Decodable {
  let startMs: Int64
  let endMs: Int64

  private enum CodingKeys: String, CodingKey {
    case startMs = "start_ms"
    case endMs = "end_ms"
  }
}

struct CalendarCreateArgs: Decodable {
  let title: String
  let startMs: Int64
  let endMs: Int64
  let location: String?

  private enum CodingKeys: String, CodingKey {
    case title
    case startMs = "start_ms"
    case endMs = "end_ms"
    case location
  }
}

/// True when the window runs forward and is no longer than `CalendarLimits.windowMsMax`.
/// The subtraction is checked: extreme values must refuse, not trap.
func isValidWindow(startMs: Int64, endMs: Int64) -> Bool {
  let (span, overflow) = endMs.subtractingReportingOverflow(startMs)
  return !overflow && span > 0 && span <= CalendarLimits.windowMsMax
}

/// The title with its surrounding whitespace removed, or nil when nothing is left.
func trimmedText(_ raw: String?) -> String? {
  guard let text = raw?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else {
    return nil
  }
  return text
}

func calendarDate(ms: Int64) -> Date {
  Date(timeIntervalSince1970: Double(ms) / 1000)
}

func calendarMs(_ date: Date) -> Int64 {
  Int64((date.timeIntervalSince1970 * 1000).rounded())
}

/// One event as sent to the Rust side. The fields are the ones the bridge may read: no notes,
/// no attachments, no attendees. `location` is `null` when absent (not an omitted key).
struct CalendarEventOut: Encodable {
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

/// `{"kind":"calendar_events","events":[...]}`
struct CalendarEventsReply: Encodable {
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

/// `{"kind":"calendar_created","event_id":"..."}`, `event_id` null when EventKit gives none.
struct CalendarCreatedReply: Encodable {
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

/// The app's event store and the permission it reports. EventKit recommends one store per app
/// rather than one per call.
enum CalendarAccess {
  static let store = EKEventStore()

  /// The permission state, read without prompting. Below iOS 17: `unsupported`.
  static func statusName() -> String {
    guard #available(iOS 17.0, *) else { return "unsupported" }
    switch EKEventStore.authorizationStatus(for: .event) {
    case .fullAccess:
      return "granted"
    // Legacy value, given to a binary built before iOS 17: it grants full access.
    case .authorized:
      return "granted"
    // Write-only can create events but cannot read them: a read is refused, so the whole
    // capability reports `denied`. This is a decision, see the report.
    case .writeOnly, .denied, .restricted:
      return "denied"
    case .notDetermined:
      return "prompt"
    @unknown default:
      return "prompt"
    }
  }
}

extension ThisDevicePlugin {
  /// The calendar permission state, read without prompting the person.
  @objc public func calendarStatus(_ invoke: Invoke) {
    invoke.resolve(Reply.status(state: CalendarAccess.statusName()))
  }

  /// Asks for full access to events. The answer is the state the person chose.
  @objc public func calendarRequest(_ invoke: Invoke) {
    guard #available(iOS 17.0, *) else {
      invoke.resolve(Reply.status(state: "unsupported"))
      return
    }
    CalendarAccess.store.requestFullAccessToEvents { _, _ in
      DispatchQueue.main.async {
        invoke.resolve(Reply.status(state: CalendarAccess.statusName()))
      }
    }
  }

  /// The events of a window, sorted by start, at most `CalendarLimits.eventsMax`.
  @objc public func calendarEvents(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(CalendarWindowArgs.self)
    guard #available(iOS 17.0, *) else {
      invoke.resolve(Reply.error(code: CalendarCode.notSupported))
      return
    }
    guard isValidWindow(startMs: args.startMs, endMs: args.endMs) else {
      invoke.resolve(Reply.error(code: CalendarCode.internalError))
      return
    }
    guard CalendarAccess.statusName() == "granted" else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    let store = CalendarAccess.store
    let predicate = store.predicateForEvents(
      withStart: calendarDate(ms: args.startMs),
      end: calendarDate(ms: args.endMs),
      calendars: nil)
    let events = store.events(matching: predicate)
      .sorted { $0.startDate < $1.startDate }
      .prefix(CalendarLimits.eventsMax)
      .map { event in
        CalendarEventOut(
          title: event.title ?? "",
          startMs: calendarMs(event.startDate),
          endMs: calendarMs(event.endDate),
          location: trimmedText(event.location))
      }
    invoke.resolve(CalendarEventsReply(events: events))
  }

  /// Writes one event in the default calendar. Refuses an empty title and an end that is not
  /// after the start, as `internal`.
  @objc public func calendarCreateEvent(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(CalendarCreateArgs.self)
    guard #available(iOS 17.0, *) else {
      invoke.resolve(Reply.error(code: CalendarCode.notSupported))
      return
    }
    guard let title = trimmedText(args.title) else {
      invoke.resolve(Reply.error(code: CalendarCode.internalError))
      return
    }
    // A create has no 31-day bound: only the order of the two dates matters.
    let (span, overflow) = args.endMs.subtractingReportingOverflow(args.startMs)
    guard !overflow, span > 0 else {
      invoke.resolve(Reply.error(code: CalendarCode.internalError))
      return
    }
    guard CalendarAccess.statusName() == "granted" else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    let store = CalendarAccess.store
    guard let calendar = store.defaultCalendarForNewEvents else {
      invoke.resolve(Reply.error(code: ReplyCode.unavailable))
      return
    }
    let event = EKEvent(eventStore: store)
    event.title = title
    event.startDate = calendarDate(ms: args.startMs)
    event.endDate = calendarDate(ms: args.endMs)
    event.location = trimmedText(args.location)
    event.calendar = calendar
    do {
      try store.save(event, span: .thisEvent)
      invoke.resolve(CalendarCreatedReply(eventId: event.eventIdentifier))
    } catch {
      invoke.resolve(Reply.error(code: CalendarCode.internalError))
    }
  }
}

// ---- Les calendriers, la modification, la suppression -----------------------------------
//
// Un événement créé qu'on ne peut plus corriger est une promesse à moitié tenue. Ces trois
// méthodes donnent la liste des calendriers (avec leur droit d'écriture), la modification et la
// suppression. Mêmes bornes et mêmes contrôles que le reste du fichier.

private struct CalendarIdArgs: Decodable {
  let id: String
}

/// Un changement : l'identifiant, et ce qui change. Les champs absents ne changent rien.
private struct CalendarUpdateArgs: Decodable {
  let id: String
  let title: String?
  let startMs: Int64?
  let endMs: Int64?
  let location: String?
  let notes: String?

  private enum CodingKeys: String, CodingKey {
    case id, title, location, notes
    case startMs = "start_ms"
    case endMs = "end_ms"
  }
}

/// `{"kind":"calendars","calendars":[{…}]}`
private struct CalendarSummariesReply: Encodable {
  let kind = "calendars"
  let calendars: [CalendarSummaryOut]
}

private struct CalendarSummaryOut: Encodable {
  let id: String
  let title: String
  let allows_modifications: Bool
}

/// `{"kind":"event_updated","event_id":"…"}` / `{"kind":"event_deleted","event_id":"…"}`
private struct CalendarEventChangedReply: Encodable {
  let kind: String
  let event_id: String
}

extension ThisDevicePlugin {
  /// Tous les calendriers d'événements, avec leur droit d'écriture.
  @objc public func calendarCalendars(_ invoke: Invoke) throws {
    guard #available(iOS 17.0, *) else {
      invoke.resolve(Reply.error(code: CalendarCode.notSupported))
      return
    }
    guard CalendarAccess.statusName() == "granted" else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    let calendars = CalendarAccess.store.calendars(for: .event).map {
      CalendarSummaryOut(
        id: $0.calendarIdentifier,
        title: $0.title,
        allows_modifications: $0.allowsContentModifications
      )
    }
    invoke.resolve(CalendarSummariesReply(calendars: calendars))
  }

  /// Modifie un événement existant. Un champ absent ne change rien ; au moins un doit changer.
  @objc public func calendarUpdateEvent(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(CalendarUpdateArgs.self)
    guard #available(iOS 17.0, *) else {
      invoke.resolve(Reply.error(code: CalendarCode.notSupported))
      return
    }
    guard CalendarAccess.statusName() == "granted" else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    let store = CalendarAccess.store
    guard let event = store.event(withIdentifier: args.id) else {
      // Un identifiant que le calendrier ne connaît plus : `not_supported`, pas une panne.
      invoke.resolve(Reply.error(code: CalendarCode.notSupported))
      return
    }
    var changed = false
    if let title = trimmedText(args.title) {
      event.title = title
      changed = true
    }
    if let startMs = args.startMs {
      event.startDate = calendarDate(ms: startMs)
      changed = true
    }
    if let endMs = args.endMs {
      event.endDate = calendarDate(ms: endMs)
      changed = true
    }
    if let location = args.location {
      event.location = location
      changed = true
    }
    if let notes = args.notes {
      event.notes = notes
      changed = true
    }
    // Un patch qui ne change rien n'est pas envoyé : la personne verrait une écriture vide.
    guard changed else {
      invoke.resolve(Reply.error(code: CalendarCode.internalError))
      return
    }
    // Le résultat doit rester un événement qui finit après son début — vérifié sur l'objet
    // MODIFIÉ, puisque le pont Rust ne connaît pas les dates stockées.
    guard event.endDate > event.startDate else {
      invoke.resolve(Reply.error(code: CalendarCode.internalError))
      return
    }
    do {
      try store.save(event, span: .thisEvent)
      invoke.resolve(
        CalendarEventChangedReply(kind: "event_updated", event_id: args.id))
    } catch {
      invoke.resolve(Reply.error(code: CalendarCode.internalError))
    }
  }

  /// Supprime un événement (cette occurrence seulement, pas la série).
  @objc public func calendarDeleteEvent(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(CalendarIdArgs.self)
    guard #available(iOS 17.0, *) else {
      invoke.resolve(Reply.error(code: CalendarCode.notSupported))
      return
    }
    guard CalendarAccess.statusName() == "granted" else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    let store = CalendarAccess.store
    guard let event = store.event(withIdentifier: args.id) else {
      invoke.resolve(Reply.error(code: CalendarCode.notSupported))
      return
    }
    do {
      try store.remove(event, span: .thisEvent)
      invoke.resolve(
        CalendarEventChangedReply(kind: "event_deleted", event_id: args.id))
    } catch {
      invoke.resolve(Reply.error(code: CalendarCode.internalError))
    }
  }
}
