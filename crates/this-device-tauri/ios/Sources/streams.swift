// Flux d'état système, SANS consentement : orientation de l'écran et niveau de batterie.
//
// Ni permission, ni entitlement, ni donnée personnelle — les mêmes lectures que
// `system_state.swift`, mais poussées au lieu d'être demandées. Rust ouvre un `Channel`
// (`tauri::ipc::Channel::new`, côté hôte), le passe à `streamStart`, et chaque échantillon
// y est envoyé : d'abord un échantillon initial, tout de suite, puis un par notification.
//
// Fil : `{"kind":"orientation","angle":…}` et `{"kind":"battery","level":…,"state":…}`.
// `angle` est une valeur FERMÉE (quatre choix), jamais du texte libre.

import Foundation
import Tauri
import UIKit

private struct SystemStreamStartArgs: Decodable {
  let stream: String
  let channel: Channel
}

private struct SystemStreamStopArgs: Decodable {
  let stream: String
}

/// Les abonnements vivants, un par flux. Une extension ne porte pas de propriété stockée :
/// l'état vit ici, et n'est touché que sur le fil principal.
private final class StreamRegistry {
  static let shared = StreamRegistry()
  var observers: [String: [NSObjectProtocol]] = [:]
  var channels: [String: Channel] = [:]
}

/// L'orientation de l'ÉCRAN (celle de l'interface), pas celle du boîtier : posé à plat, le
/// boîtier est `faceUp`, ce qui n'est aucune des quatre valeurs du flux ; l'écran, lui, en a
/// toujours une.
private func screenAngle() -> String {
  let scene = UIApplication.shared.connectedScenes
    .compactMap { $0 as? UIWindowScene }
    .first
  switch scene?.interfaceOrientation ?? .portrait {
  case .portraitUpsideDown: return "portrait_inverse"
  case .landscapeLeft: return "paysage_gauche"
  case .landscapeRight: return "paysage_droit"
  default: return "portrait"
  }
}

private func batterySample() -> JsonObject {
  let device = UIDevice.current
  let state: String
  switch device.batteryState {
  case .charging: state = "charging"
  case .full: state = "full"
  case .unplugged: state = "unplugged"
  case .unknown: state = "unknown"
  @unknown default: state = "unknown"
  }
  return ["kind": "battery", "level": Double(device.batteryLevel), "state": state]
}

extension ThisDevicePlugin {
  /// Démarre un flux (`orientation` ou `battery`). Un second départ remplace le premier.
  @objc public func streamStart(_ invoke: Invoke) {
    guard let args = try? invoke.parseArgs(SystemStreamStartArgs.self) else {
      invoke.resolve(["kind": "error", "code": "bad_args"])
      return
    }
    DispatchQueue.main.async {
      let registry = StreamRegistry.shared
      let center = NotificationCenter.default
      if let previous = registry.observers[args.stream] {
        previous.forEach { center.removeObserver($0) }
        // Les notifications d'orientation sont comptées : un départ = une fin.
        if args.stream == "orientation" {
          UIDevice.current.endGeneratingDeviceOrientationNotifications()
        }
      }
      let channel = args.channel
      var observers: [NSObjectProtocol] = []
      switch args.stream {
      case "orientation":
        UIDevice.current.beginGeneratingDeviceOrientationNotifications()
        let send = {
          let sample: JsonObject = ["kind": "orientation", "angle": screenAngle()]
          channel.send(sample)
        }
        observers.append(
          center.addObserver(
            forName: UIDevice.orientationDidChangeNotification, object: nil, queue: .main
          ) { _ in
            // L'interface tourne APRÈS le boîtier : on relit l'écran une fois la rotation faite.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { send() }
          })
        send()
      case "battery":
        UIDevice.current.isBatteryMonitoringEnabled = true
        let send = { channel.send(batterySample()) }
        for name in [
          UIDevice.batteryLevelDidChangeNotification, UIDevice.batteryStateDidChangeNotification,
        ] {
          observers.append(center.addObserver(forName: name, object: nil, queue: .main) { _ in send() })
        }
        send()
      default:
        invoke.resolve(["kind": "error", "code": "not_supported"])
        return
      }
      registry.observers[args.stream] = observers
      registry.channels[args.stream] = channel
      invoke.resolve(["kind": "stream", "stream": args.stream, "state": "started"])
    }
  }

  /// Arrête un flux. Arrêter un flux qui ne tourne pas n'est pas une erreur.
  @objc public func streamStop(_ invoke: Invoke) {
    guard let args = try? invoke.parseArgs(SystemStreamStopArgs.self) else {
      invoke.resolve(["kind": "error", "code": "bad_args"])
      return
    }
    DispatchQueue.main.async {
      let registry = StreamRegistry.shared
      let running = registry.observers[args.stream]
      running?.forEach { NotificationCenter.default.removeObserver($0) }
      registry.observers[args.stream] = nil
      registry.channels[args.stream] = nil
      if running != nil && args.stream == "orientation" {
        UIDevice.current.endGeneratingDeviceOrientationNotifications()
      }
      invoke.resolve(["kind": "stream", "stream": args.stream, "state": "stopped"])
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
final class FamilleStreams: NSObject {}
