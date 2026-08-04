import Foundation

/// Type-aware SF Symbol names for file tree and change rows.
enum FileTypeIcon {
    nonisolated static func systemImage(forFileName name: String, isDirectory: Bool = false) -> String {
        if isDirectory { return "folder" }
        let lower = name.lowercased()
        let ext = (lower as NSString).pathExtension
        switch ext {
        case "swift": return "swift"
        case "ts", "tsx", "js", "jsx", "mjs", "cjs": return "curlybraces"
        case "json": return "curlybraces.square"
        case "md", "markdown", "txt", "rst": return "doc.plaintext"
        case "yml", "yaml", "toml", "ini", "cfg", "conf": return "list.bullet.rectangle"
        case "html", "htm", "css", "scss", "sass": return "globe"
        case "png", "jpg", "jpeg", "gif", "webp", "svg", "ico": return "photo"
        case "pdf": return "doc.richtext"
        case "sh", "bash", "zsh", "fish": return "terminal"
        case "rs": return "gearshape"
        case "go": return "hammer"
        case "py": return "chevron.left.forwardslash.chevron.right"
        case "rb": return "diamond"
        case "java", "kt", "kts": return "cup.and.saucer"
        case "c", "h", "cpp", "hpp", "cc", "m", "mm": return "chevron.left.forwardslash.chevron.right"
        case "zip", "tar", "gz", "tgz", "bz2", "7z": return "archivebox"
        case "lock": return "lock"
        default:
            if lower == "dockerfile" || lower.hasPrefix("makefile") {
                return "shippingbox"
            }
            if lower.hasPrefix("readme") { return "doc.plaintext" }
            if lower.hasPrefix("license") { return "doc.badge.gearshape" }
            return "doc"
        }
    }
}
