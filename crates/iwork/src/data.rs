//! The files in a package's `Data/` folder: pictures and other media that objects name by data
//! ID (container.md, "Data files").

use std::collections::HashMap;

use craft_zip::Archive;

use crate::{Limits, Package};

/// The package metadata record in `Index/Metadata.iwa`, which lists the data files.
const METADATA_ID: u64 = 2;
const PACKAGE_METADATA: u32 = 11006;
/// The longest stored file name kept.
const MAX_NAME_BYTES: usize = 1024;

/// The data files of an opened package, read from its file on demand.
pub struct DataFiles<'a> {
    archive: Option<Archive<'a>>,
    /// The stored file name in `Data/` of each data ID.
    names: HashMap<u64, String>,
    max_file: u64,
}

impl<'a> DataFiles<'a> {
    /// The data files of `pkg`, which was opened from `bytes`. Reading them all together may
    /// expand to no more than `limits.max_decoded_bytes`.
    ///
    /// The package metadata (object 2, type 11006) lists one field-4 entry per data file: its
    /// data ID in field 1 and its stored name under `Data/` in field 4. Pages finds the file by
    /// that name whatever its pattern; an entry without one is a template resource that isn't in
    /// the package.
    pub fn new(pkg: &Package, bytes: &'a [u8], limits: &Limits) -> Self {
        let metadata = pkg.object_of(METADATA_ID, PACKAGE_METADATA).or_else(|| pkg.objects().find(|o| o.kind == PACKAGE_METADATA));
        let names = metadata
            .map(|m| {
                m.payload.messages(4).filter_map(|entry| Some((entry.varint(1)?, entry.text(4).filter(|n| is_plain_name(n))?.to_string()))).collect()
            })
            .unwrap_or_default();
        let zip_limits = craft_zip::Limits { max_entries: limits.max_entries, max_total_bytes: limits.max_decoded_bytes };
        DataFiles { archive: Archive::open(bytes, zip_limits).ok(), names, max_file: limits.max_decoded_bytes }
    }

    /// No data files: every object that needs one is left out.
    pub fn none() -> Self {
        DataFiles { archive: None, names: HashMap::new(), max_file: 0 }
    }

    /// The stored name of data ID `id`, if the package lists a file for it.
    pub fn name(&self, id: u64) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    /// The bytes of data ID `id`: `None` when it has no file, the file is missing or damaged, or
    /// reading it would pass the limit.
    pub fn read(&mut self, id: u64) -> Option<Vec<u8>> {
        let name = format!("Data/{}", self.names.get(&id)?);
        self.archive.as_mut()?.read(&name, self.max_file).ok().flatten()
    }
}

/// A file name of its own, with no folder in it.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_BYTES && !name.contains(['/', '\\', '\0']) && name != "." && name != ".."
}
