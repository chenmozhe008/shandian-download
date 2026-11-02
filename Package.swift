// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Shandian",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "Shandian", targets: ["Shandian"])],
    targets: [
        .target(name: "Isaac64", path: "Sources/Isaac64", publicHeadersPath: "include"),
        .executableTarget(name: "Shandian", dependencies: ["Isaac64"], path: "Sources/Shandian"),
        .testTarget(name: "ShandianTests", dependencies: ["Shandian"], path: "Tests/ShandianTests")
    ]
)
