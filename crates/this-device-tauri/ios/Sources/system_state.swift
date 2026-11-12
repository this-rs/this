// État système, en lecture seule, SANS consentement.
//
// Ces méthodes ne touchent à rien de personnel : modèle et OS de l'appareil, niveau de
// batterie, connectivité, langue/région, luminosité de l'écran. Elles n'ont ni permission à
// demander ni entitlement à porter — c'est ce qui les distingue des familles `device.*`,
// qui passent par le consentement (voir lib.rs, `system_state`).
//
// Le volume système n'est PAS lisible sur iOS sans session audio active : `AVAudioSession.
// outputVolume` renvoie alors une valeur par défaut, pas le vrai volume. La méthode `volume`
// le refuse donc honnêtement (`not_supported`) plutôt que d'inventer un nombre.
//
// Fil : un objet JSON avec un `kind`, encodé à la main comme Reply.swift.

import AVFoundation
import Foundation
import Network
import Tauri
import UIKit

enum SystemStateReply: Encodable {
  case deviceInfo(model: String, os: String, osVersion: String)
  case battery(level: Double, state: String)
  case network(connected: Bool, interface: String)
  case locale(language: String, region: String)
  case screen(brightness: Double, scale: Double)
  case error(code: String)

  private enum Key: String, CodingKey {
    case kind, model, os, os_version, level, state, connected, interface
    case language, region, brightness, scale, code
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    switch self {
    case let .deviceInfo(model, os, osVersion):
      try c.encode("device_info", forKey: .kind)
      try c.encode(model, forKey: .model)
      try c.encode(os, forKey: .os)
      try c.encode(osVersion, forKey: .os_version)
    case let .battery(level, state):
      try c.encode("battery", forKey: .kind)
      try c.encode(level, forKey: .level)
      try c.encode(state, forKey: .state)
    case let .network(connected, interface):
      try c.encode("network", forKey: .kind)
      try c.encode(connected, forKey: .connected)
      try c.encode(interface, forKey: .interface)
    case let .locale(language, region):
      try c.encode("locale", forKey: .kind)
      try c.encode(language, forKey: .language)
      try c.encode(region, forKey: .region)
    case let .screen(brightness, scale):
      try c.encode("screen", forKey: .kind)
      try c.encode(brightness, forKey: .brightness)
      try c.encode(scale, forKey: .scale)
    case let .error(code):
      try c.encode("error", forKey: .kind)
      try c.encode(code, forKey: .code)
    }
  }
}

extension ThisDevicePlugin {
  /// Modèle, OS et version du système. Aucune donnée personnelle : ni nom, ni identifiant.
  @objc public func deviceInfo(_ invoke: Invoke) {
    let device = UIDevice.current
    invoke.resolve(
      SystemStateReply.deviceInfo(
        model: device.model,
        os: device.systemName,
        osVersion: device.systemVersion))
  }

  /// Niveau de batterie (0…1, -1 si inconnu) et son état. Sur simulateur, `level` vaut -1.
  @objc public func batteryLevel(_ invoke: Invoke) {
    let device = UIDevice.current
    device.isBatteryMonitoringEnabled = true
    let level = Double(device.batteryLevel)
    let state: String
    switch device.batteryState {
    case .charging: state = "charging"
    case .full: state = "full"
    case .unplugged: state = "unplugged"
    case .unknown: state = "unknown"
    @unknown default: state = "unknown"
    }
    invoke.resolve(SystemStateReply.battery(level: level, state: state))
  }

  /// Connectivité : connecté ou non, et par quel type d'interface. Un chemin n'arrive pas en
  /// quelques secondes, on répond `unavailable` plutôt que de laisser la promesse pendre.
  @objc public func networkState(_ invoke: Invoke) {
    DispatchQueue.global().async {
      let monitor = NWPathMonitor()
      let queue = DispatchQueue(label: "this-device.network-state")
      let semaphore = DispatchSemaphore(value: 0)
      var path: NWPath?
      monitor.pathUpdateHandler = { p in
        path = p
        semaphore.signal()
      }
      monitor.start(queue: queue)
      _ = semaphore.wait(timeout: .now() + 3.0)
      monitor.cancel()
      let reply: SystemStateReply
      if let path = path {
        if path.status == .satisfied {
          let interface: String
          if path.usesInterfaceType(.wifi) { interface = "wifi" }
          else if path.usesInterfaceType(.cellular) { interface = "cellular" }
          else if path.usesInterfaceType(.wiredEthernet) { interface = "ethernet" }
          else { interface = "other" }
          reply = .network(connected: true, interface: interface)
        } else {
          reply = .network(connected: false, interface: "none")
        }
      } else {
        reply = .error(code: "unavailable")
      }
      DispatchQueue.main.async {
        invoke.resolve(reply)
      }
    }
  }

  /// Langue et région du système, telles que `Locale.current` les lit.
  @objc public func locale(_ invoke: Invoke) {
    let current = Locale.current
    invoke.resolve(
      SystemStateReply.locale(
        language: current.languageCode ?? "?",
        region: current.regionCode ?? "?"))
  }

  /// Luminosité (0…1) et échelle de l'écran principal.
  @objc public func screenState(_ invoke: Invoke) {
    let screen = UIScreen.main
    invoke.resolve(
      SystemStateReply.screen(
        brightness: Double(screen.brightness),
        scale: Double(screen.scale)))
  }

  /// Le volume système n'est pas lisible sur iOS sans session audio active : on le refuse
  /// honnêtement au lieu de renvoyer une valeur qui ne serait pas le vrai volume.
  @objc public func volume(_ invoke: Invoke) {
    invoke.resolve(SystemStateReply.error(code: "not_supported"))
  }
}

/// Ancre de l'éditeur de liens — voir `ThisDevicePlugin.famillesAncrees`.
///
/// Un `#selector(Foo.bar)` ne produit pas une référence au SYMBOLE de `bar` : il produit une
/// chaîne. Le dead-strip retire donc l'objet entier, et le pont répond « No command bar found
/// for plugin this-device ». Mesuré le 13/11/2026 sur un iPhone 16 Pro : dix-huit méthodes de
/// familles compilaient, étaient listées dans `famillesLiees`, et étaient **absentes du binaire**.
/// Référencer une CLASSE de ce fichier garde son objet — donc ses méthodes.
final class FamilleSystemState: NSObject {}
