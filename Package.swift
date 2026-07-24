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
        .package(
            url: "https://github.com/sparkle-project/Sparkle.git",
            from: "2.9.4"
        ),
    ],
    targets: [
        .target(name: "VibraCore"),
        .executableTarget(
            name: "VibraApp",
            dependencies: [
                "VibraCore",
                .product(name: "GhosttyTerminal", package: "libghostty-spm"),
                .product(name: "Sparkle", package: "Sparkle"),
            ],
            // Sparkle ships as a framework that Scripts/package_app.sh copies into
            // Contents/Frameworks. SwiftPM only adds an rpath to its own build
            // directory, which does not exist on a user's machine, so the bundle
            // layout has to be spelled out here for the packaged app to launch.
            linkerSettings: [
                .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"])
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
