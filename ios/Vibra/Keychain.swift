import Foundation
import Security

enum Keychain {
  static func read(_ name: String) throws -> Data? {
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.vibra.mobile.remote.v1", kSecAttrAccount as String: name,
      kSecReturnData as String: true, kSecMatchLimit as String: kSecMatchLimitOne,
    ]
    var result: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &result)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess else {
      throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
    }
    return result as? Data
  }
  static func write(_ name: String, _ value: Data) throws {
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.vibra.mobile.remote.v1", kSecAttrAccount as String: name,
    ]
    let status = SecItemUpdate(
      query as CFDictionary, [kSecValueData as String: value] as CFDictionary)
    if status == errSecItemNotFound {
      var item = query
      item[kSecValueData as String] = value
      item[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
      let added = SecItemAdd(item as CFDictionary, nil)
      guard added == errSecSuccess else {
        throw NSError(domain: NSOSStatusErrorDomain, code: Int(added))
      }
    } else if status != errSecSuccess {
      throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
    }
  }
  static func remove(_ name: String) {
    SecItemDelete(
      [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "app.vibra.mobile.remote.v1", kSecAttrAccount as String: name,
      ] as CFDictionary)
  }
}

#if targetEnvironment(simulator)
  extension Keychain {
    /// Exercises the installed app's actual signing context, using an isolated item.
    /// It never reads pairing credentials and is only activated by the CLI test.
    static func runSimulatorProbeIfRequested() {
      guard let marker = ProcessInfo.processInfo.environment["VIBRA_KEYCHAIN_PROBE"],
        UUID(uuidString: marker) != nil
      else { return }
      let name = "probe-" + marker
      let url = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("keychain-probe-" + marker + ".json")
      var report: [String: Any] = [:]
      do {
        defer { remove(name) }
        let expected = Data("Vibra keychain integration test".utf8)
        try write(name, expected)
        guard try read(name) == expected else {
          throw NSError(domain: "VibraKeychainProbe", code: 1)
        }
        remove(name)
        guard try read(name) == nil else { throw NSError(domain: "VibraKeychainProbe", code: 2) }
        report = ["ok": true]
      } catch {
        let error = error as NSError
        report = ["ok": false, "domain": error.domain, "code": error.code]
      }
      if let data = try? JSONSerialization.data(withJSONObject: report) {
        try? data.write(to: url, options: .atomic)
      }
    }
  }
#endif
