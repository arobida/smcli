// swift-tools-version: 6.0
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let package = Package(
    name: "smcli",
    // ProximaKit's hybrid search requires macOS 14 (Accelerate/NaturalLanguage).
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "smcli", targets: ["smcli"]),
    ],
    dependencies: [
        .package(url: "https://github.com/vivekptnk/ProximaKit", from: "1.8.0"),
    ],
    targets: [
        // Apple platforms ship an SQLite3 module; Linux links the system library instead.
        .systemLibrary(name: "CSQLite", pkgConfig: "sqlite3", providers: [.apt(["libsqlite3-dev"])]),
        .executableTarget(name: "smcli", dependencies: [
            .target(name: "CSQLite", condition: .when(platforms: [.linux])),
            // Apple-only; on Linux `smcli search` reports that it is unavailable.
            .product(name: "ProximaKit", package: "ProximaKit", condition: .when(platforms: [.macOS])),
            .product(name: "ProximaEmbeddings", package: "ProximaKit", condition: .when(platforms: [.macOS])),
        ]),
        .testTarget(name: "smcliTests", dependencies: ["smcli"]),
    ]
)
