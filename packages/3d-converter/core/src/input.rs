//! What a read takes, and how a set of files becomes one model: which file is
//! the model, how its references find the others, and what role each file
//! played.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;

use litegen_mesh::{package, zip, Sidecars};
use unicode_normalization::UnicodeNormalization;

use crate::detect::{by_extension, detect_format};
use crate::{Error, ErrorCode, Format, OptionName, ReadOptions, MAX_INPUT_BYTES};

/// What a read takes: one file's bytes, or a set of named files (a model and
/// the files it references, such as an OBJ's `.mtl` and textures). One file
/// that is a zip of a package, as litegen's `package = "zip"` writes, is
/// unpacked and read as its files.
#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    /// One file's bytes.
    Bytes(&'a [u8]),
    /// Named files: the model and the files it references.
    Files(&'a [InputFile<'a>]),
}

/// One named input file, its bytes borrowed or owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputFile<'a> {
    /// A relative path (`textures/a.png`); the model's references are
    /// resolved against it.
    pub name: String,
    pub data: Cow<'a, [u8]>,
}

/// What a file is to the model it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileRole {
    /// The model file itself.
    Model,
    /// An OBJ's `.mtl`.
    MaterialLibrary,
    /// A glTF's `.bin`, or another binary the model references.
    Buffer,
    /// An image a material uses.
    Texture,
    /// A USD layer the root layer composes (a sublayer, reference or payload).
    Layer,
    /// Given, but nothing referenced it.
    Unused,
}

impl FileRole {
    pub const ALL: [FileRole; 6] = [FileRole::Model, FileRole::MaterialLibrary, FileRole::Buffer, FileRole::Texture, FileRole::Layer, FileRole::Unused];

    pub fn as_str(self) -> &'static str {
        match self {
            FileRole::Model => "model",
            FileRole::MaterialLibrary => "material_library",
            FileRole::Buffer => "buffer",
            FileRole::Texture => "texture",
            FileRole::Layer => "layer",
            FileRole::Unused => "unused",
        }
    }
}

/// One input file as the read used it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputFileInfo {
    pub name: String,
    pub role: FileRole,
    pub size: u64,
}

/// The role a referenced file plays, from its extension.
pub(crate) fn role_for(name: &str) -> FileRole {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "mtl" => FileRole::MaterialLibrary,
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "ktx2" | "bmp" | "tga" | "tif" | "tiff" => FileRole::Texture,
        "usd" | "usda" | "usdc" => FileRole::Layer,
        _ => FileRole::Buffer,
    }
}

/// One file of the input, borrowed or unpacked from a zip. `name` is `None`
/// only for unnamed bytes, until the format names it `model.<ext>`.
pub(crate) struct Entry<'a> {
    pub name: Option<String>,
    pub data: Cow<'a, [u8]>,
}

impl Entry<'_> {
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or("")
    }
}

fn too_large(total: usize) -> Error {
    Error::new(ErrorCode::InputTooLarge, None).text(format!("the input is {total} bytes, over the {MAX_INPUT_BYTES}-byte limit"))
}

fn check_total(sizes: impl Iterator<Item = usize>) -> Result<(), Error> {
    check_input_size(sizes.fold(0usize, usize::saturating_add))
}

/// The input cap, for a binding to check before it copies an input in:
/// `total` bytes of input files is refused over [`MAX_INPUT_BYTES`], with the
/// error a read gives.
pub fn check_input_size(total: usize) -> Result<(), Error> {
    if total > MAX_INPUT_BYTES {
        return Err(too_large(total));
    }
    Ok(())
}

/// The input as entries, and the name of the lone zip they were unpacked
/// from, if they were.
pub(crate) struct Gathered<'a> {
    pub entries: Vec<Entry<'a>>,
    pub zip_name: Option<String>,
}

/// The input as entries: given files as they are, and a lone zip that is not
/// itself a model (a 3MF or USDZ) unpacked.
pub(crate) fn gather<'a>(input: Input<'a>, options: &ReadOptions) -> Result<Gathered<'a>, Error> {
    let entries: Vec<Entry<'a>> = match input {
        Input::Bytes(data) => vec![Entry { name: options.file_name.clone(), data: Cow::Borrowed(data) }],
        Input::Files(files) => files.iter().map(|f| Entry { name: Some(f.name.clone()), data: Cow::Borrowed(&f.data) }).collect(),
    };
    check_total(entries.iter().map(|e| e.data.len()))?;
    if let [only] = entries.as_slice() {
        let model_zip = matches!(options.from, Some(Format::ThreeMf | Format::Usdz)) || detect_format(&only.data, None).is_some();
        if zip::looks_like_zip(&only.data) && !model_zip {
            return Ok(Gathered { entries: unpack(&only.data)?, zip_name: only.name.clone() });
        }
    }
    Ok(Gathered { entries, zip_name: None })
}

/// Files a Mac adds to the zips it makes, which are never part of a model.
fn is_junk(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    name.starts_with("__MACOSX/") || base.starts_with("._") || base == ".DS_Store"
}

fn unpack(data: &[u8]) -> Result<Vec<Entry<'static>>, Error> {
    let archive = zip::Archive::parse(data)?;
    let files: Vec<&zip::Entry> = archive.entries.iter().filter(|e| !e.name.ends_with('/') && !is_junk(&e.name)).collect();
    // Each entry inflates no further than its declared size (Archive::read),
    // so the declared sizes bound what unpacking costs.
    check_total(files.iter().map(|e| e.uncompressed_size()))?;
    files.into_iter().map(|e| Ok(Entry { name: Some(e.name.clone()), data: Cow::Owned(archive.read(e)?) })).collect()
}

fn listed(entries: &[Entry<'_>], which: impl IntoIterator<Item = usize>) -> String {
    which.into_iter().map(|i| entries[i].name().to_string()).collect::<Vec<_>>().join(", ")
}

pub(crate) fn unrecognized(entries: &[Entry<'_>], from: Option<Format>) -> Error {
    let e = Error::new(ErrorCode::FormatUnrecognized, None);
    match (entries, from) {
        ([], _) => e.text("no input files were given"),
        ([_, _, ..], Some(f)) => e
            .text(format!("no file among the files ({}) is a {} file; check ", listed(entries, 0..entries.len()), f.as_str()))
            .option(OptionName::From)
            .text(", or pass ")
            .option(OptionName::FileName)
            .text(" to name the model"),
        ([_], _) => e.text("the input's format could not be recognised from its bytes or its name; pass ").option(OptionName::From),
        _ => e
            .text(format!("no model among the files ({}); pass ", listed(entries, 0..entries.len())))
            .option(OptionName::From)
            .text(" or ")
            .option(OptionName::FileName),
    }
}

/// The index of the model file among `entries`. `file_name` names one of
/// them, or, when they were unpacked from a lone zip, may name the zip
/// itself, which then says nothing about which file is the model.
pub(crate) fn choose_primary(entries: &[Entry<'_>], options: &ReadOptions, zip_name: Option<&str>) -> Result<usize, Error> {
    match entries {
        [] => return Err(unrecognized(entries, options.from)),
        [_] => return Ok(0),
        _ => {}
    }
    if let Some(name) = &options.file_name {
        let want = normalize(name);
        if let Some(i) = entries.iter().position(|e| normalize(e.name()) == want) {
            return Ok(i);
        }
        if zip_name.is_none_or(|zip| normalize(zip) != want) {
            return Err(Error::new(ErrorCode::ParamInvalid, Some(OptionName::FileName))
                .option(OptionName::FileName)
                .text(format!(" is {name:?}, which is not among the files")));
        }
    }
    let fits = |f: Format| options.from.is_none_or(|from| from == f);
    let mut candidates: Vec<usize> = (0..entries.len())
        .filter(|&i| !is_junk(entries[i].name()) && by_extension(entries[i].name()).is_some_and(fits))
        .collect();
    if candidates.is_empty() {
        candidates = (0..entries.len()).filter(|&i| detect_format(&entries[i].data, None).is_some_and(fits)).collect();
    }
    match candidates.as_slice() {
        [one] => Ok(*one),
        [] => Err(unrecognized(entries, options.from)),
        many => Err(Error::new(ErrorCode::InputAmbiguous, Some(OptionName::FileName))
            .text(format!("several files could be the model ({}); pass ", listed(entries, many.iter().copied())))
            .option(OptionName::FileName)
            .text(" to name it")),
    }
}

/// A file name as a package path: `/`-separated, without a leading `./` or
/// `/`, in Unicode's composed form (NFC), so `Textu\u{308}r.png` as macOS
/// spells it is the `Textür.png` a model names.
fn normalize(name: &str) -> String {
    let name = name.replace('\\', "/");
    let mut s = name.as_str();
    loop {
        if let Some(rest) = s.strip_prefix("./") {
            s = rest;
        } else if let Some(rest) = s.strip_prefix('/') {
            s = rest;
        } else {
            return s.nfc().collect();
        }
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Resolves the model's references among the other files: relative to the
/// model's own folder first, then from the package root, then by a unique
/// case-insensitive file name (Windows-made OBJs, and browser drops, which
/// arrive without their folders). Records which files were asked for.
pub(crate) struct Resolver<'e, 'a> {
    entries: &'e [Entry<'a>],
    names: Vec<String>,
    primary: usize,
    requested: RefCell<BTreeSet<usize>>,
}

impl<'e, 'a> Resolver<'e, 'a> {
    pub fn new(entries: &'e [Entry<'a>], primary: usize) -> Resolver<'e, 'a> {
        Resolver { entries, names: entries.iter().map(|e| normalize(e.name())).collect(), primary, requested: RefCell::default() }
    }

    fn find(&self, path: &str) -> Option<usize> {
        let path: String = path.nfc().collect();
        let path = path.as_str();
        let others = || (0..self.names.len()).filter(|&i| i != self.primary);
        let exact = |p: &str| others().find(|&i| self.names[i] == p);
        if let Some(full) = package::resolve_relative(&self.names[self.primary], path)
            && let Some(i) = exact(&full)
        {
            return Some(i);
        }
        if let Some(i) = exact(path) {
            return Some(i);
        }
        // Case-insensitively beyond ASCII too (`TEXTÜR.PNG` for `Textür.png`).
        let base = basename(path).to_lowercase();
        let mut by_name = others().filter(|&i| basename(&self.names[i]).to_lowercase() == base);
        match (by_name.next(), by_name.next()) {
            (Some(i), None) => Some(i),
            _ => None,
        }
    }

    pub fn requested(&self) -> BTreeSet<usize> {
        self.requested.borrow().clone()
    }
}

impl Sidecars for Resolver<'_, '_> {
    fn get(&self, path: &str) -> Option<Vec<u8>> {
        let i = self.find(path)?;
        self.requested.borrow_mut().insert(i);
        Some(self.entries[i].data.to_vec())
    }
}

/// Each entry's role in the read: the model, what it asked for, the rest.
pub(crate) fn roles(entries: &[Entry<'_>], primary: usize, requested: &BTreeSet<usize>) -> Vec<InputFileInfo> {
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| InputFileInfo {
            name: e.name().to_string(),
            role: if i == primary {
                FileRole::Model
            } else if requested.contains(&i) {
                role_for(e.name())
            } else {
                FileRole::Unused
            },
            size: e.data.len() as u64,
        })
        .collect()
}
