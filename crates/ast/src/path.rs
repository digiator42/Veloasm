//! Object/array traversal paths used by [`Expr::Payload`](crate::Expr::Payload)
//! and [`Expr::Signal`](crate::Expr::Signal) projections.
//!
//! A path is an ordered list of [`PathSeg`]s. It exists so a single reactive
//! signal holding a JSON-shaped value can be *projected* into a fine-grained
//! read — `{ sig "threats".path }` subscribes to the whole `threats` value
//! rather than materialising one signal per record.

use std::fmt;

/// One step of an [`ObjPath`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t", content = "v"))]
pub enum PathSeg {
    /// Object field access: `.name`
    Key(String),
    /// Array index access: `[0]`
    Index(usize),
}

/// An ordered traversal into a JSON-shaped value.
///
/// One surface syntax, with an unambiguous rule: **dotted segments are keys,
/// bracketed ones are indices.** `a.b.c` walks object fields; `a[0].b` reads the
/// first element of a list. A key that is itself the text `"0"` must therefore
/// be reached with brackets (`a["0"]`), never with a dot.
///
/// ```
/// use velo_ast::{ObjPath, PathSeg};
///
/// let dotted = ObjPath::parse("threats.name").unwrap();
/// let bracketed = ObjPath::parse("threats[0].name").unwrap();
/// assert_eq!(dotted, ObjPath::parse("threats[\"name\"]").unwrap());
/// assert_eq!(bracketed.segments()[1], PathSeg::Index(0));
/// assert_eq!(ObjPath::parse("a[2][3]").unwrap().segments().len(), 3);
/// assert!(ObjPath::parse("a[2").is_none(), "unterminated bracket");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct ObjPath(Vec<PathSeg>);

impl ObjPath {
    /// The empty path — addresses the value itself.
    pub fn root() -> Self {
        ObjPath(Vec::new())
    }

    /// The single-segment path `key`.
    pub fn key(key: impl Into<String>) -> Self {
        ObjPath(vec![PathSeg::Key(key.into())])
    }

    /// Build a path from segments.
    pub fn from_segments(segs: Vec<PathSeg>) -> Self {
        ObjPath(segs)
    }

    /// The segments, in order.
    pub fn segments(&self) -> &[PathSeg] {
        &self.0
    }

    /// `true` when the path addresses the value itself.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Append one segment, consuming and returning `self` (builder style).
    pub fn then(mut self, seg: PathSeg) -> Self {
        self.0.push(seg);
        self
    }

    /// Parse `a.b[0].c` or `a.0.c`. Returns `None` on a malformed path.
    pub fn parse(input: &str) -> Option<Self> {
        let mut segs = Vec::new();
        let bytes: Vec<char> = input.chars().collect();
        let mut i = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                '.' => {
                    i += 1;
                }
                '[' => {
                    i += 1;
                    let start = i;
                    while i < bytes.len() && bytes[i] != ']' {
                        i += 1;
                    }
                    if i >= bytes.len() {
                        return None; // unterminated `[`
                    }
                    let inner: String = bytes[start..i].iter().collect();
                    i += 1; // consume `]`
                    match inner.parse::<usize>() {
                        Ok(idx) => segs.push(PathSeg::Index(idx)),
                        // `["quoted"]` is accepted as a key for parity with JS.
                        Err(_) => {
                            let trimmed = inner.trim_matches(|c| c == '"' || c == '\'');
                            if trimmed.is_empty() {
                                return None;
                            }
                            segs.push(PathSeg::Key(trimmed.to_string()));
                        }
                    }
                }
                _ => {
                    let start = i;
                    while i < bytes.len() && bytes[i] != '.' && bytes[i] != '[' {
                        i += 1;
                    }
                    let key: String = bytes[start..i].iter().collect();
                    if key.is_empty() {
                        return None; // stray separator
                    }
                    segs.push(PathSeg::Key(key));
                }
            }
        }
        Some(ObjPath(segs))
    }
}

impl fmt::Display for ObjPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for seg in &self.0 {
            match seg {
                PathSeg::Key(k) => {
                    if !first {
                        f.write_str(".")?;
                    }
                    f.write_str(k)?;
                }
                PathSeg::Index(n) => write!(f, "[{n}]")?,
            }
            first = false;
        }
        Ok(())
    }
}

impl From<Vec<PathSeg>> for ObjPath {
    fn from(segs: Vec<PathSeg>) -> Self {
        ObjPath(segs)
    }
}

impl FromIterator<PathSeg> for ObjPath {
    fn from_iter<I: IntoIterator<Item = PathSeg>>(iter: I) -> Self {
        ObjPath(iter.into_iter().collect())
    }
}