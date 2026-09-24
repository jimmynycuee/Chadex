// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "Chadex",
    defaultLocalization: "en",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .executable(name: "Chadex", targets: ["ChadexApp"])
    ],
    targets: [
        .executableTarget(
            name: "ChadexApp",
            path: "Sources/ChadexApp",
            resources: [
                .process("Resources")
            ],
            linkerSettings: [
                .linkedFramework("Security"),
                .linkedFramework("ServiceManagement")
            ]
        ),
        .testTarget(
            name: "ChadexAppTests",
            dependencies: ["ChadexApp"],
            path: "Tests/ChadexAppTests"
        )
    ]
)
