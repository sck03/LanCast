// swift-tools-version: 5.9
import PackageDescription

let package = Package(name: "LanCastContracts", platforms: [.macOS(.v13), .iOS(.v16), .tvOS(.v17)],
    products: [.library(name: "LanCastContracts", targets: ["LanCastContracts"])],
    targets: [.target(name: "LanCastContracts"), .testTarget(name: "LanCastContractsTests", dependencies: ["LanCastContracts"])])
