import Foundation

/// minishlab/potion-code-16M-v2 (a Model2Vec static embedding) run natively: BERT WordPiece tokens,
/// a mean of their rows in one float16 table, then L2 normalization. There is no network pass, so a
/// query embeds in microseconds and loading is a memory map plus the tokenizer's vocabulary.
/// Tokenization mirrors model2vec: no special tokens, unknown ids dropped, at most 512 tokens.
final class PotionModel: @unchecked Sendable {
    static let name = "potion-code-16M-v2"
    static let maxTokens = 512

    let dimension: Int
    let directory: URL
    private let vocabulary: [String: Int32]
    private let unknown: Int32?
    private let lowercase: Bool
    private let normalize: Bool
    private let table: Data
    private let rows: Int

    /// `directory` holds `model.safetensors` and `tokenizer.json`, as in a Hugging Face snapshot.
    init(directory: URL) throws {
        self.directory = directory
        let modelURL = directory.appendingPathComponent("model.safetensors")
        let tokenizerURL = directory.appendingPathComponent("tokenizer.json")
        guard FileManager.default.fileExists(atPath: modelURL.path),
              FileManager.default.fileExists(atPath: tokenizerURL.path) else {
            throw HistoryError(message: "No Potion model in \(directory.path) (needs model.safetensors and tokenizer.json); rerun `smcli init --potion`.")
        }

        let tokenizer = try JSONSerialization.jsonObject(with: Data(contentsOf: tokenizerURL)) as? [String: Any]
        guard let model = tokenizer?["model"] as? [String: Any], model["type"] as? String == "WordPiece",
              let vocab = model["vocab"] as? [String: Int] else {
            throw HistoryError(message: "Unsupported Potion tokenizer in \(tokenizerURL.path): expected WordPiece.")
        }
        vocabulary = vocab.mapValues(Int32.init)
        unknown = (model["unk_token"] as? String).flatMap { vocab[$0] }.map(Int32.init)
        lowercase = ((tokenizer?["normalizer"] as? [String: Any])?["lowercase"] as? Bool) ?? true

        let configURL = directory.appendingPathComponent("config.json")
        let config = (try? Data(contentsOf: configURL)).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        normalize = config?["normalize"] as? Bool ?? true

        // safetensors: an 8-byte little-endian header length, a JSON header, then raw tensor bytes.
        let file = try Data(contentsOf: modelURL, options: .alwaysMapped)
        guard file.count > 8 else { throw HistoryError(message: "Potion model file is truncated: \(modelURL.path)") }
        let headerLength = file.prefix(8).enumerated().reduce(0) { $0 | Int($1.element) << (8 * $1.offset) }
        guard headerLength > 0, 8 + headerLength <= file.count,
              let header = try JSONSerialization.jsonObject(with: file.subdata(in: 8..<8 + headerLength)) as? [String: Any],
              let tensor = header["embeddings"] as? [String: Any], tensor["dtype"] as? String == "F16",
              let shape = tensor["shape"] as? [Int], shape.count == 2,
              let offsets = tensor["data_offsets"] as? [Int], offsets.count == 2,
              offsets[1] - offsets[0] == shape[0] * shape[1] * 2, 8 + headerLength + offsets[1] <= file.count else {
            throw HistoryError(message: "Unsupported Potion model in \(modelURL.path): expected one float16 embeddings table.")
        }
        rows = shape[0]
        dimension = shape[1]
        let start = 8 + headerLength + offsets[0]
        table = file.subdata(in: start..<start + rows * dimension * 2)
        guard vocabulary.values.allSatisfy({ $0 >= 0 && Int($0) < rows }) else {
            throw HistoryError(message: "Potion tokenizer and model in \(directory.path) do not match.")
        }
    }

    /// Token ids model2vec would embed for `text`.
    func tokenize(_ text: String) -> [Int32] {
        var ids: [Int32] = []
        for word in Self.preTokenize(normalized(text)) {
            wordPiece(word, into: &ids)
            if ids.count >= Self.maxTokens { break }
        }
        return Array(ids.prefix(Self.maxTokens))
    }

    /// The pooled, normalized embedding; nil when no token is known (the caller picks a fallback).
    func embed(_ text: String) -> [Float]? {
        let ids = tokenize(text)
        guard !ids.isEmpty else { return nil }
        var sum = [Float](repeating: 0, count: dimension)
        table.withUnsafeBytes { raw in
            let halves = raw.bindMemory(to: UInt16.self)
            sum.withUnsafeMutableBufferPointer { out in
                for id in ids {
                    let row = Int(id) * dimension
                    for d in 0..<dimension { out[d] += halfToFloat[Int(UInt16(littleEndian: halves[row + d]))] }
                }
            }
        }
        let count = Float(ids.count)
        var norm: Float = 0
        for d in 0..<dimension {
            sum[d] /= count
            norm += sum[d] * sum[d]
        }
        if normalize {
            guard norm > 0 else { return nil }
            let scale = 1 / norm.squareRoot()
            for d in 0..<dimension { sum[d] *= scale }
        }
        return sum
    }

    /// BertNormalizer: drop control characters, whitespace to spaces, space out CJK ideographs,
    /// and (with lowercasing, as strip_accents defaults to it) remove accents.
    private func normalized(_ text: String) -> String {
        var scalars = String.UnicodeScalarView()
        let source = lowercase ? text.decomposedStringWithCanonicalMapping : text
        for scalar in source.unicodeScalars {
            let value = scalar.value
            if value == 0 || value == 0xFFFD { continue }
            if scalar == "\t" || scalar == "\n" || scalar == "\r" || scalar.properties.generalCategory == .spaceSeparator {
                scalars.append(" ")
                continue
            }
            switch scalar.properties.generalCategory {
            case .control, .format: continue
            case .nonspacingMark where lowercase: continue
            default: break
            }
            if Self.isCJK(value) {
                scalars.append(" ")
                scalars.append(scalar)
                scalars.append(" ")
            } else {
                scalars.append(scalar)
            }
        }
        let result = String(scalars)
        return lowercase ? result.lowercased() : result
    }

    /// BertPreTokenizer: split on whitespace, and make every punctuation character its own word.
    private static func preTokenize(_ text: String) -> [Substring.UnicodeScalarView] {
        var words: [Substring.UnicodeScalarView] = []
        let scalars = Substring(text).unicodeScalars
        var start: Substring.UnicodeScalarView.Index? = nil
        var index = scalars.startIndex
        while index < scalars.endIndex {
            let scalar = scalars[index]
            let next = scalars.index(after: index)
            if scalar.properties.isWhitespace || isPunctuation(scalar) {
                if let begin = start { words.append(scalars[begin..<index]) }
                start = nil
                if !scalar.properties.isWhitespace { words.append(scalars[index..<next]) }
            } else if start == nil {
                start = index
            }
            index = next
        }
        if let begin = start { words.append(scalars[begin..<scalars.endIndex]) }
        return words
    }

    /// Greedy longest-match WordPiece; a word with any unmatched piece becomes [UNK], which is dropped.
    private func wordPiece(_ word: Substring.UnicodeScalarView, into ids: inout [Int32]) {
        let scalars = Array(word)
        guard scalars.count <= 100 else { return }
        var pieces: [Int32] = []
        var start = 0
        while start < scalars.count {
            var end = scalars.count
            var found: Int32?
            while start < end {
                var piece = String(String.UnicodeScalarView(scalars[start..<end]))
                if start > 0 { piece = "##" + piece }
                if let id = vocabulary[piece] { found = id; break }
                end -= 1
            }
            guard let id = found else { return }
            pieces.append(id)
            start = end
        }
        ids += pieces.filter { $0 != unknown }
    }

    private static func isPunctuation(_ scalar: Unicode.Scalar) -> Bool {
        let value = scalar.value
        if (33...47).contains(value) || (58...64).contains(value) || (91...96).contains(value) || (123...126).contains(value) {
            return true
        }
        switch scalar.properties.generalCategory {
        case .connectorPunctuation, .dashPunctuation, .openPunctuation, .closePunctuation,
             .initialPunctuation, .finalPunctuation, .otherPunctuation: return true
        default: return false
        }
    }

    private static func isCJK(_ value: UInt32) -> Bool {
        (0x4E00...0x9FFF).contains(value) || (0x3400...0x4DBF).contains(value) || (0x20000...0x2A6DF).contains(value)
            || (0x2A700...0x2B73F).contains(value) || (0x2B740...0x2B81F).contains(value) || (0x2B820...0x2CEAF).contains(value)
            || (0xF900...0xFAFF).contains(value) || (0x2F800...0x2FA1F).contains(value)
    }

    /// The newest Potion snapshot in the Hugging Face cache, where `model2vec`/`huggingface_hub` put it.
    static func cachedSnapshot(home: URL, environment: [String: String]) -> URL? {
        let hub = environment["HF_HUB_CACHE"].map { URL(fileURLWithPath: $0) }
            ?? environment["HF_HOME"].map { URL(fileURLWithPath: $0).appendingPathComponent("hub") }
            ?? home.appendingPathComponent(".cache/huggingface/hub")
        let snapshots = hub.appendingPathComponent("models--minishlab--\(name)/snapshots")
        let names = (try? FileManager.default.contentsOfDirectory(at: snapshots, includingPropertiesForKeys: [.contentModificationDateKey])) ?? []
        return names
            .filter { FileManager.default.fileExists(atPath: $0.appendingPathComponent("model.safetensors").path) }
            .max { a, b in
                let date = { (url: URL) in (try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast }
                return date(a) < date(b)
            }
    }
}

/// IEEE half → float for every bit pattern; a lookup is faster than converting per element and
/// works on every architecture (x86 macOS has no native Float16).
private let halfToFloat: [Float] = (0...UInt16.max).map { bits in
    let sign: UInt32 = UInt32(bits & 0x8000) << 16
    let exponent = UInt32(bits >> 10) & 0x1F
    var mantissa = UInt32(bits & 0x3FF)
    let result: UInt32
    if exponent == 0 {
        if mantissa == 0 {
            result = sign
        } else {
            // Subnormal: shift until the implicit bit appears.
            var e: UInt32 = 127 - 15 + 1
            while mantissa & 0x400 == 0 { mantissa <<= 1; e -= 1 }
            result = sign | (e << 23) | ((mantissa & 0x3FF) << 13)
        }
    } else if exponent == 0x1F {
        result = sign | 0x7F80_0000 | (mantissa << 13)
    } else {
        result = sign | ((exponent + 127 - 15) << 23) | (mantissa << 13)
    }
    return Float(bitPattern: result)
}

#if canImport(ProximaKit)
import ProximaKit

struct PotionEmbedder: TextEmbedder {
    let model: PotionModel
    var dimension: Int { model.dimension }

    /// Text with no known tokens gets a fixed vector, leaving its ranking to the BM25 leg.
    func embed(_ text: String) async throws -> Vector {
        if let vector = model.embed(text) { return Vector(vector) }
        return Vector(dimension: dimension, repeating: 1 / Float(dimension).squareRoot())
    }

    /// Embedding is cheap arithmetic, so a plain loop beats spawning a task per text.
    func embedBatch(_ texts: [String]) async throws -> [Vector] {
        var vectors: [Vector] = []
        vectors.reserveCapacity(texts.count)
        for text in texts { vectors.append(try await embed(text)) }
        return vectors
    }
}
#endif
