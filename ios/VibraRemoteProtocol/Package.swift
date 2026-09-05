// swift-tools-version:6.0
import PackageDescription
let package = Package(
    name: "VibraRemoteProtocol",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "VibraRemoteProtocol", targets: ["VibraRemoteProtocol"])],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm.git", exact: "1.20.0"),
        .package(url: "https://github.com/swift-libp2p/swift-noise.git", revision: "0adfd28786322784860fb3c8f228591c6e8fd92f"),
        .package(url: "https://github.com/apple/swift-crypto.git", from: "4.0.0")
    ],
    targets: [
        .target(name: "VibraRemoteProtocol", dependencies: [.product(name: "Noise", package: "swift-noise"), .product(name: "Crypto", package: "swift-crypto")]),
        .testTarget(name: "VibraRemoteProtocolTests", dependencies: ["VibraRemoteProtocol", .product(name: "SwiftTerm", package: "SwiftTerm")], resources: [.copy("Fixtures")])
    ]
)
