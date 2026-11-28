//! Reading the documents that Pages, Keynote and Numbers save (`.pages`, `.key`, `.numbers`).
//!
//! Written from the open iwork-format specification, which is built from black-box experiments
//! (<https://alehel.github.io/iwork-format/>, source <https://github.com/alehel/iwork-format> at
//! commit `57b31c5`, CC-BY-4.0), and from the public Snappy and Protocol Buffers format
//! descriptions. It implements:
//!
//! - container.md: "The package" (ZIP files, and the folder form zipped whole with its
//!   `Index.zip`), "Chunks", "Records" and, for password-protected documents, telling them
//!   apart ("Password protection"; they aren't decrypted yet);
//! - container.md "Data files" ([`DataFiles`]): the files in `Data/`, found by data ID through
//!   the package metadata;
//! - with the `pages` feature ([`pages`]): text.md "The body text", "Paragraphs", "Positions in
//!   the text" and "Runs" (paragraph styles, character styles, attachments, links and bookmarks,
//!   columns, sections, drop caps, tracked deletions); styles.md "Character properties",
//!   "Paragraph properties" (borders among them), "Named and unnamed styles" and "Finding a
//!   character's formatting"; pages.md "Page setup" and the sections' page colour; drawables.md
//!   "On the page or in the text", "Which page an object is on", "Move with Text", "Text wrap",
//!   "Geometry" and "Images".
//!
//! Every file is treated as hostile: sizes are checked before anything is allocated, decoded
//! bytes and records are counted against [`Limits`], and nothing panics. A part that is damaged
//! partway keeps the records before the damage ([`Package::damage`] lists it).
//!
//! Pages, Keynote and Numbers are trademarks of Apple Inc. This crate isn't affiliated with or
//! endorsed by Apple.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable, clippy::indexing_slicing)]

mod data;
mod iwa;
mod package;
#[cfg(feature = "pages")]
pub mod pages;
mod snappy;
pub mod wire;

pub use data::DataFiles;
pub use package::{Damage, Object, Package};

/// What reading a document may use, whatever the file declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Most entries the ZIP archive may declare.
    pub max_entries: u64,
    /// Most bytes the parts may come to once read from the archive and once decoded, each.
    pub max_decoded_bytes: u64,
    /// Most records all the parts may hold together.
    pub max_records: usize,
}

impl Default for Limits {
    /// Room for documents many times larger than the experiments' biggest, small enough for a
    /// browser tab.
    fn default() -> Self {
        Limits { max_entries: 20_000, max_decoded_bytes: 512 << 20, max_records: 4_000_000 }
    }
}

/// Why a document wasn't read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The bytes aren't a ZIP archive with object parts.
    #[error("not a Pages, Keynote or Numbers document: {0}")]
    NotPackage(String),
    /// The document is protected with a password; `hint` is the hint it stores in plain text.
    #[error("the document is protected with a password, and opening protected documents isn't supported yet")]
    PasswordProtected { hint: Option<String> },
    /// The parts come to more than [`Limits::max_decoded_bytes`].
    #[error("the document comes to more than {limit} bytes once decoded")]
    TooLarge { limit: u64 },
    /// The parts hold more than [`Limits::max_records`] records.
    #[error("the document holds more than {limit} records")]
    TooManyRecords { limit: usize },
    /// The archive couldn't be read.
    #[error("the document's archive couldn't be read: {0}")]
    Zip(#[from] craft_zip::Error),
    /// A Pages document whose body text can't be found.
    #[error("the document's body text can't be found")]
    NoBody,
}
