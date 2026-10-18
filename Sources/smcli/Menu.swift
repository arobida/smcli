import Foundation

/// Interactive command picker shown when smcli runs in a terminal with no arguments.
struct Menu {
    struct Item {
        let icon: String
        let title: String
        let detail: String
        /// Asked after selection; nil runs the command directly.
        let prompt: String?
        let inputRequired: Bool
        let arguments: (String) -> [String]
    }

    static func items(shell: String) -> [Item] {
        [
            Item(icon: "⌕", title: "Search history", detail: "Keyword + semantic search across every command",
                 prompt: "Search for", inputRequired: true) { ["search"] + words($0) },
            Item(icon: "✦", title: "Suggest", detail: "Top commands for this directory, live as you type",
                 prompt: nil, inputRequired: false) { _ in ["pick"] },
            Item(icon: "☰", title: "Show history", detail: "Native shell files plus captured metadata",
                 prompt: nil, inputRequired: false) { _ in ["history"] },
            Item(icon: "◆", title: "Choose embedding model", detail: "Pick an Ollama model for search",
                 prompt: nil, inputRequired: false) { _ in ["init"] },
            Item(icon: "⚙", title: "Shell integration", detail: "Print capture hooks for \(shell)",
                 prompt: nil, inputRequired: false) { _ in ["shell-init", shell] },
            Item(icon: "⇪", title: "Import legacy history", detail: "Load records from a .jsonl file",
                 prompt: "Path to .jsonl", inputRequired: true) { ["import", ($0 as NSString).expandingTildeInPath] },
            Item(icon: "↻", title: "Rebuild search index", detail: "Re-embed every command with the current model",
                 prompt: nil, inputRequired: false) { _ in ["reindex"] },
            Item(icon: "♥", title: "Check index health", detail: "Index coverage, size, and embedding model status",
                 prompt: nil, inputRequired: false) { _ in ["health"] },
            Item(icon: "?", title: "Help", detail: "Show command-line usage",
                 prompt: nil, inputRequired: false) { _ in ["--help"] },
        ]
    }

    private static func words(_ text: String) -> [String] {
        text.split(whereSeparator: \.isWhitespace).map(String.init)
    }

    static var isInteractive: Bool { isatty(STDIN_FILENO) == 1 && isatty(STDOUT_FILENO) == 1 }

    /// Returns the arguments to run, or nil if the user quit.
    static func choose(environment: [String: String]) -> [String]? {
        let shell = (environment["SHELL"] as NSString?)?.lastPathComponent == "bash" ? "bash" : "zsh"
        let items = items(shell: shell)
        var selected = 0
        while true {
            guard let index = pick(items, selected: selected) else { return nil }
            selected = index
            let item = items[index]
            guard let prompt = item.prompt else { return item.arguments("") }
            print("\n  \(Style.accent)❯\(Style.reset) \(Style.bold)\(prompt):\(Style.reset) ", terminator: "")
            fflush(stdout)
            guard let line = readLine() else { return nil }
            let input = line.trimmingCharacters(in: .whitespaces)
            if input.isEmpty && item.inputRequired { continue }
            print()
            return item.arguments(input)
        }
    }

    // MARK: - Rendering

    private enum Key { case up, down, enter, quit, number(Int), other }

    private static func pick(_ items: [Item], selected start: Int) -> Int? {
        var selected = start
        var original = termios()
        tcgetattr(STDIN_FILENO, &original)
        var raw = original
        raw.c_lflag &= ~tcflag_t(ICANON | ECHO)
        tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw)
        write("\u{1B}[?1049h\u{1B}[?25l")
        defer {
            write("\u{1B}[?25h\u{1B}[?1049l")
            tcsetattr(STDIN_FILENO, TCSAFLUSH, &original)
        }
        while true {
            render(items, selected: selected)
            switch readKey() {
            case .up: selected = (selected - 1 + items.count) % items.count
            case .down: selected = (selected + 1) % items.count
            case .enter: return selected
            case .quit: return nil
            case .number(let n) where n >= 1 && n <= items.count: return n - 1
            default: break
            }
        }
    }

    private static func render(_ items: [Item], selected: Int) {
        var out = "\u{1B}[H\u{1B}[2J\n"
        out += "  \(Style.gradient("smcli"))  \(Style.dim)shell history, remembered\(Style.reset)\n\n"
        for (index, item) in items.enumerated() {
            if index == selected {
                out += "  \(Style.accent)▌\(Style.reset)\(Style.highlight) \(item.icon)  \(Style.bold)\(item.title)\(Style.reset)\n"
                out += "  \(Style.accent)▌\(Style.reset)    \(Style.soft)\(item.detail)\(Style.reset)\n"
            } else {
                out += "   \(Style.dim)\(item.icon)\(Style.reset)  \(item.title)\n"
                out += "      \(Style.dim)\(item.detail)\(Style.reset)\n"
            }
        }
        out += "\n  \(Style.dim)↑↓/jk move · enter select · 1-\(items.count) jump · q quit\(Style.reset)\n"
        write(out)
    }

    private static func readKey() -> Key {
        guard let byte = readByte() else { return .quit }
        switch byte {
        case 0x0A, 0x0D: return .enter
        case UInt8(ascii: "q"), 0x03, 0x04: return .quit
        case UInt8(ascii: "k"): return .up
        case UInt8(ascii: "j"): return .down
        case UInt8(ascii: "1")...UInt8(ascii: "9"): return .number(Int(byte - UInt8(ascii: "0")))
        case 0x1B:
            // A lone Escape quits; arrow keys arrive as ESC [ A / ESC [ B (or ESC O A in application mode).
            var fds = pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0)
            guard poll(&fds, 1, 40) > 0, let next = readByte(), next == 0x5B || next == 0x4F,
                  let code = readByte() else { return .quit }
            return code == UInt8(ascii: "A") ? .up : code == UInt8(ascii: "B") ? .down : .other
        default: return .other
        }
    }

    private static func readByte() -> UInt8? {
        var byte: UInt8 = 0
        return read(STDIN_FILENO, &byte, 1) == 1 ? byte : nil
    }

    private static func write(_ text: String) {
        FileHandle.standardOutput.write(Data(text.utf8))
    }
}

private enum Style {
    static let reset = "\u{1B}[0m"
    static let bold = "\u{1B}[1m"
    static let dim = "\u{1B}[2m"
    static let accent = "\u{1B}[38;5;141m"
    static let soft = "\u{1B}[38;5;183m"
    static let highlight = "\u{1B}[48;5;236m\u{1B}[38;5;231m"

    static func gradient(_ text: String) -> String {
        let colors = [99, 135, 171, 207, 213]
        return text.enumerated().map { "\u{1B}[1;38;5;\(colors[$0.offset % colors.count])m\($0.element)" }.joined() + reset
    }
}
