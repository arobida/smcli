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

    /// Whether a quoted word may go after `before` (the command line left of the cursor): only where the shell is
    /// reading plain words: the top level, or the innermost `$(…)`/`(…)` (even one inside double quotes). Inside an open quote, backtick, `${…}` or after
    /// a dangling backslash, the word's own quotes would be reinterpreted (in `"…"`, `'$(x)'` runs `x`), so not there.
    static func insertable(after before: String, shell: String) -> Bool {
        enum Frame { case command, single, double, dollarSingle, backtick, brace }
        let fish = shell == "fish"
        // shortcut: reject even quoted/finished here-doc syntax; parse delimiters only if insertion there is needed.
        guard fish || !before.replacingOccurrences(of: "\\\n", with: "").contains("<<") else { return false }
        var stack: [Frame] = [.command]
        let chars = Array(before.unicodeScalars)
        var i = 0
        while i < chars.count {
            let c = chars[i], next: Unicode.Scalar? = i + 1 < chars.count ? chars[i + 1] : nil
            switch stack.last! {
            case .command, .brace:
                if c == "\\" {
                    guard next != nil else { return false }
                    i += 1
                } else if c == "'" {
                    stack.append(.single)
                } else if c == "\"" {
                    stack.append(.double)
                } else if c == "`" && !fish {
                    stack.append(.backtick)
                } else if c == "$" && next == "'" && !fish {
                    stack.append(.dollarSingle); i += 1
                } else if c == "$" && next == "(" {
                    stack.append(.command); i += 1
                } else if c == "$" && next == "{" && !fish {
                    stack.append(.brace); i += 1
                } else if c == "(" {
                    stack.append(.command)
                } else if c == ")" && stack.count > 1 && stack.last == .command {
                    stack.removeLast()
                } else if c == "}" && stack.last == .brace {
                    stack.removeLast()
                }
            case .single:
                // Fish allows `\'` and `\\` inside single quotes; POSIX shells allow no escapes there.
                if fish && c == "\\" && next != nil { i += 1 } else if c == "'" { stack.removeLast() }
            case .dollarSingle:
                if c == "\\" && next != nil { i += 1 } else if c == "'" { stack.removeLast() }
            case .double:
                if c == "\\" && next != nil { i += 1 }
                else if c == "\"" { stack.removeLast() }
                else if c == "$" && next == "(" { stack.append(.command); i += 1 }
                else if c == "`" && !fish { stack.append(.backtick) }
                else if c == "$" && next == "{" && !fish { stack.append(.brace); i += 1 }
            case .backtick:
                if c == "\\" && next != nil { i += 1 } else if c == "`" { stack.removeLast() }
            }
            i += 1
        }
        // `$(…)` starts a fresh parse even inside quotes; backticks reprocess backslashes first, so never under one.
        return stack.last == .command && !stack.contains(.backtick)
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
