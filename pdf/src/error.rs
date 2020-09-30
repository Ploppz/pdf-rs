use crate::object::ObjNr;
use std::io;
use std::error::Error;

#[derive(Debug, Snafu)]
pub enum PdfError {
    // Syntax / parsing
    #[snafu(display("Unexpected end of file"))]
    EOF,

    #[snafu(display("Error parsing from string: {}", source))]
    Parse { source: Box<dyn Error + Send + Sync> },

    #[snafu(display("Invalid encoding: {}", source))]
    Encoding { source: Box<dyn Error + Send + Sync> },

    #[snafu(display("Out of bounds: index {}, but len is {}", index, len))]
    Bounds { index: usize, len: usize },

    #[snafu(display("Unexpected token '{}' at {} - expected '{}'", lexeme, pos, expected))]
    UnexpectedLexeme {pos: usize, lexeme: String, expected: &'static str},
    
    #[snafu(display("Expecting an object, encountered {} at pos {}. Rest:\n{}\n\n((end rest))", first_lexeme, pos, rest))]
    UnknownType {pos: usize, first_lexeme: String, rest: String},
    
    #[snafu(display("Unknown variant '{}' for enum {}", name, id))]
    UnknownVariant { id: &'static str, name: String },
    
    #[snafu(display("'{}' not found.", word))]
    NotFound { word: String },
    
    #[snafu(display("Cannot follow reference during parsing - no resolve fn given (most likely /Length of Stream)."))]
    Reference, // TODO: which one?
    
    #[snafu(display("Erroneous 'type' field in xref stream - expected 0, 1 or 2, found {}", found))]
    XRefStreamType { found: u64 },
    
    #[snafu(display("Parsing read past boundary of Contents."))]
    ContentReadPastBoundary,
    
    //////////////////
    // Encode/decode
    #[snafu(display("Hex decode error. Position {}, bytes {:?}", pos, bytes))]
    HexDecode {pos: usize, bytes: [u8; 2]},
    
    #[snafu(display("Ascii85 tail error"))]
    Ascii85TailError,
    
    #[snafu(display("Failed to convert '{}' into PredictorType", n))]
    IncorrectPredictorType {n: u8},
    
    //////////////////
    // Dictionary
    #[snafu(display("Can't parse field {} of struct {}.", field, typ))]
    FromPrimitive {
        typ: &'static str,
        field: &'static str,
        source: Box<PdfError>
    },
    
    #[snafu(display("Field /{} is missing in dictionary for type {}.", field, typ))]
    MissingEntry {
        typ: &'static str,
        field: String
    },
    
    #[snafu(display("Expected to find value {} for key {}. Found {} instead.", value, key, found))]
    KeyValueMismatch {
        key: String,
        value: String,
        found: String,
    },
    
    #[snafu(display("Expected dictionary /Type = {}. Found /Type = {}.", expected, found))]
    WrongDictionaryType {expected: String, found: String},
    
    //////////////////
    // Misc
    #[snafu(display("Tried to dereference free object nr {}.", obj_nr))]
    FreeObject {obj_nr: u64},
    
    #[snafu(display("Tried to dereference non-existing object nr {}.", obj_nr))]
    NullRef {obj_nr: u64},

    #[snafu(display("Expected primitive {}, found primive {} instead.", expected, found))]
    UnexpectedPrimitive {expected: &'static str, found: &'static str},
    /*
    WrongObjectType {expected: &'static str, found: &'static str} {
        description("Function called on object of wrong type.")
        display("Expected {}, found {}.", expected, found)
    }
    */
    #[snafu(display("Object stream index out of bounds ({}/{}).", index, max))]
    ObjStmOutOfBounds {index: usize, max: usize},
    
    #[snafu(display("Page out of bounds ({}/{}).", page_nr, max))]
    PageOutOfBounds {page_nr: u32, max: u32},
    
    #[snafu(display("Page {} could not be found in the page tree.", page_nr))]
    PageNotFound {page_nr: u32},
    
    #[snafu(display("Entry {} in xref table unspecified", id))]
    UnspecifiedXRefEntry {id: ObjNr},
    
    #[snafu(display("Invalid user password"))]
    InvalidPassword,
    
    #[snafu(display("JPEG"))]
    Jpeg { source: jpeg_decoder::Error },

    #[snafu(display("IO Error"))]
    Io { source: io::Error },
    
    #[snafu(display("{}", msg))]
    Other { msg: String },
    
    #[snafu(display("NoneError at {}:{}:{}", file, line, column))]
    NoneError { file: &'static str, line: u32, column: u32 },

    #[snafu(display("Try at {}:{}:{}", file, line, column))]
    Try { file: &'static str, line: u32, column: u32, source: Box<PdfError> },

    #[snafu(display("TryContext at {}:{}:{}: {:?}", file, line, column, context))]
    TryContext { file: &'static str, line: u32, column: u32, context: Vec<(&'static str, String)>, source: Box<PdfError> }
}
impl PdfError {
    pub fn trace(&self) {
        trace(self, 0);
    }
    pub fn is_eof(&self) -> bool {
        match self {
            &PdfError::EOF => true,
            &PdfError::Try { ref source, .. } | PdfError::TryContext { ref source, .. } => source.is_eof(),
            _ => false
        }
    }
}
fn trace(err: &dyn Error, depth: usize) {
    println!("{}: {}", depth, err);
    if let Some(source) = err.source() {
        trace(source, depth+1);
    }
}
    

pub type Result<T, E=PdfError> = std::result::Result<T, E>;

impl From<io::Error> for PdfError {
    fn from(source: io::Error) -> PdfError {
        PdfError::Io { source }
    }
}
impl From<String> for PdfError {
    fn from(msg: String) -> PdfError {
        PdfError::Other { msg }
    }
}

#[macro_export]
macro_rules! try_opt {
    ($e:expr) => (
        match $e {
            Some(v) => v,
            None => return Err($crate::PdfError::NoneError {
                file: file!(),
                line: line!(),
                column: column!()
            })
        }
    )
}

#[macro_export]
macro_rules! t {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return Err($crate::PdfError::Try { file: file!(), line: line!(), column: column!(), source: e.into() })
        }
    };
    ($e:expr, $($c:expr),*) => {
        match $e {
            Ok(v) => v,
            Err(e) => {
                let context = vec![ $( (stringify!($c), format!("{:?}", $c) ) ),* ];
                return Err($crate::PdfError::TryContext { file: file!(), line: line!(), column: column!(), context, source: e.into() })
            }
        }
    };
}

macro_rules! err_from {
    ($($st:ty),* => $variant:ident) => (
        $(
            impl From<$st> for PdfError {
                fn from(e: $st) -> PdfError {
                    PdfError::$variant { source: e.into() }
                }
            }
        )*
    )
}
err_from!(std::str::Utf8Error, std::string::FromUtf8Error, std::string::FromUtf16Error => Encoding);
err_from!(std::num::ParseIntError, std::string::ParseError => Parse);
err_from!(jpeg_decoder::Error => Jpeg);

macro_rules! err {
    ($e: expr) => ({
        return Err($e);
    })
}
macro_rules! bail {
    ($($t:tt)*) => {
        err!($crate::PdfError::Other { msg: format!($($t)*) })
    }
}
macro_rules! unimplemented {
    () => (bail!("Unimplemented @ {}:{}", file!(), line!()))
}

#[cfg(not(feature = "dump"))]
pub fn dump_data(_data: &[u8]) {}

#[cfg(feature = "dump")]
pub fn dump_data(data: &[u8]) {
    use std::io::Write;
    if let Some(path) = ::std::env::var_os("PDF_OUT") {
        let (mut file, path) = tempfile::Builder::new()
            .prefix("")
            .tempfile_in(path).unwrap()
            .keep().unwrap();
        file.write_all(&data).unwrap();
        info!("data written to {:?}", path);
    } else {
        info!("set PDF_OUT to an existing directory to dump stream data");
    }
}

#[cfg(test)]
mod tests {
    use super::PdfError;

    fn assert_send<T: Send>() {}

    fn assert_sync<T: Sync>() {}

    #[test]
    fn error_is_send_and_sync() {
        // note that these checks happens at compile time, not when the test is run
        assert_send::<PdfError>();
        assert_sync::<PdfError>();
    }
}
