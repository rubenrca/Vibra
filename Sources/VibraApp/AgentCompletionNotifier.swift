import AppKit
import Foundation
import UserNotifications

struct AgentCompletionEvent: Equatable, Sendable {
    let agent: CodingAgent
    let succeeded: Bool?
    let finishedAt: Date

    static func transition(
        from previous: AgentActivity?,
        to current: AgentActivity
    ) -> AgentCompletionEvent? {
        guard case .finished(let agent, let succeeded, let finishedAt) = current else {
            return nil
        }
        if case .finished(_, _, let previousFinishedAt) = previous,
           previousFinishedAt == finishedAt {
            return nil
        }
        return AgentCompletionEvent(
            agent: agent,
            succeeded: succeeded,
            finishedAt: finishedAt
        )
    }
}

@MainActor
final class AgentCompletionNotifier {
    static let shared = AgentCompletionNotifier()

    private init() {}

    /// UserNotifications requires a real application bundle. SwiftPM's
    /// `swift run Vibra` executable has no bundle proxy, and merely asking for
    /// the current notification center raises an Objective-C exception.
    var isAvailable: Bool {
        Self.isBundledApplication(
            bundleURL: Bundle.main.bundleURL,
            bundleIdentifier: Bundle.main.bundleIdentifier
        )
    }

    nonisolated static func isBundledApplication(
        bundleURL: URL,
        bundleIdentifier: String?
    ) -> Bool {
        bundleURL.pathExtension.localizedCaseInsensitiveCompare("app") == .orderedSame
            && bundleIdentifier?.isEmpty == false
    }

    func requestAuthorizationIfEnabled() {
        guard isAvailable,
              UserDefaults.standard.bool(
            forKey: SettingsKeys.agentCompletionNotificationsEnabled
        ) else { return }
        UNUserNotificationCenter.current().requestAuthorization(
            options: [.alert, .sound]
        ) { _, _ in }
    }

    func notify(
        event: AgentCompletionEvent,
        workspaceTitle: String,
        workspaceIsVisible: Bool
    ) {
        guard isAvailable,
              UserDefaults.standard.bool(
            forKey: SettingsKeys.agentCompletionNotificationsEnabled
        ) else { return }

        // The selected pane already shows the completed state. Avoid a banner
        // while the user is looking directly at it, but notify for hidden panes
        // and whenever Vibra is in the background.
        guard !NSApp.isActive || !workspaceIsVisible else { return }

        let content = UNMutableNotificationContent()
        content.title = event.succeeded == false
            ? "\(event.agent.displayName) command failed"
            : "\(event.agent.displayName) finished"
        content.body = workspaceTitle
        content.sound = .default
        content.threadIdentifier = "agent-completion"

        let request = UNNotificationRequest(
            identifier: "agent-\(event.agent.rawValue)-\(event.finishedAt.timeIntervalSince1970)",
            content: content,
            trigger: nil
        )
        UNUserNotificationCenter.current().add(request) { _ in }
    }
}
