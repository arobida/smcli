import Foundation

/// Turns a file path into one shell word for Bash, Zsh or Fish, so inserting it into the command line can't
/// split, expand or execute anything. Leading `-` gets `./` so the word can't be read as an option.
enum ShellQuoting {
    private static let plain = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_./+,:@-")

    static func quote(_ path: String, shell: String) -> String {
        let path = path.hasPrefix("-") ? "./" + path : path
        guard !path.isEmpty else { return "''" }
        if path.allSatisfy(plain.contains) { return path }
        return shell == "fish" ? fish(path) : posix(path)
    }

    private static func isControl(_ byte: UInt8) -> Bool { byte < 0x20 || byte == 0x7F }

    /// Bash and Zsh: single quotes, or `$'…'` when the path holds control characters (newline, tab...).
    private static func posix(_ path: String) -> String {
        guard path.utf8.contains(where: isControl) else {
            return "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'"
        }
        var out = "$'"
        for scalar in path.unicodeScalars {
            switch scalar {
            case "\\": out += "\\\\"
            case "'": out += "\\'"
            case "\n": out += "\\n"
            case "\t": out += "\\t"
            case "\r": out += "\\r"
            case _ where scalar.value < 0x20 || scalar.value == 0x7F: out += String(format: "\\x%02x", scalar.value)
            default: out.unicodeScalars.append(scalar)
            }
        }
        return out + "'"
    }

    /// Fish: single quotes know only `\\` and `\'`, so control characters sit between quoted runs as unquoted escapes.
    private static func fish(_ path: String) -> String {
        var out = "", run = ""
        func flush() {
            if !run.isEmpty { out += "'" + run.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "'", with: "\\'") + "'" }
            run = ""
        }
        for scalar in path.unicodeScalars {
            if scalar.value < 0x20 || scalar.value == 0x7F {
                flush()
                out += scalar == "\n" ? "\\n" : scalar == "\t" ? "\\t" : scalar == "\r" ? "\\r" : String(format: "\\x%02x", scalar.value)
            } else {
                run.unicodeScalars.append(scalar)
            }
        }
        flush()
        return out
    }
}
