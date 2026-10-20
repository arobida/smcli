import Foundation
import SmcliCore

/// Captured stats plus native history. Native files are best-effort: an unreadable one should not block the rest.
package func searchCorpus(captured: CapturedHistory, home: URL, environment: [String: String],
                          project: String? = nil) throws -> [String: SearchEntry] {
    let native = try historyFiles(home: home, environment: environment)
        .flatMap { (try? $0.read())?.components(separatedBy: "\n") ?? [] }
    return searchCorpus(captured: try captured.commandStats(directory: workingDirectory(environment), project: project),
                        native: native)
}

/// Signals for an in-process search, read from the database: the session's previous command and what followed it
/// before, and earlier picks. (The service keeps these in memory.)
package func searchSignals(captured: CapturedHistory, context: RankingContext?, user: String = NSUserName()) throws -> SearchSignals {
    guard FileManager.default.fileExists(atPath: captured.url.path) else { return SearchSignals(context: context) }
    let db = try captured.openConnection()
    var signals = SearchSignals(context: context, picks: try captured.picks(on: db))
    if let session = context?.session, context?.uses(.transition) ?? true,
       let previous = try captured.previousCommand(session: session, user: user, on: db) {
        (signals.previous, signals.followers) = (previous, try captured.followers(after: previous, user: user, on: db))
    }
    return signals
}

/// The shell's $PWD keeps symlinked paths as typed, matching what the capture hooks recorded.
package func workingDirectory(_ environment: [String: String]) -> String {
    environment["PWD"].flatMap { $0.hasPrefix("/") ? $0 : nil } ?? FileManager.default.currentDirectoryPath
}

#if canImport(ProximaKit)
/// The search index next to the history database, using the model chosen by `smcli init`. With no choice
/// made, the potion-code-16M-v2 built into the binary (or, in a build without it, from the Hugging Face
/// cache); Apple's model when neither loads, so search keeps working.
package func searchEngine(captured: CapturedHistory, home: URL, environment: [String: String]) throws -> HistorySearch {
    var engine = HistorySearch(directory: captured.url.appendingPathExtension("search"))
    let config = try SmcliConfig.load(from: SmcliConfig.location(home: home, environment: environment))
    if let path = config.potionPath {
        engine.potion = PotionEmbedder(model: try PotionModel(directory: URL(fileURLWithPath: path)))
    } else if let model = config.ollamaModel, let dimension = config.ollamaDimension {
        engine.ollama = OllamaEmbedder(client: Ollama(environment: environment), model: model, dimension: dimension)
    } else if config.builtin != true {
        let model = (try? PotionModel.bundled()) ?? PotionModel.cachedSnapshot(home: home, environment: environment)
            .flatMap { try? PotionModel(directory: $0) }
        engine.potion = model.map(PotionEmbedder.init)
    }
    return engine
}
#endif
