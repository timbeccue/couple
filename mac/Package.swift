// swift-tools-version: 5.10
// CoupleKit + Couple.app (SPEC §3.1 / §11). The package links couple-core's C ABI
// (ffi/couple.h, libcouple.a) built by `cargo build --release -p couple-ffi`.
// Open this Package.swift directly in Xcode, or build with Scripts/build.sh.

import PackageDescription
import Foundation

// Absolute path to the repo's cargo output, derived from this manifest's location
// so the link flags work from both `swift build` and Xcode.
let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path
let rustLibDir = "\(packageDir)/../target/release"

let package = Package(
    name: "Couple",
    platforms: [.macOS(.v14)],
    targets: [
        .target(
            name: "CCouple",
            linkerSettings: [
                .unsafeFlags(["-L\(rustLibDir)"]),
                .linkedLibrary("couple"),
            ]
        ),
        .target(
            name: "CoupleKit",
            dependencies: ["CCouple"]
        ),
        .executableTarget(
            name: "Couple",
            dependencies: ["CoupleKit"]
        ),
        .testTarget(
            name: "CoupleKitTests",
            dependencies: ["CoupleKit"]
        ),
    ]
)
