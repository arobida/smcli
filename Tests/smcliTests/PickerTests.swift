import Foundation
import Testing
@testable import smcli
@testable import SmcliSearch
@testable import SmcliCore

struct PickerTests {
    @Test func `Open on suggestions, then the rest by recency`() {
        let captured = ["make": CommandStats(runs: 1, lastRun: 100, lastDirectory: "/", lastExitCode: 0),
                        "git status": CommandStats(runs: 9, lastRun: 200, lastDirectory: "/", lastExitCode: 0)]
        let picker = Picker(suggestions: ["git push", "make"], captured: captured,
                            native: ["ls", "smcli search x", "", "vim a", "ls"])
        #expect(picker.candidates == ["git push", "make", "git status", "ls", "vim a"])
        #expect(picker.matches == picker.candidates)
        #expect(picker.rows(picker.matches) == [.header("Suggested in \(FileManager.default.currentDirectoryPath)"), .command(0), .command(1),
                                                 .header("History"), .command(2), .command(3), .command(4)])
    }

    @Test func `Headers only for sections present`() {
        let picker = Picker(suggestions: ["git push"], captured: [:], native: ["ls"], query: "ls")
        #expect(picker.rows(picker.matches) == [.header("History"), .command(0)])
    }

    @Test func `Typing filters on every word, ignoring case`() {
        var picker = Picker(suggestions: ["git push origin main", "git status"], captured: [:],
                            native: ["Git Pull origin", "make"], query: "GIT orig")
        #expect(picker.matches == ["git push origin main", "Git Pull origin"])
        picker.semantic = ["make"]
        picker.semanticQuery = "GIT orig"
        #expect(picker.matches == ["make"])
        picker.query += "i"
        #expect(picker.matches == ["git push origin main", "Git Pull origin"])
    }

    @Test func `Control characters from history or a repository are shown, never sent to the terminal`() {
        #expect(Picker.visible("a\u{1B}[2Jb") == "a^[[2Jb")
        #expect(Picker.visible("x\u{9B}31m\u{7F}\u{07}") == "x<U+009B>31m^?^G")
        #expect(Picker.visible("one\ntwo\tthree é") == "one↵ two  three é")
        var picker = Picker(suggestions: [], captured: [:], native: [], query: "")
        (picker.mode, picker.globalFiles) = (.files, true)
        picker.hits = [FileHit(path: "evil\u{1B}]0;title\u{07}.txt", insert: "x", provenance: ["files"], git: "\u{1B}[",
                               line: 3, text: "\u{1B}[H\u{1B}[2Jforged")]
        let screen = picker.render([], size: (rows: 10, columns: 120))
        #expect(!screen.contains("\u{1B}[H\u{1B}[2Jforged") && !screen.contains("\u{1B}]0;") && !screen.contains("\u{07}"))
        #expect(screen.contains("evil^[]0;title^G.txt") && screen.contains("^[[H^[[2Jforged"))
    }
}
