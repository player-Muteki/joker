#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSet {
    read: bool,
    grep: bool,
    write: bool,
    shell: bool,
    web_search: bool,
}

impl ToolSet {
    #[must_use]
    pub fn new() -> Self {
        Self { read: false, grep: false, write: false, shell: false, web_search: false }
    }

    #[must_use]
    pub fn read(mut self) -> Self { self.read = true; self }
    #[must_use]
    pub fn grep(mut self) -> Self { self.grep = true; self }
    #[must_use]
    pub fn write(mut self) -> Self { self.write = true; self }
    #[must_use]
    pub fn shell(mut self) -> Self { self.shell = true; self }
    #[must_use]
    pub fn web_search(mut self) -> Self { self.web_search = true; self }

    #[must_use] pub fn has_read(&self) -> bool { self.read }
    #[must_use] pub fn has_grep(&self) -> bool { self.grep }
    #[must_use] pub fn has_write(&self) -> bool { self.write }
    #[must_use] pub fn has_shell(&self) -> bool { self.shell }
    #[must_use] pub fn has_web_search(&self) -> bool { self.web_search }
}

impl Default for ToolSet {
    fn default() -> Self { Self::new() }
}
