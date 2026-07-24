// swift-tools-version: 6.2

import PackageDescription

let package = Package(
    name: "Vibra",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(name: "Vibra", targets: ["VibraApp"]),
    ],
    dependencies: [
        .package(
            url: "https://github.com/egoist-labs/libghostty-spm.git",
            revision: "07f258b077048888f36b9f7ad7df3c40cdaa25d1"
        ),
    ],
    targets: [
        .target(name: "VibraCore"),
        .executableTarget(
            name: "VibraApp",
            dependencies: [
                "VibraCore",
                .product(name: "GhosttyTerminal", package: "libghostty-spm"),
            ]
        ),
        .testTarget(
            name: "VibraCoreTests",
            dependencies: ["VibraCore"]
        ),
        .testTarget(
            name: "VibraAppTests",
            dependencies: ["VibraApp"]
        ),
    ]
)
