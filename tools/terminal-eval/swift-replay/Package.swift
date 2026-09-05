// swift-tools-version:6.0
import PackageDescription
import Foundation

let package = Package(
    name: "TerminalReplay",
    platforms: [.macOS(.v14)],
    dependencies: [.package(name: "SwiftTerm", path: ProcessInfo.processInfo.environment["SWIFTTERM_SOURCE"]!)],
    targets: [.executableTarget(name: "TerminalReplay", dependencies: [.product(name: "SwiftTerm", package: "SwiftTerm")])]
)
