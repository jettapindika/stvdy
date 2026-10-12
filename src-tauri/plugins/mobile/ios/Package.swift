// swift-tools-version:5.7

import PackageDescription

// The iOS half of the mobile plugin (A311): the `BGTaskScheduler` registration and
// the two handlers. Built by `tauri_plugin::Builder`'s `ios_path` link step, which
// copies the Tauri iOS API into `../.tauri/tauri-api` before SwiftPM runs — hence the
// relative dependency below.
let package = Package(
  name: "tauri-plugin-stvdy-mobile",
  platforms: [
    .iOS(.v15)
  ],
  products: [
    .library(
      name: "tauri-plugin-stvdy-mobile",
      type: .static,
      targets: ["tauri-plugin-stvdy-mobile"])
  ],
  dependencies: [
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    // No `testTarget`: SwiftPM requires `Tests/<name>` to exist for one, and this
    // crate's tests are the Rust source assertions in `tests/scheduler.rs`. A test
    // target with no sources fails the whole build
    // ("Source files for target PluginTests should be located under Tests/PluginTests").
    .target(
      name: "tauri-plugin-stvdy-mobile",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources")
  ]
)
