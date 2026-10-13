import Foundation

struct HistoryFile {
    enum Shell {
        case bash, zsh
    }

    let url: URL
    let shell: Shell

    func read() throws -> String {
        let data = try Data(contentsOf: url)
        var bytes = [UInt8](data)
        if shell == .zsh {
            // Zsh escapes non-ASCII bytes with Meta (0x83), followed by byte XOR 32.
            var decoded: [UInt8] = []
            var index = 0
            while index < bytes.count {
                if bytes[index] == 0x83 && index + 1 < bytes.count {
                    index += 1
                    decoded.append(bytes[index] ^ 32)
                } else {
                    decoded.append(bytes[index])
                }
                index += 1
            }
            bytes = decoded
        }
        let text = String(decoding: bytes, as: UTF8.self)
        var lines = text.components(separatedBy: "\n")
        if shell == .bash {
            lines.removeAll { $0.range(of: "^#[0-9]+$", options: .regularExpression) != nil }
        } else {
            var continued = false
            for index in lines.indices {
                if !continued {
                    lines[index] = lines[index].replacingOccurrences(
                        of: "^: [0-9]+:[0-9]+;", with: "", options: .regularExpression
                    )
                }
                continued = index < lines.count - 1 && lines[index].hasSuffix("\\")
                if continued {
                    lines[index].removeLast()
                }
            }
        }
        return lines.joined(separator: "\n")
    }
}

func historyFiles(home: URL, environment: [String: String]) throws -> [HistoryFile] {
    let manager = FileManager.default
    var files: [HistoryFile] = []
    for (name, shell) in [("bash", HistoryFile.Shell.bash), ("zsh", .zsh)] {
        let file = home.appendingPathComponent(".\(name)_history")
        if manager.fileExists(atPath: file.path) {
            files.append(HistoryFile(url: file, shell: shell))
        }
        let sessions = home.appendingPathComponent(".\(name)_sessions")
        if manager.fileExists(atPath: sessions.path) {
            let entries = try manager.contentsOfDirectory(
                at: sessions, includingPropertiesForKeys: nil
            ).sorted { $0.lastPathComponent < $1.lastPathComponent }
            for entry in entries where ["history", "historynew"].contains(entry.pathExtension) {
                files.append(HistoryFile(url: entry, shell: shell))
            }
        }
    }
    if let path = environment["HISTFILE"], !path.isEmpty {
        let shell: HistoryFile.Shell = environment["SHELL"]?.hasSuffix("/bash") == true ? .bash : .zsh
        files.append(HistoryFile(url: URL(fileURLWithPath: path), shell: shell))
    }
    var seen = Set<String>()
    return files.filter { seen.insert($0.url.resolvingSymlinksInPath().path).inserted }
}
