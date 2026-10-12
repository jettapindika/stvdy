import Foundation
import UIKit
import UniformTypeIdentifiers

/// The iOS Share Extension (A312).
///
/// **The extension cannot launch its host app.** `NSExtensionContext.open(_:)` is
/// supported only by the Today and iMessage extension points, so a Share Extension
/// that "opens Stvdy" cannot; the whole design follows from that. What it can do is
/// write into a container both processes own — an App Group — and return. The app
/// imports the inbox on its next foreground, which is why the file arrives without the
/// extension having launched anything.
///
/// **Nothing here runs the Rust shell.** This target links UIKit and Foundation only:
/// no Tauri, no `libapp.a`, no webview. A share can arrive with the app suspended, so
/// there is no engine to call and no window to talk to, and starting one from an
/// extension is not something iOS permits.
///
/// **The layout is the app's, not this file's.** The bytes land at `<inbox>/<id>.<ext>`
/// and the metadata beside them at `<inbox>/<id>.json`, which is the same convention
/// the Android intake writes through the mobile plugin's Rust half. The app's reader
/// (`src-tauri/src/share.rs`) owns that format; this file conforms to it.
final class ShareViewController: UIViewController {

  /// The App Group the extension and the app share. One spelling: `tests/share_group.rs`
  /// asserts this literal appears exactly once here and exactly once in
  /// `src-tauri/src/share.rs`, and that no other container name exists in the shell.
  private static let appGroup = "group.app.stvdy"

  /// The inbox inside the container. The same string the app's reader joins.
  private static let inbox = "Inbox"

  private let status = UILabel()

  override func viewDidLoad() {
    super.viewDidLoad()
    view.backgroundColor = .systemBackground
    status.text = "Saving to Stvdy…"
    status.textAlignment = .center
    status.translatesAutoresizingMaskIntoConstraints = false
    view.addSubview(status)
    NSLayoutConstraint.activate([
      status.centerXAnchor.constraint(equalTo: view.centerXAnchor),
      status.centerYAnchor.constraint(equalTo: view.centerYAnchor),
    ])
  }

  override func viewDidAppear(_ animated: Bool) {
    super.viewDidAppear(animated)
    Task { await importSharedItems() }
  }

  /// Copies every attachment into the inbox and completes the request.
  ///
  /// `completeRequest` is called on every path, including the failures: an extension
  /// that never completes holds the share sheet open until the system kills it, which
  /// looks to the user like a hang rather than a failure.
  private func importSharedItems() async {
    guard let context = extensionContext else { return }

    guard
      let container = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: Self.appGroup
      )
    else {
      // No entitlement, no container. Nothing this extension can do about it — and the
      // reason the App Group is asserted in the shell's own test rather than trusted.
      NSLog("StvdyShare: the App Group container is unavailable; is the entitlement set?")
      finish(context, message: "Stvdy could not be reached")
      return
    }

    let inbox = container.appendingPathComponent(Self.inbox, isDirectory: true)
    do {
      try FileManager.default.createDirectory(at: inbox, withIntermediateDirectories: true)
    } catch {
      NSLog("StvdyShare: the inbox could not be created: \(error)")
      finish(context, message: "Stvdy could not be reached")
      return
    }

    var saved = 0
    for case let item as NSExtensionItem in context.inputItems {
      for provider in item.attachments ?? [] {
        if await copy(provider: provider, into: inbox, item: item) {
          saved += 1
        }
      }
    }
    finish(context, message: saved > 0 ? "Saved to Stvdy" : "Nothing to save")
  }

  private func finish(_ context: NSExtensionContext, message: String) {
    status.text = message
    // An empty `returningItems` is right: this extension hands nothing back to the
    // sending app, it files the file for Stvdy to pick up.
    context.completeRequest(returningItems: [], completionHandler: nil)
  }

  /// Copies one provider's file representation into the inbox, with its sidecar.
  ///
  /// Returns whether anything landed, so the caller can say so honestly.
  private func copy(provider: NSItemProvider, into inbox: URL, item: NSExtensionItem) async -> Bool {
    let identifier = provider.registeredTypeIdentifiers.first ?? UTType.data.identifier
    guard let source = await load(provider: provider, identifier: identifier) else {
      return false
    }

    let id = UUID().uuidString
    let ext = source.pathExtension.isEmpty ? "dat" : source.pathExtension
    let destination = inbox.appendingPathComponent("\(id).\(ext)")
    do {
      try FileManager.default.copyItem(at: source, to: destination)
    } catch {
      NSLog("StvdyShare: the shared file could not be copied: \(error)")
      return false
    }

    // The name the sending app showed is metadata and is never joined onto a path —
    // the bytes' own name is the id. A name from another app is untrusted input.
    let name =
      provider.suggestedName
      ?? item.attributedTitle?.string
      ?? "shared-\(ext)"

    var sidecar: [String: Any] = [
      "id": id,
      "name": name,
      "path": destination.path,
    ]
    // `null` rather than an absent key: the app's reader deserializes `Option<String>`,
    // and an absent key is the same thing but only by serde's default. Writing it is
    // the honest spelling of "the intent carried no type".
    sidecar["mime"] = UTType(identifier)?.preferredMIMEType ?? NSNull()
    do {
      let encoded = try JSONSerialization.data(withJSONObject: sidecar)
      try encoded.write(to: inbox.appendingPathComponent("\(id).json"), options: .atomic)
    } catch {
      // A sidecar without bytes is a row the app would show and then fail to upload,
      // so the bytes go too.
      try? FileManager.default.removeItem(at: destination)
      NSLog("StvdyShare: the sidecar could not be written: \(error)")
      return false
    }
    return true
  }

  /// Reads a provider's file representation into a stable location.
  ///
  /// The URL `loadFileRepresentation` hands the completion handler is **deleted as soon
  /// as the handler returns**, so the bytes are copied out inside the closure. A
  /// `loadItem`-style read that returned the URL directly would hand the caller a path
  /// to a file that no longer exists by the time it is used.
  private func load(provider: NSItemProvider, identifier: String) async -> URL? {
    await withCheckedContinuation { continuation in
      provider.loadFileRepresentation(forTypeIdentifier: identifier) { url, error in
        guard let url else {
          NSLog("StvdyShare: the item could not be loaded: \(String(describing: error))")
          continuation.resume(returning: nil)
          return
        }
        let staged = FileManager.default.temporaryDirectory
          .appendingPathComponent("\(UUID().uuidString).\(url.pathExtension)")
        do {
          try FileManager.default.copyItem(at: url, to: staged)
          continuation.resume(returning: staged)
        } catch {
          NSLog("StvdyShare: the item could not be staged: \(error)")
          continuation.resume(returning: nil)
        }
      }
    }
  }
}
