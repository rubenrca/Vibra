import Foundation

/// Which rule produced the Files/Git panel root.
enum PanelRootSource: String, Equatable, Sendable {
    /// Anchored to the project's repository top-level.
    case project
    /// Anchored to the shell's repository top-level (stable across in-repo `cd`).
    case shell
    /// Re-rooted because the foreground job is in another checkout/worktree.
    case foregroundWorktree
    /// Outside a repository — follow the live shell directory.
    case fallback
}

struct PanelRootResolution: Equatable, Sendable {
    let root: String
    let source: PanelRootSource
}

/// Pure panel-root resolution for Files/Git. Keeps the panel pinned to a
/// repository top-level during in-repo `cd`, and re-roots when the foreground
/// job enters another checkout/worktree.
enum PanelRootResolver {
    /// - Parameters:
    ///   - projectRoot: Project directory (stable pin / workspace root).
    ///   - shellDirectory: Shell's live cwd.
    ///   - foregroundDirectory: Foreground job cwd when it differs from the shell.
    ///   - gitTopLevel: Returns the git repository top-level for a path, or nil.
    nonisolated static func resolve(
        projectRoot: String,
        shellDirectory: String,
        foregroundDirectory: String?,
        gitTopLevel: (String) -> String?
    ) -> PanelRootResolution {
        let shell = normalize(shellDirectory)
        let project = normalize(projectRoot)
        let foreground = foregroundDirectory.map(normalize)

        let shellRepo = shell.isEmpty ? nil : gitTopLevel(shell).map(normalize)
        let projectRepo = project.isEmpty ? nil : gitTopLevel(project).map(normalize)
        let foregroundRepo = foreground.flatMap { dir in
            dir.isEmpty ? nil : gitTopLevel(dir).map(normalize)
        }

        // Foreground job moved into a different checkout/worktree.
        if let foregroundRepo,
           let base = shellRepo ?? projectRepo,
           foregroundRepo != base
        {
            return PanelRootResolution(root: foregroundRepo, source: .foregroundWorktree)
        }
        if let foregroundRepo, shellRepo == nil, projectRepo == nil {
            return PanelRootResolution(root: foregroundRepo, source: .foregroundWorktree)
        }

        // In-repo: stay at the repository top-level (do not follow nested cds).
        if let shellRepo {
            return PanelRootResolution(root: shellRepo, source: .shell)
        }
        if let projectRepo {
            return PanelRootResolution(root: projectRepo, source: .project)
        }

        // Outside any repository: follow the live shell directory.
        let fallback = shell.isEmpty ? project : shell
        return PanelRootResolution(root: fallback, source: .fallback)
    }

    private nonisolated static func normalize(_ path: String) -> String {
        URL(fileURLWithPath: path).standardizedFileURL.path
    }
}
