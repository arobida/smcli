import Foundation
import SmcliCore

/// Directory context from the service's cache, or discovered in-process when it is unreachable. Git status is
/// the slow part: the service refreshes it in the background and this waits briefly for a first one, while the
/// fallback runs Git once with a short timeout and goes without on timeout.
func directoryContext(_ directory: String, service: ServiceClient, wait: Double = 0.5) -> ProjectContext {
    var request = ServiceRequest(op: .context)
    request.directory = directory
    let deadline = Date().addingTimeInterval(wait)
    while let response = service.send(request, timeout: 2), response.ok, let context = response.context {
        if context.status != nil || !context.statusPending || Date() >= deadline { return context }
        usleep(25_000)
    }
    var context = ProjectContext.discover(directory)
    if let root = context.repository?.root { context.status = GitStatus.read(root: root, timeout: wait) }
    return context
}

func context(_ arguments: [String], service: ServiceClient, environment: [String: String]) throws {
    var json = false
    var directory = environment["PWD"].flatMap { $0.hasPrefix("/") ? $0 : nil } ?? FileManager.default.currentDirectoryPath
    var index = 0
    while index < arguments.count {
        switch arguments[index] {
        case "--json": json = true
        case "--dir":
            index += 1
            guard index < arguments.count, arguments[index].hasPrefix("/") else {
                throw HistoryError(message: "--dir requires an absolute path.")
            }
            directory = arguments[index]
        default: throw HistoryError(message: "Usage: smcli context [--json] [--dir PATH]")
        }
        index += 1
    }
    let context = directoryContext(directory, service: service)
    if json {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        print(String(decoding: try encoder.encode(context), as: UTF8.self))
        return
    }
    print("directory  \(context.directory)")
    guard let repository = context.repository else {
        print("repo       none")
        printProjects(context)
        return
    }
    let branch = repository.branch ?? "detached at \(repository.head ?? "?")"
    print("repo       \(repository.root)" + (repository.linkedWorktree ? " (linked worktree of \(repository.commonDir))" : ""))
    print("branch     \(branch)" + (repository.operation.map { "  [\($0) in progress]" } ?? ""))
    if let status = context.status {
        var parts = [status.staged > 0 ? "\(status.staged) staged" : nil, status.unstaged > 0 ? "\(status.unstaged) unstaged" : nil,
                     status.untracked > 0 ? "\(status.untracked) untracked" : nil,
                     status.conflicted > 0 ? "\(status.conflicted) conflicted" : nil].compactMap { $0 }
        if parts.isEmpty { parts = ["clean"] }
        if status.ahead > 0 || status.behind > 0 { parts.append("ahead \(status.ahead), behind \(status.behind)") }
        print("status     " + parts.joined(separator: ", ") + (context.statusPending ? " (refreshing)" : ""))
    } else {
        print("status     " + (context.statusPending ? "refreshing" : "unavailable"))
    }
    print("in repo    \(context.repoRelative ?? ".")")
    printProjects(context)
}

private func printProjects(_ context: ProjectContext) {
    for project in context.projects {
        let manager = project.packageManager.map { ", \($0)" } ?? ""
        print("project    \(project.root) (\(project.kind)\(manager))" + (project.name.map { " \"\($0)\"" } ?? ""))
        for (file, names) in project.scripts.sorted(by: { $0.key < $1.key }) {
            print("           \(file): " + names.prefix(12).joined(separator: " ") + (names.count > 12 ? " ..." : ""))
        }
    }
    if let relative = context.projectRelative { print("in project \(relative)") }
}
