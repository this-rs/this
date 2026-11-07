// Wire format of the device bridge. Every answer is one JSON object with a `kind`, and the
// shape is the serde contract of `this_platform::device_bridge::Reply` on the Rust side:
//
//   {"kind":"gyroscope","x":..,"y":..,"z":..,"ts_ms":..}
//   {"kind":"devices","devices":[{"id":"..","name":null,"rssi":-60}]}
//   {"kind":"error","code":"unavailable"}
//   {"kind":"status","state":"granted"}
//
// Encoded by hand: `Optional` must become `null` (not an absent key), and `ts_ms` must be an
// integer JSON number (serde rejects `1.7e12` or `1700000000000.0` for a `u64`).

import Foundation

/// A peripheral seen by a scan.
struct BtDevice {
  let id: String
  let name: String?
  let rssi: Int
}

enum Reply: Encodable {
  case gyroscope(x: Double, y: Double, z: Double, tsMs: Int64)
  case devices([BtDevice])
  case error(code: String)
  case status(state: String)

  private enum Key: String, CodingKey {
    case kind, x, y, z, ts_ms, devices, id, name, rssi, code, state
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    switch self {
    case let .gyroscope(x, y, z, tsMs):
      try c.encode("gyroscope", forKey: .kind)
      try c.encode(x, forKey: .x)
      try c.encode(y, forKey: .y)
      try c.encode(z, forKey: .z)
      try c.encode(tsMs, forKey: .ts_ms)
    case let .devices(list):
      try c.encode("devices", forKey: .kind)
      try c.encode(list, forKey: .devices)
    case let .error(code):
      try c.encode("error", forKey: .kind)
      try c.encode(code, forKey: .code)
    case let .status(state):
      try c.encode("status", forKey: .kind)
      try c.encode(state, forKey: .state)
    }
  }
}

extension BtDevice: Encodable {
  private enum Key: String, CodingKey {
    case id, name, rssi
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode(id, forKey: .id)
    // `encode` (not `encodeIfPresent`) writes `null` for an unknown name.
    try c.encode(name, forKey: .name)
    try c.encode(rssi, forKey: .rssi)
  }
}

/// Error codes, as the Rust side names them (`DeviceError`, snake_case).
enum ReplyCode {
  static let unavailable = "unavailable"
  static let notAuthorized = "not_authorized"
  static let timeout = "timeout"
}
