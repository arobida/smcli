//! Pluggable sources of command examples for `@@` completion. smcli-core owns the trait;
//! implementations (tldr, later cheat.sh) live in crates that have their data loaders.
//! Sources answer offline by program name; smcli decides which programs to ask about.

/// One example of using a program: a description and a command template.
/// `source` names the source that supplied it so results can be attributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandExample {
    pub program: String,
    pub description: String,
    pub template: String,
    pub source: String,
}

/// A named, versioned supplier of examples for programs on a platform.
/// `version` distinguishes corpora from the same source (e.g. a tldr commit),
/// so an index built against one version is rebuilt when the data changes.
pub trait CommandSource {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    /// Examples for `program` on `platform`, empty when the source has none.
    fn examples(&self, program: &str, platform: &str) -> Vec<CommandExample>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub source proves the trait shape without any real corpus.
    struct Stub;

    impl CommandSource for Stub {
        fn name(&self) -> &str {
            "stub"
        }
        fn version(&self) -> &str {
            "1"
        }
        fn examples(&self, program: &str, _platform: &str) -> Vec<CommandExample> {
            if program == "jq" {
                vec![CommandExample {
                    program: "jq".into(),
                    description: "Pretty-print JSON".into(),
                    template: "jq . {{file}}".into(),
                    source: self.name().into(),
                }]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn stub_source_answers_by_name() {
        let source = Stub;
        assert_eq!(source.name(), "stub");
        assert_eq!(source.version(), "1");
        let examples = source.examples("jq", "osx");
        assert_eq!(examples.len(), 1);
        assert_eq!(examples[0].source, "stub");
        assert_eq!(examples[0].template, "jq . {{file}}");
        assert!(source.examples("nope", "osx").is_empty());
    }
}
