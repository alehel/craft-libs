//! The package: a ZIP file whose `Index/*.iwa` parts hold the document's objects
//! (container.md, "The package").

use std::collections::HashMap;
use std::ops::Range;

use craft_zip::Archive;

use crate::iwa::{self, PartError};
use crate::wire::Message;
use crate::{Error, Limits};

/// The file that marks a password-protected document (container.md, "Password protection").
const PASSWORD_FILE: &str = ".iwpv2";
/// Its password hint, as plain text.
const HINT_FILE: &str = ".iwph";
/// The folder form keeps its parts in this inner archive, under the same `Index/` names.
const INNER_INDEX: &str = "Index.zip";
/// The longest password hint kept.
const MAX_HINT_BYTES: u64 = 4096;

/// A decoded part: its name in the package and its records' bytes.
#[derive(Debug)]
struct Part {
    name: String,
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Located {
    part: usize,
    payloads: Vec<(u32, Range<usize>)>,
}

/// A part that didn't decode in full, and why. Records before the damage are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Damage {
    pub part: String,
    pub what: String,
}

/// A record of the document: an object with an ID, a type and a payload.
#[derive(Clone, Copy, Debug)]
pub struct Object<'p> {
    pub id: u64,
    /// The type of its first payload of a type other than 0; type-0 payloads are extras the
    /// apps ignore (container.md, "Records").
    pub kind: u32,
    /// The part it was read from, such as `Index/Document.iwa`.
    pub part: &'p str,
    pub payload: Message<'p>,
}

/// An opened document: every object of its `.iwa` parts, by ID.
#[derive(Debug)]
pub struct Package {
    parts: Vec<Part>,
    objects: HashMap<u64, Located>,
    /// Object IDs in the order they were read: parts by name, records in part order.
    order: Vec<u64>,
    damage: Vec<Damage>,
}

impl Package {
    /// Open a document saved as a single file (a ZIP archive) with [`Limits::default`].
    pub fn open(bytes: &[u8]) -> Result<Package, Error> {
        Package::open_with(bytes, &Limits::default())
    }

    pub fn open_with(bytes: &[u8], limits: &Limits) -> Result<Package, Error> {
        let zip_limits = craft_zip::Limits { max_entries: limits.max_entries, max_total_bytes: limits.max_decoded_bytes };
        let mut archive = Archive::open(bytes, zip_limits).map_err(|e| Error::NotPackage(e.to_string()))?;
        if archive.contains(PASSWORD_FILE) {
            let hint = archive.read(HINT_FILE, MAX_HINT_BYTES).ok().flatten().map(|b| String::from_utf8_lossy(&b).trim().to_string());
            return Err(Error::PasswordProtected { hint: hint.filter(|h| !h.is_empty()) });
        }
        let mut names: Vec<String> = archive.names().filter(|n| is_object_part(n)).map(|n| n.into_owned()).collect();
        if names.is_empty() && archive.contains(INNER_INDEX) {
            // The folder form, zipped whole: its parts are in the inner archive.
            let inner = archive.read(INNER_INDEX, limits.max_decoded_bytes)?.unwrap_or_default();
            let mut inner_archive = Archive::open(&inner, zip_limits).map_err(|e| Error::NotPackage(e.to_string()))?;
            names = inner_archive.names().filter(|n| is_object_part(n)).map(|n| n.into_owned()).collect();
            return Package::read_parts(&mut inner_archive, names, limits);
        }
        Package::read_parts(&mut archive, names, limits)
    }

    fn read_parts(archive: &mut Archive<'_>, mut names: Vec<String>, limits: &Limits) -> Result<Package, Error> {
        if names.is_empty() {
            return Err(Error::NotPackage("it has no Index/*.iwa parts".into()));
        }
        names.sort();
        let mut pkg = Package { parts: Vec::new(), objects: HashMap::new(), order: Vec::new(), damage: Vec::new() };
        let mut allowance = limits.max_decoded_bytes;
        let mut records_left = limits.max_records;
        for name in names {
            let stored = match archive.read(&name, limits.max_decoded_bytes) {
                Ok(Some(b)) => b,
                Ok(None) => continue,
                Err(e) if e.is_limit() => return Err(Error::TooLarge { limit: limits.max_decoded_bytes }),
                Err(e) => {
                    pkg.damage.push(Damage { part: name, what: e.to_string() });
                    continue;
                }
            };
            let (data, chunk_err) = iwa::decode_chunks(&stored, &mut allowance);
            if chunk_err == Some(PartError::TooLarge) {
                return Err(Error::TooLarge { limit: limits.max_decoded_bytes });
            }
            let (records, record_err) = iwa::split_records(&data, records_left);
            if record_err == Some(PartError::TooLarge) {
                return Err(Error::TooManyRecords { limit: limits.max_records });
            }
            records_left -= records.len();
            if let Some(PartError::Damaged(what)) = chunk_err.or(record_err) {
                pkg.damage.push(Damage { part: name.clone(), what: what.to_string() });
            }
            pkg.add_part(name, data, records);
        }
        Ok(pkg)
    }

    fn add_part(&mut self, name: String, data: Vec<u8>, records: Vec<iwa::RawRecord>) {
        let part = self.parts.len();
        for r in records {
            // IDs are unique across a document; should one repeat, the first is kept.
            if self.objects.contains_key(&r.id) {
                continue;
            }
            let payloads = r.payloads.into_iter().map(|p| (p.kind, p.range)).collect();
            self.objects.insert(r.id, Located { part, payloads });
            self.order.push(r.id);
        }
        self.parts.push(Part { name, data });
    }

    /// The object with this ID.
    pub fn object(&self, id: u64) -> Option<Object<'_>> {
        let loc = self.objects.get(&id)?;
        let part = self.parts.get(loc.part)?;
        let (kind, range) = loc.payloads.iter().find(|(k, _)| *k != 0)?;
        let payload = Message::new(part.data.get(range.clone())?);
        Some(Object { id, kind: *kind, part: &part.name, payload })
    }

    /// The object with this ID, when it is of type `kind`.
    pub fn object_of(&self, id: u64, kind: u32) -> Option<Object<'_>> {
        self.object(id).filter(|o| o.kind == kind)
    }

    /// Every object, parts in name order and records in the order they are stored.
    pub fn objects(&self) -> impl Iterator<Item = Object<'_>> {
        self.order.iter().filter_map(|id| self.object(*id))
    }

    /// The names of the parts read, such as `Index/Document.iwa`.
    pub fn parts(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().map(|p| p.name.as_str())
    }

    /// Parts that didn't decode in full.
    pub fn damage(&self) -> &[Damage] {
        &self.damage
    }
}

/// Is this an object part, `Index/*.iwa` at any depth below `Index/` (tables keep theirs in
/// `Index/Tables/`)?
fn is_object_part(name: &str) -> bool {
    name.starts_with("Index/") && name.ends_with(".iwa")
}
