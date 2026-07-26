/// A builder for a set of enabled tool flags.
///
/// Used to describe which categories of tools an agent profile permits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSet {
    read: bool,
    grep: bool,
    write: bool,
    shell: bool,
    web_search: bool,
}

impl ToolSet {
    /// All tools disabled by default.
    #[must_use]
    pub fn new() -> Self {
        Self { read: false, grep: false, write: false, shell: false, web_search: false }
    }

    /// Enable read tools.
    #[must_use]
    pub fn read(mut self) -> Self { self.read = true; self }
    /// Enable grep tools.
    #[must_use]
    pub fn grep(mut self) -> Self { self.grep = true; self }
    /// Enable write tools.
    #[must_use]
    pub fn write(mut self) -> Self { self.write = true; self }
    /// Enable shell tools.
    #[must_use]
    pub fn shell(mut self) -> Self { self.shell = true; self }
    /// Enable web-search tools.
    #[must_use]
    pub fn web_search(mut self) -> Self { self.web_search = true; self }

    /// Whether read tools are enabled.
    #[must_use] pub fn has_read(&self) -> bool { self.read }
    /// Whether grep tools are enabled.
    #[must_use] pub fn has_grep(&self) -> bool { self.grep }
    /// Whether write tools are enabled.
    #[must_use] pub fn has_write(&self) -> bool { self.write }
    /// Whether shell tools are enabled.
    #[must_use] pub fn has_shell(&self) -> bool { self.shell }
    /// Whether web-search tools are enabled.
    #[must_use] pub fn has_web_search(&self) -> bool { self.web_search }
}

impl Default for ToolSet {
    fn default() -> Self { Self::new() }
}
