// swift-tools-version:6.0
import PackageDescription
let package = Package(
    name: "VibraRemoteProtocol",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "VibraRemoteProtocol", targets: ["VibraRemoteProtocol"])],
    targets: [
        .target(name: "VibraRemoteProtocol"),
        .testTarget(name: "VibraRemoteProtocolTests", dependencies: ["VibraRemoteProtocol"], resources: [.copy("Fixtures")])
    ]
)
