// swift-tools-version: 5.9
import PackageDescription

let package = Package(name: "LanCastContracts", platforms: [.macOS("15.0"), .iOS(.v16), .tvOS(.v17)],
    products: [.library(name: "LanCastContracts", targets: ["LanCastContracts"])],
    targets: [.target(name: "LanCastContracts"), .testTarget(name: "LanCastContractsTests", dependencies: ["LanCastContracts"])])
