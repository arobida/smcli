import Foundation
import SmcliCore
#if canImport(MachO)
import MachO
#endif

/// minishlab/potion-code-16M-v2 (a Model2Vec static embedding) run natively: BERT WordPiece tokens,
/// a mean of their rows in one float16 table, then L2 normalization. There is no network pass, so a
/// query embeds in microseconds and loading is a memory map plus the tokenizer's vocabulary.
/// Tokenization mirrors model2vec: no special tokens, unknown ids dropped, at most 512 tokens.
package final class PotionModel: @unchecked Sendable {
    package static let name = "potion-code-16M-v2"
    package static let maxTokens = 512

    /// The Hugging Face revision of the copy in Models/, linked into the binary.
    package static let bundledRevision = "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b"

    package let dimension: Int
    /// Where the model came from, for messages: a directory path, or "built in".
    package let source: String
    /// Names the index built from this model, so another model or revision never reuses its vectors.
    package let identity: String
    private let vocabulary: [String: Int32]
    private let unknown: Int32?
    /// BertNormalizer options; all off when the tokenizer has no normalizer.
    private let lowercase: Bool
    private let stripAccents: Bool
    private let cleanText: Bool
    private let spaceChinese: Bool
    /// WordPiece options.
    private let subwordPrefix: String
    private let maxWordLength: Int
    private let normalize: Bool
    private let table: Data
    private let rows: Int

    /// `directory` holds `model.safetensors` and `tokenizer.json`, as in a Hugging Face snapshot.
    package convenience init(directory: URL) throws {
        let modelURL = directory.appendingPathComponent("model.safetensors")
        let tokenizerURL = directory.appendingPathComponent("tokenizer.json")
        guard FileManager.default.fileExists(atPath: modelURL.path),
              FileManager.default.fileExists(atPath: tokenizerURL.path) else {
            throw HistoryError(message: "No Potion model in \(directory.path) (needs model.safetensors and tokenizer.json); rerun `smcli init --potion`.")
        }
        // A stable hash (FNV-1a) of the resolved directory, which for a Hugging Face snapshot names its revision.
        let path = directory.resolvingSymlinksInPath().standardizedFileURL.path
        let hash = path.utf8.reduce(UInt64(0xcbf29ce484222325)) { ($0 ^ UInt64($1)) &* 0x100000001b3 }
        try self.init(model: try Data(contentsOf: modelURL, options: .alwaysMapped), tokenizer: try Data(contentsOf: tokenizerURL),
                      config: try? Data(contentsOf: directory.appendingPathComponent("config.json")),
                      source: directory.path, identity: String(hash, radix: 16))
    }

    /// The copy linked into the executable as the __POTION segment (see Package.swift); nil in builds without it.
    /// The segment is part of the mapped image, so this reads it in place like a mapped file.
    package static func bundled() throws -> PotionModel? {
        #if canImport(MachO)
        func section(_ name: String) -> Data? {
            var size: UInt = 0
            guard let header = _dyld_get_image_header(0).map({ UnsafeRawPointer($0).assumingMemoryBound(to: mach_header_64.self) }),
                  let bytes = getsectiondata(header, "__POTION", name, &size), size > 0 else { return nil }
            return Data(bytesNoCopy: bytes, count: Int(size), deallocator: .none)
        }
        guard let model = section("__model"), let tokenizer = section("__tokenizer") else { return nil }
        return try PotionModel(model: model, tokenizer: tokenizer, config: section("__config"), source: "built in",
                               identity: "bundled-" + bundledRevision.prefix(12))
        #else
        return nil
        #endif
    }

    package init(model file: Data, tokenizer tokenizerData: Data, config configData: Data?, source: String, identity: String) throws {
        self.source = source
        self.identity = identity
        let tokenizer = try JSONSerialization.jsonObject(with: tokenizerData) as? [String: Any]
        // Only the pieces this port implements are accepted: anything else would tokenize differently
        // from model2vec and quietly degrade every embedding.
        let normalizer = tokenizer?["normalizer"]
        guard let model = tokenizer?["model"] as? [String: Any], model["type"] as? String == "WordPiece",
              let vocab = model["vocab"] as? [String: Int],
              (tokenizer?["pre_tokenizer"] as? [String: Any])?["type"] as? String == "BertPreTokenizer",
              normalizer == nil || normalizer is NSNull || (normalizer as? [String: Any])?["type"] as? String == "BertNormalizer" else {
            throw HistoryError(message: "Unsupported Potion tokenizer in \(source): expected BertNormalizer, BertPreTokenizer, and WordPiece.")
        }
        vocabulary = vocab.mapValues(Int32.init)
        unknown = (model["unk_token"] as? String).flatMap { vocab[$0] }.map(Int32.init)
        subwordPrefix = model["continuing_subword_prefix"] as? String ?? "##"
        maxWordLength = model["max_input_chars_per_word"] as? Int ?? 100
        if let options = normalizer as? [String: Any] {
            lowercase = options["lowercase"] as? Bool ?? true
            // A null strip_accents follows lowercase, as in the Hugging Face tokenizers library.
            stripAccents = options["strip_accents"] as? Bool ?? lowercase
            cleanText = options["clean_text"] as? Bool ?? true
            spaceChinese = options["handle_chinese_chars"] as? Bool ?? true
        } else {
            (lowercase, stripAccents, cleanText, spaceChinese) = (false, false, false, false)
        }

        let config = configData.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        normalize = config?["normalize"] as? Bool ?? true

        // safetensors: an 8-byte little-endian header length, a JSON header, then raw tensor bytes.
        guard file.count > 8 else { throw HistoryError(message: "Potion model file is truncated: \(source)") }
        let headerLength = file.prefix(8).enumerated().reduce(0) { $0 | Int($1.element) << (8 * $1.offset) }
        guard headerLength > 0, 8 + headerLength <= file.count,
              let header = try JSONSerialization.jsonObject(with: file.subdata(in: 8..<8 + headerLength)) as? [String: Any],
              let tensor = header["embeddings"] as? [String: Any], tensor["dtype"] as? String == "F16",
              let shape = tensor["shape"] as? [Int], shape.count == 2,
              let offsets = tensor["data_offsets"] as? [Int], offsets.count == 2,
              offsets[1] - offsets[0] == shape[0] * shape[1] * 2, 8 + headerLength + offsets[1] <= file.count else {
            throw HistoryError(message: "Unsupported Potion model in \(source): expected one float16 embeddings table.")
        }
        rows = shape[0]
        dimension = shape[1]
        let start = file.startIndex + 8 + headerLength + offsets[0]
        // A slice shares the mapped file, so only the rows a query touches are paged in. Its start can be odd,
        // so `embed` reads unaligned.
        table = file[start..<start + rows * dimension * 2]
        guard vocabulary.values.allSatisfy({ $0 >= 0 && Int($0) < rows }) else {
            throw HistoryError(message: "Potion tokenizer and model in \(source) do not match.")
        }
    }

    /// Token ids model2vec would embed for `text`.
    package func tokenize(_ text: String) -> [Int32] {
        var ids: [Int32] = []
        for word in Self.preTokenize(normalized(text)) {
            wordPiece(word, into: &ids)
            if ids.count >= Self.maxTokens { break }
        }
        return Array(ids.prefix(Self.maxTokens))
    }

    /// The pooled, normalized embedding; nil when no token is known (the caller picks a fallback).
    package func embed(_ text: String) -> [Float]? {
        let ids = tokenize(text)
        guard !ids.isEmpty else { return nil }
        var sum = [Float](repeating: 0, count: dimension)
        table.withUnsafeBytes { raw in
            sum.withUnsafeMutableBufferPointer { out in
                for id in ids {
                    let row = Int(id) * dimension * 2
                    for d in 0..<dimension {
                        let half = raw.loadUnaligned(fromByteOffset: row + d * 2, as: UInt16.self)
                        out[d] += halfToFloat[Int(UInt16(littleEndian: half))]
                    }
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

    /// BertNormalizer, each step as configured: clean_text drops control characters and turns whitespace
    /// into spaces, handle_chinese_chars spaces out CJK ideographs, strip_accents removes accents.
    private func normalized(_ text: String) -> String {
        var scalars = String.UnicodeScalarView()
        let source = stripAccents ? text.decomposedStringWithCanonicalMapping : text
        for scalar in source.unicodeScalars {
            let value = scalar.value
            if cleanText {
                if value == 0 || value == 0xFFFD { continue }
                if scalar == "\t" || scalar == "\n" || scalar == "\r" || scalar.properties.generalCategory == .spaceSeparator {
                    scalars.append(" ")
                    continue
                }
                switch scalar.properties.generalCategory {
                case .control, .format: continue
                default: break
                }
            }
            if stripAccents && scalar.properties.generalCategory == .nonspacingMark { continue }
            if spaceChinese && Self.isCJK(value) {
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
        guard scalars.count <= maxWordLength else { return }
        var pieces: [Int32] = []
        var start = 0
        while start < scalars.count {
            var end = scalars.count
            var found: Int32?
            while start < end {
                var piece = String(String.UnicodeScalarView(scalars[start..<end]))
                if start > 0 { piece = subwordPrefix + piece }
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

    /// The Potion snapshot in the Hugging Face cache, where `model2vec`/`huggingface_hub` put it: the revision
    /// `refs/main` names, else the newest. The cache is found as huggingface_hub does: HF_HUB_CACHE, then
    /// HF_HOME/hub, then XDG_CACHE_HOME/huggingface/hub, then ~/.cache/huggingface/hub.
    package static func cachedSnapshot(home: URL, environment: [String: String]) -> URL? {
        let set = { (key: String) in environment[key].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) } }
        let hub = set("HF_HUB_CACHE")
            ?? set("HF_HOME").map { $0.appendingPathComponent("hub") }
            ?? set("XDG_CACHE_HOME").map { $0.appendingPathComponent("huggingface/hub") }
            ?? home.appendingPathComponent(".cache/huggingface/hub")
        let repository = hub.appendingPathComponent("models--minishlab--\(name)")
        let snapshots = repository.appendingPathComponent("snapshots")
        if let main = try? String(contentsOf: repository.appendingPathComponent("refs/main"), encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines), !main.isEmpty, !main.contains("/") {
            let snapshot = snapshots.appendingPathComponent(main)
            if FileManager.default.fileExists(atPath: snapshot.appendingPathComponent("model.safetensors").path) { return snapshot }
        }
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

#if canImport(TalaDB)
/// Turns text into a fixed-dimension vector for the index's embedding leg.
package protocol TextEmbedder: Sendable {
    var dimension: Int { get }
    func embed(_ text: String) async throws -> [Float]
    func embedBatch(_ texts: [String]) async throws -> [[Float]]
}

extension TextEmbedder {
    package func embedBatch(_ texts: [String]) async throws -> [[Float]] {
        var vectors: [[Float]] = []
        vectors.reserveCapacity(texts.count)
        for text in texts { vectors.append(try await embed(text)) }
        return vectors
    }
}

package struct PotionEmbedder: TextEmbedder {
    package let model: PotionModel
    package var dimension: Int { model.dimension }

    /// Text with no known tokens gets a fixed vector, leaving its ranking to the BM25 leg.
    package func embed(_ text: String) async throws -> [Float] {
        model.embed(text) ?? [Float](repeating: 1 / Float(dimension).squareRoot(), count: dimension)
    }
}
#endif
