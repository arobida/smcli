import Foundation
import Testing
@testable import smcli

struct PickerTests {
    @Test func `Open on suggestions, then the rest by recency`() {
        let captured = ["make": CommandStats(runs: 1, lastRun: 100, lastDirectory: "/", lastExitCode: 0),
                        "git status": CommandStats(runs: 9, lastRun: 200, lastDirectory: "/", lastExitCode: 0)]
        let picker = Picker(suggestions: ["git push", "make"], captured: captured,
                            native: ["ls", "smcli search x", "", "vim a", "ls"])
        #expect(picker.candidates == ["git push", "make", "git status", "ls", "vim a"])
        #expect(picker.matches == picker.candidates)
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
}
