//! What every binding raises: a stable code spelled as litegen's HTTP API
//! spells it, the option it is about, and a message whose option names each
//! binding renders in its own spelling (`to` in TypeScript, `to_format` in
//! Python).

use crate::OptionName;

/// Why a conversion failed. The spellings are the HTTP converter's error
/// codes, plus the three a library needs that an HTTP body cannot hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// No format was given and none could be recognised from the bytes or
    /// the file names; also when a set of files holds no model file.
    FormatUnrecognized,
    /// The bytes are not the format they were said to be (bad magic, bad
    /// header).
    FormatMismatch,
    /// Several of the given files could be the model.
    InputAmbiguous,
    /// The input is over [`crate::MAX_INPUT_BYTES`].
    InputTooLarge,
    /// The file is that format, but unreadable.
    MeshMalformed,
    /// The file is valid but uses something the converter does not model, or
    /// is past one of its budgets.
    MeshUnsupported,
    /// The model holds no triangles, lines or points.
    MeshEmpty,
    /// An option that does not apply to this format (or does not exist).
    ParamUnsupported,
    /// An option's value is not one it takes.
    ParamInvalid,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 9] = [
        ErrorCode::FormatUnrecognized,
        ErrorCode::FormatMismatch,
        ErrorCode::InputAmbiguous,
        ErrorCode::InputTooLarge,
        ErrorCode::MeshMalformed,
        ErrorCode::MeshUnsupported,
        ErrorCode::MeshEmpty,
        ErrorCode::ParamUnsupported,
        ErrorCode::ParamInvalid,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::FormatUnrecognized => "format_unrecognized",
            ErrorCode::FormatMismatch => "format_mismatch",
            ErrorCode::InputAmbiguous => "input_ambiguous",
            ErrorCode::InputTooLarge => "input_too_large",
            ErrorCode::MeshMalformed => "mesh_malformed",
            ErrorCode::MeshUnsupported => "mesh_unsupported",
            ErrorCode::MeshEmpty => "mesh_empty",
            ErrorCode::ParamUnsupported => "param_unsupported",
            ErrorCode::ParamInvalid => "param_invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Part {
    Text(String),
    Option(OptionName),
}

/// A failed read, write or conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub code: ErrorCode,
    /// The option the error is about, when there is one.
    pub param: Option<OptionName>,
    parts: Vec<Part>,
}

impl Error {
    /// A new error, for the bindings that check their own inputs; its message is
    /// built with [`Error::text`] and [`Error::option`].
    pub fn new(code: ErrorCode, param: Option<OptionName>) -> Error {
        Error { code, param, parts: Vec::new() }
    }

    /// Appends text to the message.
    pub fn text(mut self, text: impl Into<String>) -> Error {
        self.parts.push(Part::Text(text.into()));
        self
    }

    /// Adds an option's name, rendered by [`Error::message_with`].
    pub fn option(mut self, name: OptionName) -> Error {
        self.parts.push(Part::Option(name));
        self
    }

    /// The message, with each option named the way `spell` names it, in
    /// backticks.
    pub fn message_with(&self, spell: impl Fn(OptionName) -> &'static str) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Option(o) => {
                    out.push('`');
                    out.push_str(spell(*o));
                    out.push('`');
                }
            }
        }
        out
    }

    /// The message, with options in their Rust spelling ([`OptionName::as_str`]).
    pub fn message(&self) -> String {
        self.message_with(OptionName::as_str)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for Error {}

impl From<litegen_mesh::MeshError> for Error {
    fn from(e: litegen_mesh::MeshError) -> Error {
        use litegen_mesh::MeshError as M;
        let code = match &e {
            M::NotThisFormat { .. } => ErrorCode::FormatMismatch,
            M::Malformed(_) => ErrorCode::MeshMalformed,
            M::Unsupported(_) => ErrorCode::MeshUnsupported,
            M::Empty => ErrorCode::MeshEmpty,
        };
        Error::new(code, None).text(e.to_string())
    }
}
