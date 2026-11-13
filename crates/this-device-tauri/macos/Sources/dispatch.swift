// Main entry points for the macOS device bridge, exported as C functions via @_cdecl.
//
// The Rust side calls these two functions:
// - macos_device_call(method, args)  for family operations (contacts, photos, etc.)
// - macos_device_status(capability)  for permission status checks
//
// The method names are the SAME as the iOS side (from device_bridge::METHODS), so the rest
// of the Rust code works unchanged.

import Foundation

/// The single entry point for all device family calls on macOS.
///
/// - Parameters:
///   - methodPtr: The native method name (e.g. "captureStatus", "contactsList")
///   - argsPtr: JSON arguments string
/// - Returns: JSON reply string (allocated with strdup, freed by Rust with libc::free)
@_cdecl("macos_device_call")
func macos_device_call(
    _ methodPtr: UnsafePointer<CChar>,
    _ argsPtr: UnsafePointer<CChar>
) -> UnsafeMutablePointer<CChar> {
    let method = String(cString: methodPtr)
    let args = decodeArgs(argsPtr)

    let result: String
    switch method {
    // ── Camera & Microphone (AVFoundation) ──────────────────────────
    case "captureStatus":   result = MacCameraMic.status(args)
    case "captureRequest":  result = MacCameraMic.request(args)
    case "cameraCapturePhoto": result = MacCameraMic.capturePhoto(args)
    case "cameraRecordVideo":  result = MacCameraMic.recordVideo(args)
    case "microphoneStart":    result = MacCameraMic.microphoneStart(args)
    case "microphoneStop":     result = MacCameraMic.microphoneStop(args)

    // ── Location (CoreLocation) ─────────────────────────────────────
    // L'ÉTAT de la permission, sans rien demander : le même besoin que sur iOS, et la carte
    // l'annonce désormais sur macOS aussi.
    case "locationStatus":   result = MacLocation.state(args)
    case "locationRequest":  result = MacLocation.request(args)
    case "locationSample":   result = MacLocation.sample(args)
    case "locationStreamStart": result = MacLocation.streamStart(args)
    case "locationStreamStop":  result = MacLocation.streamStop(args)
    case "locationRecent":      result = MacLocation.recent(args)

    // ── Contacts (Contacts framework) ───────────────────────────────
    case "contactsStatus":  result = MacContacts.status()
    case "contactsRequest": result = MacContacts.request()
    case "contactsList":    result = MacContacts.list(args)
    case "contactsGet":     result = MacContacts.get(args)
    case "contactsSearch":  result = MacContacts.search(args)
    case "contactsCreate":  result = MacContacts.create(args)

    // ── Calendar (EventKit) ─────────────────────────────────────────
    case "calendarStatus":      result = MacCalendar.status()
    case "calendarRequest":     result = MacCalendar.request()
    case "calendarEvents":      result = MacCalendar.events(args)
    case "calendarCreateEvent": result = MacCalendar.createEvent(args)
    case "calendarCalendars":   result = MacCalendar.calendars()
    case "calendarUpdateEvent": result = MacCalendar.updateEvent(args)
    case "calendarDeleteEvent": result = MacCalendar.deleteEvent(args)

    // ── Bluetooth (CoreBluetooth) ───────────────────────────────────
    case "bluetoothScan":   result = MacBluetooth.scan(args)
    case "bluetoothState":          result = MacBluetooth.radioState()
    case "bluetoothConnect":        result = MacBluetooth.connect(args)
    case "bluetoothDisconnect":     result = MacBluetooth.disconnect(args)
    case "bluetoothServices":       result = MacBluetooth.services(args)
    case "bluetoothRead":           result = MacBluetooth.read(args)
    case "bluetoothWrite":          result = MacBluetooth.write(args)
    case "bluetoothSubscribe":      result = MacBluetooth.subscribe(args)
    case "bluetoothNotifications":  result = MacBluetooth.notifications(args)

    // ── Biometrics / Touch ID (LocalAuthentication) ─────────────────
    case "faceIdStatus":    result = MacBiometrics.status()
    case "faceIdConfirm":   result = MacBiometrics.confirm(args)

    // ── Speech recognition (Speech framework) ───────────────────────
    case "speechStatus":         result = MacSpeech.status()
    case "speechRequest":        result = MacSpeech.request()
    case "speechTranscribeFile": result = MacSpeech.transcribeFile(args)

    // ── Notifications (UserNotifications) ───────────────────────────
    case "notificationsStatus":  result = MacNotifications.status()
    case "notificationsRequest": result = MacNotifications.request()
    case "notificationsPost":    result = MacNotifications.post(args)
    case "notificationsCancel":  result = MacNotifications.cancel(args)

    // ── Clipboard (AppKit / NSPasteboard) ───────────────────────────
    case "clipboardRead":   result = MacClipboard.read()
    case "clipboardWrite":  result = MacClipboard.write(args)

    // ── Share (AppKit / NSSharingService) ────────────────────────────
    case "shareText":       result = MacShare.shareText(args)
    case "shareItem":       result = MacShare.shareItem(args)

    // ── Files (AppKit / NSOpenPanel) ────────────────────────────────
    case "filesPick":       result = MacFiles.pick(args)
    case "filesRead":       result = MacFiles.read(args)
    case "filesWrite":      result = MacFiles.write(args)

    // ── Photos (Photos + PhotosUI) ──────────────────────────────────
    case "photosStatus":    result = MacPhotos.status()
    case "photosRequest":   result = MacPhotos.request()
    case "photosPick":      result = MacPhotos.pick(args)
    case "photosList":      result = MacPhotos.list(args)
    case "photosRead":      result = MacPhotos.readPhoto(args)

    // ── Local Network (Network / NWBrowser) ─────────────────────────
    case "localNetworkStatus": result = MacLocalNetwork.status()
    case "localNetworkBrowse": result = MacLocalNetwork.browse(args)

    // ── Live Activity (not available on macOS) ──────────────────────
    case "liveActivityStatus":
        result = statusReply(state: "unsupported")

    // ── NFC (not available on macOS) ────────────────────────────────
    case "nfcStatus", "nfcReadOnce":
        result = errorReply(code: ReplyCode.notSupported)

    // ── Motion / Health / Gyroscope (not available on macOS) ────────
    case "motionActivityStatus", "motionActivityRequest", "motionStepsSample",
         "healthStatus", "healthRequest", "healthQuantitySample",
         "gyroscopeSample":
        result = errorReply(code: ReplyCode.notSupported)

    default:
        result = errorReply(code: ReplyCode.notSupported)
    }

    return jsonResult(result)
}

/// The permission status of a capability on macOS.
///
/// - Parameter capabilityPtr: The snake_case capability name (e.g. "camera", "contacts")
/// - Returns: JSON status reply (allocated with strdup, freed by Rust)
@_cdecl("macos_device_status")
func macos_device_status(_ capabilityPtr: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar> {
    let capability = String(cString: capabilityPtr)

    let state: String
    switch capability {
    case "camera":          state = MacCameraMic.cameraState()
    case "microphone":      state = MacCameraMic.microphoneState()
    case "location":        state = MacLocation.currentState()
    case "location_always": state = MacLocation.currentState()
    case "contacts":        state = MacContacts.currentState()
    case "calendar":        state = MacCalendar.currentState()
    case "bluetooth":       state = MacBluetooth.currentState()
    case "face_id":         state = MacBiometrics.currentState()
    case "speech":          state = MacSpeech.currentState()
    case "notifications":   state = MacNotifications.currentState()
    case "photos":          state = MacPhotos.currentState()
    case "local_network":   state = MacLocalNetwork.currentState()

    // No permission needed on macOS
    case "clipboard", "share", "photos_pick", "files_pick":
        state = "granted"

    // Not available on macOS
    case "gyroscope", "motion_activity", "health", "nfc", "live_activity":
        state = "unsupported"

    default:
        state = "unsupported"
    }

    return jsonResult(statusReply(state: state))
}
