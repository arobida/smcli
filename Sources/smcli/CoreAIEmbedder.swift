#if canImport(CoreAI) && canImport(ProximaKit)
import CoreAI
import Foundation
import ProximaKit
import Tokenizers

/// F2LLM-v2-80M, bundled as a Core AI graph: Qwen3 encoder, last-token pooling, and L2 normalization
/// all run in-graph, so one call returns a unit vector. Exported with the smcli-models
/// export_f2llm.py script (a coreai-model-zoo recipe): fp16, right-padded grid of 512 tokens.
@available(macOS 27, *)
struct CoreAIEmbedder: TextEmbedder {
    /// What a search query is asking for. F2LLM is instruction-tuned: queries carry a task
    /// instruction and documents (the indexed commands) carry none.
    enum Task: String, CaseIterable, Sendable {
        /// "undo last commit" → `git reset --soft HEAD~1`
        case describe
        /// "git push -f" → `git push --force-with-lease origin main`
        case similar

        var instruction: String {
            switch self {
            case .describe: "Given a description of a task, retrieve the shell command that performs it"
            case .similar: "Given a shell command, retrieve similar or related shell commands"
            }
        }

        /// Queries with shell syntax (flags, paths, pipes, assignments) are commands; anything else is prose.
        static func infer(from query: String) -> Task {
            let words = query.split(separator: " ")
            let shell = words.contains { $0.hasPrefix("-") || $0.contains("/") || $0.contains("=") || $0.hasPrefix("~") || $0.hasPrefix("$") }
                || query.contains { "|&;<>`".contains($0) }
            return shell ? .similar : .describe
        }

        func prompt(_ query: String) -> String { "Instruct: \(instruction)\nQuery: \(query)" }
    }

    static let modelName = "f2llm-v2-80m"
    let dimension = 320
    /// nil infers the task from each query.
    var task: Task? = nil
    private let runner = Runner()

    /// ProximaKit embeds queries one at a time and documents in batches.
    func embed(_ text: String) async throws -> Vector {
        let task = task ?? Task.infer(from: text)
        return try await runner.embed(task.prompt(text))
    }

    func embedBatch(_ texts: [String]) async throws -> [Vector] {
        var vectors: [Vector] = []
        vectors.reserveCapacity(texts.count)
        for text in texts { vectors.append(try await runner.embed(text)) }
        return vectors
    }

    /// Loads the graph and tokenizer on first use, so building a search engine stays cheap.
    private actor Runner {
        private var loaded: (function: InferenceFunction, tokenizer: any Tokenizer)?
        private let sequenceLength = 512
        private let endOfText: Int32 = 151645  // <|im_end|>, appended by the tokenizer; pooling reads it.

        func embed(_ text: String) async throws -> Vector {
            let (function, tokenizer) = try await load()
            var ids = tokenizer.encode(text: text).map(Int32.init)
            if ids.count > sequenceLength { ids = ids.prefix(sequenceLength - 1) + [endOfText] }
            var mask = [Int32](repeating: 1, count: ids.count)
            // Right padding is safe under causal attention: real tokens never attend to it.
            mask += [Int32](repeating: 0, count: sequenceLength - ids.count)
            ids += [Int32](repeating: 0, count: sequenceLength - ids.count)

            var outputs = try await function.run(inputs: [
                "input_ids": array(ids), "attention_mask": array(mask),
            ])
            guard let embedding = outputs.remove("embedding")?.ndArray else {
                throw HistoryError(message: "Core AI model returned no embedding.")
            }
            let values: [Float] = embedding.scalarType == .float16
                ? embedding.view(as: Float16.self).withUnsafePointer { pointer, _, _ in
                    UnsafeBufferPointer(start: pointer, count: 320).map(Float.init) }
                : embedding.view(as: Float.self).withUnsafePointer { pointer, _, _ in
                    Array(UnsafeBufferPointer(start: pointer, count: 320)) }
            return Vector(values)
        }

        private func array(_ values: [Int32]) -> NDArray {
            var array = NDArray(shape: [1, values.count], scalarType: .int32)
            var view = array.mutableView(as: Int32.self)
            view.copyElements(fromContentsOf: values)
            return array
        }

        private func load() async throws -> (InferenceFunction, any Tokenizer) {
            if let loaded { return loaded }
            guard let bundle = Bundle.module.url(forResource: "F2LLM-v2-80M", withExtension: nil),
                  let model = try FileManager.default.contentsOfDirectory(at: bundle, includingPropertiesForKeys: nil)
                    .first(where: { $0.pathExtension == "aimodel" })
            else { throw HistoryError(message: "Bundled Core AI model F2LLM-v2-80M is missing.") }
            // A GPU preference, as the zoo recommends: mixed placement can return wrong numerics.
            let aiModel = try await AIModel(contentsOf: model, options: SpecializationOptions(preferredComputeUnitKind: .gpu))
            guard let function = try aiModel.loadFunction(named: "main") else {
                throw HistoryError(message: "Core AI model has no main function.")
            }
            let tokenizer = try await AutoTokenizer.from(modelFolder: bundle.appendingPathComponent("tokenizer"))
            loaded = (function, tokenizer)
            return (function, tokenizer)
        }
    }
}
#endif
