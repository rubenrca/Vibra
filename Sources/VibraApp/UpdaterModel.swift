import Foundation
import Sparkle
import SwiftUI

/// Wraps Sparkle so nothing else in the app has to import it.
///
/// The updater is only started for a real application bundle carrying a feed
/// URL. A `swift run` build has no bundle, no `SUFeedURL` and no code
/// signature, and starting Sparkle there produces a modal error on every
/// launch, so development builds get an inert updater whose menu item stays
/// disabled.
@MainActor
final class UpdaterModel: ObservableObject {
    @Published private(set) var canCheckForUpdates = false
    @Published var automaticallyChecksForUpdates: Bool {
        didSet { controller?.updater.automaticallyChecksForUpdates = automaticallyChecksForUpdates }
    }

    /// False for development builds, so the UI can explain why updating is
    /// unavailable instead of showing a control that does nothing.
    let isConfigured: Bool

    private let controller: SPUStandardUpdaterController?
    private var canCheckObservation: NSKeyValueObservation?

    init() {
        let bundle = Bundle.main
        guard bundle.bundleURL.pathExtension == "app",
              bundle.object(forInfoDictionaryKey: "SUFeedURL") != nil
        else {
            controller = nil
            isConfigured = false
            automaticallyChecksForUpdates = false
            return
        }

        let controller = SPUStandardUpdaterController(
            startingUpdater: true,
            updaterDelegate: nil,
            userDriverDelegate: nil
        )
        self.controller = controller
        isConfigured = true
        automaticallyChecksForUpdates = controller.updater.automaticallyChecksForUpdates

        // Sparkle drives `canCheckForUpdates` from an update check already in
        // flight, so the menu item has to follow it rather than be set once.
        canCheckObservation = controller.updater.observe(
            \.canCheckForUpdates,
            options: [.initial, .new]
        ) { [weak self] _, change in
            guard let value = change.newValue else { return }
            Task { @MainActor in self?.canCheckForUpdates = value }
        }
    }

    func checkForUpdates() {
        controller?.checkForUpdates(nil)
    }

    /// The version the user is running, for the Settings pane.
    var currentVersion: String {
        let bundle = Bundle.main
        let short = bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        let build = bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        switch (short, build) {
        case let (short?, build?): return "\(short) (\(build))"
        case let (short?, nil): return short
        default: return "development build"
        }
    }
}
