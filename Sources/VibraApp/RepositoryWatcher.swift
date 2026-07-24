import CoreServices
import Foundation

final class RepositoryWatcher: @unchecked Sendable {
    private let path: String
    private let onChange: @Sendable ([String]) -> Void
    private let queue = DispatchQueue(label: "app.vibra.repository-events", qos: .utility)
    private var stream: FSEventStreamRef?

    init(path: String, onChange: @escaping @Sendable ([String]) -> Void) {
        self.path = path
        self.onChange = onChange
    }

    func start() {
        guard stream == nil else { return }
        var context = FSEventStreamContext(
            version: 0,
            info: Unmanaged.passUnretained(self).toOpaque(),
            retain: nil,
            release: nil,
            copyDescription: nil
        )
        let callback: FSEventStreamCallback = { _, info, count, eventPaths, _, _ in
            guard let info else { return }
            let watcher = Unmanaged<RepositoryWatcher>
                .fromOpaque(info)
                .takeUnretainedValue()
            let rawPaths = unsafeBitCast(eventPaths, to: CFArray.self) as? [String] ?? []
            let paths = rawPaths.prefix(count).filter { watcher.isMeaningful($0) }
            guard !paths.isEmpty else { return }
            watcher.onChange(Array(paths))
        }
        let paths = [path] as CFArray
        stream = FSEventStreamCreate(
            nil,
            callback,
            &context,
            paths,
            FSEventStreamEventId(kFSEventStreamEventIdSinceNow),
            0.35,
            FSEventStreamCreateFlags(
                kFSEventStreamCreateFlagFileEvents
                    | kFSEventStreamCreateFlagWatchRoot
                    | kFSEventStreamCreateFlagUseCFTypes
            )
        )
        guard let stream else { return }
        FSEventStreamSetDispatchQueue(stream, queue)
        FSEventStreamStart(stream)
    }

    func stop() {
        guard let stream else { return }
        FSEventStreamStop(stream)
        FSEventStreamInvalidate(stream)
        FSEventStreamRelease(stream)
        self.stream = nil
    }

    deinit {
        stop()
    }

    private func isMeaningful(_ changedPath: String) -> Bool {
        let relative = changedPath == path
            ? ""
            : String(changedPath.dropFirst(min(changedPath.count, path.count + 1)))
        guard !relative.isEmpty else { return true }

        let components = relative.split(separator: "/").map(String.init)
        let ignored = Set([
            ".build", ".swiftpm", "DerivedData", "dist", "node_modules",
            ".DS_Store", "xcuserdata"
        ])
        if components.contains(where: ignored.contains) { return false }

        if components.first == ".git" {
            guard components.count > 1 else { return true }
            let allowed = Set(["HEAD", "index", "packed-refs", "refs"])
            return allowed.contains(components[1])
        }
        return true
    }
}
